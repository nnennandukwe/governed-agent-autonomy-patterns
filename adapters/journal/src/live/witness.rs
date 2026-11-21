use super::archive;
use super::authority::Admission;
use super::observation::{Start, Watch, ambiguous};
use crate::storage::linux::Volume;
use crate::writer::Submission;
use crate::{Result, canonical, digest};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const LIMIT: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Arm { start: Start },
    Bind { name: String },
    Finish,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum Reply {
    Ok { value: Value },
    Error { volume_eio: bool },
}

fn receive<T: DeserializeOwned>(s: &mut UnixStream) -> Result<T> {
    let mut header = [0u8; 4];
    s.read_exact(&mut header).map_err(|_| ambiguous())?;
    let n = u32::from_be_bytes(header) as usize;
    if n > LIMIT {
        return Err(ambiguous());
    }
    let mut bytes = vec![0; n];
    s.read_exact(&mut bytes).map_err(|_| ambiguous())?;
    serde_json::from_slice(&bytes).map_err(|_| ambiguous())
}

fn send<T: Serialize>(s: &mut UnixStream, value: &T) -> Result<()> {
    let bytes = canonical(value)?;
    if bytes.len() > LIMIT {
        return Err(ambiguous());
    }
    s.write_all(&(bytes.len() as u32).to_be_bytes())
        .and_then(|()| s.write_all(&bytes))
        .map_err(|_| ambiguous())
}

fn deadlines(s: &UnixStream) -> Result<()> {
    s.set_read_timeout(Some(TIMEOUT))
        .and_then(|()| s.set_write_timeout(Some(TIMEOUT)))
        .map_err(|_| ambiguous())
}

/// Private helper entry for the shipping binary. It only reads the connected socket and workspace.
pub fn witness_main() -> Result<()> {
    let fd = io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .map_err(|_| ambiguous())?;
    let mut channel = UnixStream::from(fd);
    channel.peer_addr().map_err(|_| ambiguous())?;
    deadlines(&channel)?;
    let result = (|| {
        let Request::Arm { start } = receive(&mut channel)? else {
            return Err(ambiguous());
        };
        let mut watch = Watch::arm(start)?;
        send(
            &mut channel,
            &Reply::Ok {
                value: watch.before().clone(),
            },
        )?;
        match receive(&mut channel)? {
            Request::Bind { name } => {
                watch.bind_stage(name)?;
                send(&mut channel, &Reply::Ok { value: Value::Null })?;
                if !matches!(receive(&mut channel)?, Request::Finish) {
                    return Err(ambiguous());
                }
            }
            Request::Finish => (),
            Request::Arm { .. } => return Err(ambiguous()),
        }
        send(
            &mut channel,
            &Reply::Ok {
                value: watch.finish()?,
            },
        )
    })();
    if let Err(e) = &result {
        let _ = send(
            &mut channel,
            &Reply::Error {
                volume_eio: e.reason == "witness_workspace_eio",
            },
        );
    }
    result
}

pub struct EffectEvidence {
    pub disposition: String,
    pub after: Option<Value>,
    pub source: Option<Value>,
}

pub struct Witness {
    child: Child,
    channel: UnixStream,
    binding: String,
    before: Value,
}

impl Witness {
    pub fn arm(
        tx: &Submission<'_>,
        binding: &str,
        attempt_id: &str,
        proposed_artifact_digest: &str,
    ) -> Result<Self> {
        let host = tx.host();
        let (channel, child_socket) = UnixStream::pair().map_err(|_| ambiguous())?;
        deadlines(&channel)?;
        let mut cmd = Command::new("/proc/self/exe");
        cmd.arg("__journal-witness")
            .env_clear()
            .stdin(Stdio::from(OwnedFd::from(child_socket)))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: pre_exec performs only the raw close_range syscall. It marks unknown
        // inherited descriptors close-on-exec without touching the new standard streams.
        unsafe {
            cmd.pre_exec(|| {
                if libc::syscall(
                    libc::SYS_close_range,
                    3u32,
                    u32::MAX,
                    libc::CLOSE_RANGE_CLOEXEC,
                ) < 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn().map_err(|_| ambiguous())?;
        let mut witness = Self {
            child,
            channel,
            binding: binding.into(),
            before: Value::Null,
        };
        witness.before = witness.exchange(
            tx.volume(),
            Request::Arm {
                start: Start {
                    workspace: host.slot.workspace.clone(),
                    mount_id: host.slot.mount_id,
                    binding: binding.into(),
                    attempt_id: attempt_id.into(),
                    proposed: proposed_artifact_digest.into(),
                },
            },
        )?;
        Ok(witness)
    }

    pub fn before(&self) -> &Value {
        &self.before
    }

    fn exchange(&mut self, volume: &Volume<'_>, request: Request) -> Result<Value> {
        send(&mut self.channel, &request)?;
        match receive(&mut self.channel)? {
            Reply::Ok { value } => Ok(value),
            Reply::Error { volume_eio: true } => volume.io(
                Err(io::Error::from_raw_os_error(libc::EIO)),
                false,
                "witness_workspace_read",
            ),
            Reply::Error { volume_eio: false } => Err(ambiguous()),
        }
    }

    pub fn bind_stage(&mut self, tx: &Submission<'_>, basename: &str) -> Result<()> {
        self.exchange(
            tx.volume(),
            Request::Bind {
                name: basename.into(),
            },
        )
        .map(|_| ())
    }

    pub fn finish(mut self, tx: &Submission<'_>, admission: &Admission) -> Result<EffectEvidence> {
        // No callback from this invocation can obtain a rename permit after this point.
        admission.retire();
        let raw = match self.exchange(tx.volume(), Request::Finish) {
            Ok(raw) => raw,
            Err(e) if e.reason == "ambiguous_effect_witness" => {
                return Ok(EffectEvidence {
                    disposition: "unresolved".into(),
                    after: None,
                    source: None,
                });
            }
            Err(e) => return Err(e),
        };
        let disposition = raw["disposition"]
            .as_str()
            .filter(|s| matches!(*s, "replaced" | "stopped_before_rename" | "unresolved"))
            .ok_or_else(ambiguous)?;
        let after = (!raw["after"].is_null()).then(|| raw["after"].clone());
        let actor = &tx.host().profile.effect_observer;
        let locator = format!("gaap:journal/effect/{}", &self.binding[7..]);
        let bytes = canonical(
            &json!({"kind":"observation","actor_id":actor,"binding":self.binding,"before":self.before,
            "after":after,"disposition":disposition,"covers_through_stop":raw["covers_through_stop"]}),
        )?;
        archive::retain(
            tx,
            &self.binding,
            "effect",
            vec![archive::entry(actor, &locator, &bytes)],
            json!({"observation":raw,"authority":*admission.rename_check.borrow()}),
        )?;
        Ok(EffectEvidence {
            disposition: disposition.into(),
            after,
            source: Some(json!({"actor_id":actor,
            "evidence":{"evidence_type":"artifact","locator":locator,"digest":digest(&bytes)}})),
        })
    }
}

impl Drop for Witness {
    fn drop(&mut self) {
        let _ = self.channel.shutdown(std::net::Shutdown::Both);
        if self.child.try_wait().is_ok_and(|s| s.is_some()) {
            return;
        }
        let _ = self.child.kill();
        let until = Instant::now() + Duration::from_secs(1);
        while Instant::now() < until {
            match self.child.try_wait() {
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                _ => break,
            }
        }
    }
}

use super::read_limit;
use crate::storage::linux::{identity_matches, mount_id, open_directory};
use crate::{Error, Identity, Result, canonical, digest, token};
use rustix::fs::{self, AtFlags, Mode, OFlags, ResolveFlags, inotify};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::fs::File;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

pub(super) fn ambiguous() -> Error {
    Error::unavailable("ambiguous_effect_witness")
}
fn io_error(error: impl Into<io::Error>) -> Error {
    if error.into().raw_os_error() == Some(libc::EIO) {
        Error::unavailable("witness_workspace_eio")
    } else {
        ambiguous()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Start {
    pub workspace: Identity,
    pub mount_id: u64,
    pub binding: String,
    pub attempt_id: String,
    pub proposed: String,
}

#[derive(Serialize)]
struct Event {
    wd: i32,
    mask: u32,
    cookie: u32,
    name: String,
}

pub(super) struct Watch {
    start: Start,
    directory: File,
    inotify: OwnedFd,
    wd: i32,
    before: Value,
    stage: Option<(String, File)>,
    events: Vec<Event>,
    bound_at: usize,
}

impl Watch {
    pub fn arm(start: Start) -> Result<Self> {
        let pin = &start.workspace;
        if !crate::hex(&start.attempt_id, 32)
            || !crate::is_digest(&start.binding)
            || !crate::is_digest(&start.proposed)
            || pin.mode != 0o700
            || pin.uid != rustix::process::geteuid().as_raw()
        {
            return Err(ambiguous());
        }
        let directory = open_directory(Path::new(&pin.path)).map_err(io_error)?;
        let inotify = inotify::init(inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK)
            .map_err(io_error)?;
        use inotify::WatchFlags as W;
        let wd = inotify::add_watch(
            &inotify,
            format!("/proc/self/fd/{}", directory.as_raw_fd()),
            W::CREATE
                | W::MODIFY
                | W::ATTRIB
                | W::CLOSE_WRITE
                | W::MOVE
                | W::DELETE
                | W::MOVE_SELF
                | W::DELETE_SELF
                | W::ONLYDIR,
        )
        .map_err(io_error)?;
        let mut watch = Self {
            start,
            directory,
            inotify,
            wd,
            before: Value::Null,
            stage: None,
            events: Vec::new(),
            bound_at: 0,
        };
        watch.before = watch.snapshot()?;
        watch.drain()?;
        if !watch.events.is_empty() {
            return Err(ambiguous());
        }
        Ok(watch)
    }

    pub fn before(&self) -> &Value {
        &self.before
    }

    fn directory_ok(&self) -> Result<()> {
        let pin = &self.start.workspace;
        let reopened = open_directory(Path::new(&pin.path)).map_err(io_error)?;
        for file in [&self.directory, &reopened] {
            if !identity_matches(&file.metadata().map_err(io_error)?, pin)
                || mount_id(file)? != self.start.mount_id
            {
                return Err(ambiguous());
            }
        }
        Ok(())
    }

    fn open(&self, name: &str) -> Result<File> {
        let fd = fs::openat2(
            &self.directory,
            name,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::NO_XDEV | ResolveFlags::NO_SYMLINKS,
        )
        .map_err(io_error)?;
        let file = File::from(fd);
        let stat = file.metadata().map_err(io_error)?;
        if !stat.is_file()
            || stat.uid() != self.start.workspace.uid
            || stat.dev().to_string() != self.start.workspace.dev
            || stat.nlink() != 1
            || stat.mode() & 0o7000 != 0
        {
            return Err(ambiguous());
        }
        self.same_entry(name, &stat)?;
        Ok(file)
    }

    fn same_entry(&self, name: &str, stat: &std::fs::Metadata) -> Result<()> {
        let s = fs::statat(&self.directory, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io_error)?;
        if s.st_dev != stat.dev()
            || s.st_ino != stat.ino()
            || s.st_mode != stat.mode()
            || s.st_nlink != 1
            || s.st_uid != stat.uid()
        {
            return Err(ambiguous());
        }
        Ok(())
    }

    fn snapshot(&self) -> Result<Value> {
        self.directory_ok()?;
        let mut snapshot = Map::new();
        let mut target = None;
        for name in ["TASK.md", "shipping.py", "deployment.json"] {
            let file = self.open(name)?;
            let stat = file.metadata().map_err(io_error)?;
            let bytes = read_limit(&file, 32768).map_err(io_error)?;
            self.same_entry(name, &stat)?;
            if name == "shipping.py" {
                let pin = Identity {
                    path: Path::new(&self.start.workspace.path)
                        .join(name)
                        .to_str()
                        .ok_or_else(ambiguous)?
                        .into(),
                    dev: stat.dev().to_string(),
                    ino: stat.ino().to_string(),
                    uid: stat.uid(),
                    mode: stat.mode() & 0o7777,
                };
                target = Some((pin, digest(&bytes)));
            }
            snapshot.insert(
                name.into(),
                json!({"content":String::from_utf8(bytes).map_err(|_| ambiguous())?,
                "identity":{"device":stat.dev(),"inode":stat.ino(),"mode":stat.mode()&0o777}}),
            );
        }
        let (pin, hash) = target.ok_or_else(ambiguous)?;
        Ok(
            json!({"file":pin,"artifact_digest":hash,"subject":{"kind":"repository","locator":self.start.workspace.path,
            "digest":digest(&canonical(&snapshot)?)}}),
        )
    }

    fn drain(&mut self) -> Result<()> {
        let mut buf = [MaybeUninit::uninit(); 4096];
        let mut reader = inotify::Reader::new(&self.inotify, &mut buf);
        loop {
            let e = match reader.next() {
                Ok(e) => e,
                Err(rustix::io::Errno::AGAIN) => return Ok(()),
                Err(rustix::io::Errno::INTR) => continue,
                Err(e) => return Err(io_error(e)),
            };
            use inotify::ReadFlags as R;
            if e.wd() != self.wd
                || e.events().intersects(
                    R::QUEUE_OVERFLOW
                        | R::IGNORED
                        | R::UNMOUNT
                        | R::MOVE_SELF
                        | R::DELETE_SELF
                        | R::ISDIR,
                )
                || self.events.len() >= 128
            {
                return Err(ambiguous());
            }
            let name = e
                .file_name()
                .and_then(|n| n.to_str().ok())
                .ok_or_else(ambiguous)?;
            self.events.push(Event {
                wd: e.wd(),
                mask: e.events().bits(),
                cookie: e.cookie(),
                name: name.into(),
            });
        }
    }

    pub fn bind_stage(&mut self, name: String) -> Result<Value> {
        if self.stage.is_some()
            || !token(&name)
            || [".", "..", "shipping.py", "TASK.md", "deployment.json"].contains(&name.as_str())
        {
            return Err(ambiguous());
        }
        self.directory_ok()?;
        let file = self.open(&name)?;
        let stat = file.metadata().map_err(io_error)?;
        if stat.mode() & 0o7777 != 0o600
            || self.before["file"]["ino"]
                .as_str()
                .and_then(|v| v.parse::<u64>().ok())
                == Some(stat.ino())
            || digest(&read_limit(&file, 32768).map_err(io_error)?) != self.start.proposed
        {
            return Err(ambiguous());
        }
        self.drain()?;
        use inotify::ReadFlags as R;
        let allowed = (R::CREATE | R::MODIFY | R::CLOSE_WRITE | R::ATTRIB).bits();
        if self
            .events
            .iter()
            .any(|e| e.name != name || e.mask & !allowed != 0)
            || self
                .events
                .iter()
                .filter(|e| e.mask == R::CREATE.bits())
                .count()
                != 1
        {
            return Err(ambiguous());
        }
        let retained = json!({"name":name, "dev":stat.dev().to_string(), "ino":stat.ino().to_string(),
            "uid":stat.uid(), "mode":stat.mode() & 0o7777, "artifact_digest":self.start.proposed,
            "wd":self.wd, "events":self.events});
        self.bound_at = self.events.len();
        self.stage = Some((name, file));
        Ok(retained)
    }

    pub fn finish(mut self) -> Result<Value> {
        let after = self.snapshot()?;
        self.drain()?;
        let (name, file) = self.stage.as_ref().ok_or_else(ambiguous)?;
        let stat = file.metadata().map_err(io_error)?;
        let commits = &self.events[self.bound_at..];
        use inotify::ReadFlags as R;
        let replaced = matches!(commits,[from,to] if from.name == *name && from.mask==R::MOVED_FROM.bits()
            && to.name=="shipping.py" && to.mask == R::MOVED_TO.bits() && from.cookie!=0 && from.cookie==to.cookie)
            && stat.nlink() == 1
            && after["file"]["ino"]
                .as_str()
                .and_then(|v| v.parse::<u64>().ok())
                == Some(stat.ino())
            && after["artifact_digest"] == self.start.proposed;
        let stopped = matches!(commits,[deleted] if deleted.name == *name && deleted.mask==R::DELETE.bits())
            && stat.nlink() == 0
            && self.before == after;
        let disposition = if replaced {
            "replaced"
        } else if stopped {
            "stopped_before_rename"
        } else {
            "unresolved"
        };
        Ok(
            json!({"disposition":disposition,"after":replaced.then_some(after),"covers_through_stop":stopped,
            "events":self.events,"wd":self.wd,"bound_at":self.bound_at,"workspace":self.start.workspace,
            "attempt_id":self.start.attempt_id,"binding":self.start.binding,
            "stage":{"name":name,"dev":stat.dev().to_string(),"ino":stat.ino().to_string(),"nlink":stat.nlink()}}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Workspace(PathBuf);

    impl Workspace {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "gaap-watch-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .unwrap();
            let workspace = Self(path);
            for name in ["TASK.md", "shipping.py", "deployment.json"] {
                workspace.write(name, b"original");
            }
            workspace
        }

        fn write(&self, name: &str, bytes: &[u8]) {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(self.0.join(name))
                .unwrap();
            file.write_all(bytes).unwrap();
            file.sync_all().unwrap();
        }

        fn watch(&self) -> Watch {
            let dir = File::open(&self.0).unwrap();
            let stat = dir.metadata().unwrap();
            Watch::arm(Start {
                workspace: Identity {
                    path: self.0.to_str().unwrap().into(),
                    dev: stat.dev().to_string(),
                    ino: stat.ino().to_string(),
                    uid: stat.uid(),
                    mode: stat.mode() & 0o7777,
                },
                mount_id: mount_id(&dir).unwrap(),
                binding: digest(b"attempt"),
                attempt_id: "ab".repeat(16),
                proposed: digest(b"replacement"),
            })
            .unwrap()
        }
    }

    impl Drop for Workspace {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn replacement_by_another_stage_with_matching_bytes_stays_unresolved() {
        let workspace = Workspace::new();
        let mut watch = workspace.watch();
        workspace.write("stage", b"replacement");
        watch.bind_stage("stage".into()).unwrap();
        workspace.write("other", b"replacement");
        std::fs::rename(workspace.0.join("other"), workspace.0.join("shipping.py")).unwrap();
        let result = watch.finish().unwrap();
        assert_eq!(result["disposition"], "unresolved");
        assert!(result["after"].is_null());
    }

    #[test]
    fn unchanged_target_and_a_live_link_to_the_stage_cant_prove_termination() {
        let workspace = Workspace::new();
        let mut watch = workspace.watch();
        workspace.write("stage", b"replacement");
        watch.bind_stage("stage".into()).unwrap();
        std::fs::hard_link(workspace.0.join("stage"), workspace.0.join("other")).unwrap();
        std::fs::remove_file(workspace.0.join("stage")).unwrap();
        let result = watch.finish().unwrap();
        assert_eq!(result["disposition"], "unresolved");
        assert_eq!(result["covers_through_stop"], false);
        assert_eq!(result["stage"]["nlink"], 1);
    }
}

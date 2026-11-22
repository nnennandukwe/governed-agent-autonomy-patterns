//! Reconstruct the effect from the enrolled observer's source records.
use super::profile_digest;
use crate::{Error, Host, Identity, MAX_SAFE, Result, canonical, digest};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug)]
pub(crate) struct Finding {
    pub observer_id: String,
    pub disposition: String,
    pub reason: String,
    pub after: Option<Value>,
}
pub(super) fn insufficient(reason: &'static str) -> Error {
    Error::new("ineligible_evidence", reason)
}

// Missing bytes can't stand in for an observed null (especially a withheld entry permit).
fn nullable<'de, D, T>(d: D) -> std::result::Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(d)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Candidate {
    format: String,
    workspace_id: String,
    store_id: String,
    profile_digest: String,
    reconciliation_profile_digest: String,
    observer_id: String,
    attempt_id: String,
    binding: String,
    workspace: Identity,
    before: Snapshot,
    after: Snapshot,
    watch: Watch,
    #[serde(deserialize_with = "nullable")]
    gate: Option<Gate>,
    stop: Stop,
    #[serde(deserialize_with = "nullable")]
    stage: Option<Stage>,
}
#[derive(Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct FileIdentity {
    device: u64,
    inode: u64,
    mode: u32,
}
#[derive(Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct SnapshotFile {
    content: String,
    identity: FileIdentity,
}
#[derive(Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    files: BTreeMap<String, SnapshotFile>,
    target: Identity,
}
impl Snapshot {
    fn observation(&self, root: &Identity) -> Result<Value> {
        let names = ["TASK.md", "deployment.json", "shipping.py"];
        if self.files.len() != names.len()
            || !names.iter().all(|n| self.files.contains_key(*n))
            || self.files.values().any(|f| {
                f.content.len() > 32768
                    || f.identity.inode == 0
                    || f.identity.inode > MAX_SAFE
                    || f.identity.device > MAX_SAFE
                    || f.identity.device.to_string() != root.dev
                    || f.identity.mode > 0o777
            })
        {
            return Err(insufficient("incomplete_observer_snapshot"));
        }
        let target = &self.files["shipping.py"];
        if self.target.dev != root.dev
            || self.target.ino != target.identity.inode.to_string()
            || self.target.mode != target.identity.mode
            || self.target.uid != root.uid
            || Some(self.target.path.as_str()) != Path::new(&root.path).join("shipping.py").to_str()
        {
            return Err(insufficient("snapshot_target_mismatch"));
        }
        Ok(
            json!({"subject":{"kind":"repository","locator":root.path,"digest":digest(&canonical(&self.files)?)},
            "file":self.target,"artifact_digest":digest(target.content.as_bytes())}),
        )
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Watch {
    wd: i32,
    armed_ns: String,
    events: Vec<Event>,
    bound_at: usize,
    drained_ns: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Event {
    wd: i32,
    mask: u32,
    cookie: u32,
    name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Gate {
    request: String,
    #[serde(deserialize_with = "nullable")]
    reply: Option<String>,
    sender_pid: i32,
    sender_uid: u32,
    received_ns: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stop {
    pid: i32,
    boot_id: String,
    start_ticks: String,
    launched_ns: String,
    stopped_ns: String,
    pidfd_revents: i16,
    wait_code: i32,
    wait_status: i32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stage {
    name: String,
    dev: String,
    ino: String,
    uid: u32,
    mode: u32,
    content: String,
    final_nlink: u64,
}

fn number(s: &str) -> Result<u64> {
    s.parse::<u64>()
        .ok()
        .filter(|n| n.to_string() == s)
        .ok_or_else(|| insufficient("invalid_kernel_counter"))
}
fn boot_id(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == b'-'
            } else {
                c.is_ascii_digit() || (b'a'..=b'f').contains(&c)
            }
        })
}

impl Candidate {
    fn binding<'a>(&self, host: &Host, attempt: &'a Value) -> Result<&'a Value> {
        let slot = &host.slot;
        let facts = attempt["facts"]
            .as_array()
            .ok_or_else(|| Error::invalid("original_facts_missing"))?;
        let dispatch = facts
            .iter()
            .find(|f| f["tag"] == "dispatch")
            .ok_or_else(|| insufficient("original_attempt_never_dispatched"))?;
        let proposed = &attempt["proposal"];
        let proposal_digest = digest(&canonical(proposed)?);
        // The history validator has recomputed both digests from the original typed
        // requests, including their historical serialization defaults.
        let intent = &facts[0]["payload"];
        let computed = digest(&canonical(&json!([
            slot.workspace_id,
            slot.store_id,
            slot.profile_digest,
            attempt["principal"],
            attempt["key"],
            attempt["attempt_id"],
            proposal_digest,
            intent["run_request_digest"],
            intent["effect_request_digest"]
        ]))?);
        if self.format != "gaap.shipping-observer-candidate/0.2"
            || self.workspace_id != slot.workspace_id
            || self.store_id != slot.store_id
            || self.profile_digest != slot.profile_digest
            || self.reconciliation_profile_digest != profile_digest(host)?
            || self.workspace != slot.workspace
            || self.observer_id != host.profile.effect_observer
            || self.binding != computed
            || attempt["attempt_id"] != self.attempt_id
            || attempt["proposal_digest"] != proposal_digest
            || dispatch["binding"] != computed
            || proposed["path"] != "shipping.py"
        {
            return Err(insufficient("observer_binding_mismatch"));
        }
        Ok(&dispatch["payload"]["before"])
    }

    fn coverage(&self, host: &Host) -> Result<()> {
        let s = &self.stop;
        let launched = number(&s.launched_ns)?;
        let stopped = number(&s.stopped_ns)?;
        if self.watch.wd <= 0
            || self.watch.events.len() > 128
            || self.watch.bound_at > self.watch.events.len()
            || self.watch.events.iter().any(|e| e.wd != self.watch.wd)
            || !boot_id(&s.boot_id)
            || number(&s.start_ticks)? == 0
            || s.pid <= 0
            || s.pidfd_revents & libc::POLLIN == 0
            || s.pidfd_revents & !(libc::POLLIN | libc::POLLHUP) != 0
            || !match s.wait_code {
                libc::CLD_EXITED => (0..=255).contains(&s.wait_status),
                libc::CLD_KILLED | libc::CLD_DUMPED => (1..=64).contains(&s.wait_status),
                _ => false,
            }
            || number(&self.watch.armed_ns)? > launched
            || launched > stopped
            || stopped > number(&self.watch.drained_ns)?
        {
            return Err(insufficient("incomplete_observer_interval"));
        }
        if let Some(gate) = &self.gate {
            let received = number(&gate.received_ns)?;
            let permit = format!("permit {} {}", self.attempt_id, self.binding);
            if gate.sender_pid != s.pid
                || gate.sender_uid != host.slot.workspace.uid
                || received < launched
                || received > stopped
                || gate.request != format!("enter {} {}", self.attempt_id, self.binding)
                || gate.reply.as_ref().is_some_and(|reply| reply != &permit)
            {
                return Err(insufficient("observer_entry_mismatch"));
            }
        }
        Ok(())
    }
    fn stage_setup(&self, stage: &Stage, proposed: &str) -> Result<()> {
        let prefix = &self.watch.events[..self.watch.bound_at];
        let stage_inode = number(&stage.ino)?;
        if stage.name != format!(".gaap-stage-target-{}", self.attempt_id)
            || stage.dev != self.workspace.dev
            || stage.uid != self.workspace.uid
            || stage.mode != 0o600
            || stage.content != proposed
            || stage_inode == 0
            || stage.ino == self.before.target.ino
            || prefix.first().is_none_or(|e| e.mask != libc::IN_CREATE)
            || prefix.iter().filter(|e| e.mask == libc::IN_CREATE).count() != 1
            || prefix.iter().any(|e| {
                e.name != stage.name
                    || e.cookie != 0
                    || !matches!(
                        e.mask,
                        libc::IN_CREATE | libc::IN_MODIFY | libc::IN_ATTRIB | libc::IN_CLOSE_WRITE
                    )
            })
        {
            return Err(insufficient("observer_stage_mismatch"));
        }
        Ok(())
    }
}

pub(super) fn assess(host: &Host, attempt: &Value, raw: Value) -> Result<Finding> {
    let c: Candidate =
        serde_json::from_value(raw).map_err(|_| Error::invalid("malformed_observer_deposit"))?;
    let dispatched = c.binding(host, attempt)?;
    c.coverage(host)?;
    if c.before.observation(&c.workspace)? != *dispatched {
        return Err(insufficient("observer_before_mismatch"));
    }
    let after = c.after.observation(&c.workspace)?;
    let withheld = c.gate.as_ref().is_some_and(|g| g.reply.is_none());
    let (disposition, reason, result) = if let Some(stage) = &c.stage {
        if withheld {
            return Err(insufficient("observed_stage_without_entry"));
        }
        let content = attempt["proposal"]["content"]
            .as_str()
            .ok_or_else(|| Error::invalid("missing_original_content"))?;
        c.stage_setup(stage, content)?;
        if ["TASK.md", "deployment.json"]
            .iter()
            .any(|name| c.before.files[*name] != c.after.files[*name])
        {
            return Err(insufficient("unrelated_subject_change"));
        }
        match &c.watch.events[c.watch.bound_at..] {
            [from, to]
                if from.name == stage.name
                    && to.name == "shipping.py"
                    && from.mask == libc::IN_MOVED_FROM
                    && to.mask == libc::IN_MOVED_TO
                    && from.cookie != 0
                    && from.cookie == to.cookie
                    && stage.final_nlink == 1
                    && c.after.target.ino == stage.ino
                    && c.after.target.mode == stage.mode
                    && c.after.files["shipping.py"].content == stage.content =>
            {
                ("replaced", "observed_replacement", Some(after))
            }
            [delete]
                if delete.mask == libc::IN_DELETE
                    && delete.name == stage.name
                    && delete.cookie == 0
                    && stage.final_nlink == 0
                    && c.before == c.after =>
            {
                ("stopped_before_rename", "failed_before_rename", None)
            }
            _ => return Err(insufficient("ambiguous_observer_stage_outcome")),
        }
    } else if withheld && c.watch.events.is_empty() && c.watch.bound_at == 0 && c.before == c.after
    {
        ("not_started", "never_entered_before_stop", None)
    } else {
        return Err(insufficient("missing_end_to_end_effect_evidence"));
    };
    Ok(Finding {
        observer_id: c.observer_id,
        disposition: disposition.into(),
        reason: reason.into(),
        after: result,
    })
}

#[cfg(test)]
mod tests {
    use super::Candidate;
    use serde_json::{Value, json};

    #[test]
    fn missing_source_fields_cant_stand_in_for_observed_nulls() {
        let identity = json!({"path":"/workspace","dev":"1","ino":"2","uid":1000,"mode":448});
        let snapshot = json!({"files":{},"target":identity});
        let record = json!({
            "format":"gaap.shipping-observer-candidate/0.2", "workspace_id":"w","store_id":"s",
            "profile_digest":"p","reconciliation_profile_digest":"q","observer_id":"o",
            "attempt_id":"a","binding":"b","workspace":identity,"before":snapshot,"after":snapshot,
            "watch":{"wd":1,"armed_ns":"1","events":[],"bound_at":0,"drained_ns":"5"},
            "gate":{"request":"enter a b","reply":null,"sender_pid":34,"sender_uid":1000,"received_ns":"3"},
            "stop":{"pid":34,"boot_id":"boot","start_ticks":"245","launched_ns":"2","stopped_ns":"4",
                    "pidfd_revents":1,"wait_code":2,"wait_status":9},"stage":null
        });
        assert!(serde_json::from_value::<Candidate>(record.clone()).is_ok());
        for (object, member) in [("", "gate"), ("", "stage"), ("/gate", "reply")] {
            let mut incomplete = record.clone();
            incomplete
                .pointer_mut(object)
                .and_then(Value::as_object_mut)
                .unwrap()
                .remove(member)
                .unwrap();
            assert!(
                serde_json::from_value::<Candidate>(incomplete).is_err(),
                "{object}/{member}"
            );
        }
    }
}

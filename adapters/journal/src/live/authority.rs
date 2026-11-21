use super::{archive, binding, read_limit};
use crate::storage::linux::{bounded_flock, identity_matches, open_directory};
use crate::writer::Submission;
use crate::{Error, Host, MAX_SAFE, Result, canonical, digest, is_digest, json_document};
use gaap::contracts::*;
use rustix::fs::{self, AtFlags, Mode, OFlags, ResolveFlags};
use serde::Deserialize;
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::fs::File;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Authority {
    format: String,
    revision: u64,
    workspace_id: String,
    store_id: String,
    profile_digest: String,
    principal: String,
    key: String,
    proposal_digest: String,
    target: String,
    operation: String,
    can_execute: bool,
    allow_retention: bool,
    revoked: bool,
    cancelled: bool,
    valid_from_ms: u64,
    expires_at_ms: u64,
    budget: ResourceBudget,
    approval: ApprovalReference,
    approval_bytes_hex: String,
}

fn deny() -> Error {
    Error::new("forbidden", "current_shipping_authority_denied")
}
fn source_error() -> Error {
    Error::unavailable("live_authority_source_unavailable")
}
fn now_ms() -> Result<u64> {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| source_error())?
        .as_millis();
    u64::try_from(ms)
        .ok()
        .filter(|n| *n <= MAX_SAFE)
        .ok_or_else(source_error)
}
fn regular(dir: &File, name: &str, uid: u32) -> Result<File> {
    let fd = fs::openat2(
        dir,
        name,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_XDEV | ResolveFlags::NO_SYMLINKS,
    )
    .map_err(|_| source_error())?;
    let file = File::from(fd);
    let stat = file.metadata().map_err(|_| source_error())?;
    if !stat.is_file() || stat.nlink() != 1 || stat.uid() != uid || stat.mode() & 0o7777 != 0o600 {
        return Err(source_error());
    }
    Ok(file)
}

struct LockedAuthority {
    _lock: File,
    record: Authority,
    bytes: Vec<u8>,
}

impl LockedAuthority {
    fn open(host: &Host) -> Result<Self> {
        let live = binding(host)?;
        if !live.can_submit {
            return Err(deny());
        }
        let pin = &live.authority;
        let lock_pin = &live.authority_lock;
        if pin.uid != host.slot.workspace.uid
            || pin.dev == host.slot.workspace.dev
            || pin.mode != 0o700
            || lock_pin.uid != pin.uid
            || lock_pin.dev != pin.dev
            || lock_pin.mode != 0o600
            || Path::new(&pin.path).join("authority.lock") != Path::new(&lock_pin.path)
        {
            return Err(source_error());
        }
        let dir = open_directory(Path::new(&pin.path)).map_err(|_| source_error())?;
        let check_directory = || -> Result<()> {
            let reopened = open_directory(Path::new(&pin.path)).map_err(|_| source_error())?;
            if !identity_matches(&reopened.metadata().map_err(|_| source_error())?, pin)
                || !identity_matches(&dir.metadata().map_err(|_| source_error())?, pin)
            {
                return Err(source_error());
            }
            Ok(())
        };
        check_directory()?;
        let lock = regular(&dir, "authority.lock", pin.uid)?;
        let stat = lock.metadata().map_err(|_| source_error())?;
        if !identity_matches(&stat, lock_pin) || stat.len() != 0 {
            return Err(source_error());
        }
        bounded_flock(&lock, false).map_err(|e| {
            if e == rustix::io::Errno::AGAIN {
                Error::new("busy", "live_authority_lock_busy")
            } else {
                source_error()
            }
        })?;
        check_directory()?;
        let lock_entry = fs::statat(&dir, "authority.lock", AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|_| source_error())?;
        if lock_entry.st_ino != stat.ino()
            || lock_entry.st_dev != stat.dev()
            || lock_entry.st_nlink != 1
        {
            return Err(source_error());
        }
        let file = regular(&dir, "authority.json", pin.uid)?;
        let bytes = read_limit(&file, 64 * 1024).map_err(|_| source_error())?;
        let record =
            serde_json::from_value(json_document(&bytes, true)?).map_err(|_| source_error())?;
        Ok(Self {
            _lock: lock,
            record,
            bytes,
        })
    }
}

pub struct Admission {
    pub approval: ApprovalReference,
    pub budget: ResourceBudget,
    subject: Subject,
    proposal_digest: String,
    source_digest: String,
    start: Instant,
    start_ms: u64,
    dispatch: RefCell<Option<(String, String, EffectUsage)>>,
    used_permit: Cell<bool>,
    completion_checked: Cell<bool>,
    pub(super) rename_check: RefCell<Option<Value>>,
}

pub fn admit(
    host: &Host,
    key: &str,
    proposal_digest: &str,
    subject: &Subject,
) -> Result<Admission> {
    let source = LockedAuthority::open(host)?;
    let a = &source.record;
    let now = now_ms()?;
    let approval = &a.approval;
    if a.format != "gaap.shipping-authority/0.1"
        || a.revision == 0
        || a.workspace_id != host.slot.workspace_id
        || a.store_id != host.slot.store_id
        || a.profile_digest != host.slot.profile_digest
        || a.principal != host.principal
        || a.key != key
        || !is_digest(proposal_digest)
        || a.proposal_digest != proposal_digest
        || a.target != "shipping.py"
        || a.operation != "filesystem.write"
        || !a.can_execute
        || !a.allow_retention
        || a.revoked
        || a.cancelled
        || now < a.valid_from_ms
        || now >= a.expires_at_ms
        || subject.kind != SubjectKind::Repository
        || subject.locator != host.slot.workspace.path
        || !is_digest(&subject.digest)
        || approval.subject_digest != subject.digest
        || approval.scope != "filesystem.write:shipping.py"
        || approval.approval_id.trim().is_empty()
        || approval.actor_id.trim().is_empty()
        || approval.evidence.evidence_type != EvidenceType::Approval
        || approval
            .evidence
            .locator
            .as_deref()
            .is_none_or(|s| s.trim().is_empty())
        || a.approval_bytes_hex.is_empty()
        || archive::decode_hex(&a.approval_bytes_hex)
            .is_none_or(|b| digest(&b) != approval.evidence.digest)
    {
        return Err(deny());
    }
    Ok(Admission {
        approval: approval.clone(),
        budget: a.budget.clone(),
        subject: subject.clone(),
        proposal_digest: proposal_digest.to_owned(),
        source_digest: digest(&source.bytes),
        start: Instant::now(),
        start_ms: now,
        dispatch: RefCell::new(None),
        used_permit: Cell::new(false),
        completion_checked: Cell::new(false),
        rename_check: RefCell::new(None),
    })
}

pub struct GrantEvidence {
    pub checked_at_ms: u64,
    pub checks: Vec<Value>,
}

impl Admission {
    pub(super) fn retire(&self) {
        self.used_permit.set(true);
    }

    /// The engine calls its executor's one-shot completion claim after verification.
    /// A cooperating source update either precedes this decision or takes effect later.
    pub fn check_completion(&self, host: &Host, usage: &ResourceUsage) -> Result<()> {
        if self.completion_checked.replace(true) || self.rename_check.borrow().is_none() {
            return Err(deny());
        }
        let source = LockedAuthority::open(host)?;
        let remaining = EffectUsage {
            cost_micros: 0,
            model_tokens: 0,
            elapsed_ms: 0,
            tool_calls: 0,
        };
        self.check_now(&source, usage, &remaining)?;
        Ok(())
    }

    fn verify_request(
        &self,
        host: &Host,
        run: &AgentRunRequest,
        effect: &ProtectedEffectRequest,
    ) -> Result<()> {
        let p = &host.profile;
        let scope = RequestedScope::Filesystem {
            root: self.subject.locator.clone(),
            access: vec![FilesystemAccess::Read, FilesystemAccess::Modify],
            recursive: false,
        };
        if run.subject != self.subject
            || effect.subject != self.subject
            || effect.input_digest != self.proposal_digest
            || run.approval_context.as_slice() != std::slice::from_ref(&self.approval)
            || !effect.approval_context.is_empty()
            || run.resource_budget != self.budget
            || run.requested_capability != p.capability
            || run.policies != p.policies
            || effect.policies != p.policies
            || effect.capability != p.capability
            || effect.sandbox_profile != p.sandbox
            || effect.tool_schema_digest.as_ref() != Some(&p.tool_schema_digest)
            || effect.operation_family != OperationFamily::Filesystem
            || effect.normalized_operation != "filesystem.write"
            || !matches!(&effect.input_metadata[..], [m] if m.name == "path" && m.value == "shipping.py")
            || effect.requested_scopes.as_slice() != [scope]
            || effect.effect_sequence != 1
            || effect.agent_run_request_digest != digest(&canonical(run)?)
            || effect.run_id != run.run_id
        {
            return Err(deny());
        }
        Ok(())
    }

    fn check_now(
        &self,
        source: &LockedAuthority,
        used: &ResourceUsage,
        reserved: &EffectUsage,
    ) -> Result<u64> {
        let elapsed = u64::try_from(self.start.elapsed().as_millis()).map_err(|_| deny())?;
        let now = now_ms()?.max(self.start_ms.checked_add(elapsed).ok_or_else(deny)?);
        let a = &source.record;
        if digest(&source.bytes) != self.source_digest
            || a.approval != self.approval
            || a.budget != self.budget
            || now < a.valid_from_ms
            || now >= a.expires_at_ms
            || [
                (
                    used.cost_micros,
                    reserved.cost_micros,
                    self.budget.max_cost_micros,
                ),
                (
                    used.model_tokens,
                    reserved.model_tokens,
                    self.budget.max_model_tokens,
                ),
                (
                    used.tool_calls,
                    reserved.tool_calls,
                    self.budget.max_tool_calls,
                ),
                (
                    used.elapsed_ms.max(elapsed),
                    reserved.elapsed_ms,
                    self.budget.max_elapsed_ms,
                ),
            ]
            .iter()
            .any(|(u, r, max)| u.checked_add(*r).is_none_or(|n| n > *max))
        {
            return Err(deny());
        }
        Ok(now)
    }

    pub fn dispatch_check(
        &self,
        tx: &Submission<'_>,
        binding: &str,
        run: &AgentRunRequest,
        effect: &ProtectedEffectRequest,
        usage: &ResourceUsage,
        reserved: &EffectUsage,
    ) -> Result<GrantEvidence> {
        if self.dispatch.borrow().is_some() || !is_digest(binding) {
            return Err(deny());
        }
        self.verify_request(tx.host(), run, effect)?;
        let source = LockedAuthority::open(tx.host())?;
        let time = self.check_now(&source, usage, reserved)?;
        let actor = &tx.host().profile.grant_observer;
        let locator = format!("gaap:journal/grant/{}", &binding[7..]);
        let bytes = canonical(
            &json!({"kind":"grant", "actor_id":actor, "binding":binding, "checked_at_ms":time,
            "approval":self.approval, "valid":true}),
        )?;
        let check = json!({"approval":self.approval,"source":{"actor_id":actor,"evidence":{
            "evidence_type":"approval","locator":locator,"digest":digest(&bytes)}}});
        let raw = json_document(&source.bytes, true)?;
        drop(source);
        archive::retain(
            tx,
            binding,
            "grant",
            vec![archive::entry(actor, &locator, &bytes)],
            raw,
        )?;
        *self.dispatch.borrow_mut() = Some((
            binding.to_owned(),
            digest(&canonical(effect)?),
            reserved.clone(),
        ));
        Ok(GrantEvidence {
            checked_at_ms: time,
            checks: vec![check],
        })
    }

    pub fn with_rename_permit<T>(
        &self,
        host: &Host,
        run: &AgentRunRequest,
        effect: &ProtectedEffectRequest,
        reserved: &EffectUsage,
        reobserve: impl FnOnce() -> Result<(Subject, ResourceUsage)>,
        rename_once: impl FnOnce() -> T,
    ) -> Result<T> {
        if self.used_permit.replace(true) {
            return Err(deny());
        }
        self.verify_request(host, run, effect)?;
        let effect_digest = digest(&canonical(effect)?);
        if self
            .dispatch
            .borrow()
            .as_ref()
            .is_none_or(|(_, e, r)| e != &effect_digest || r != reserved)
        {
            return Err(deny());
        }
        let source = LockedAuthority::open(host)?;
        let (subject, usage) = reobserve()?;
        if subject != self.subject {
            return Err(deny());
        }
        let now = self.check_now(&source, &usage, reserved)?;
        let result = rename_once();
        drop(source._lock);
        *self.rename_check.borrow_mut() = Some(
            json!({"checked_at_ms":now,"revision":source.record.revision,
            "authority_digest":self.source_digest,"subject_digest":subject.digest,"usage":usage,"reserved":reserved}),
        );
        Ok(result)
    }
}

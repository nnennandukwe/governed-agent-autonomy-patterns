use gaap::{Decision, Gate, Outcome, RunCoordinator};
use rustix::fs::{Mode, OFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub type Result<T> = std::result::Result<T, String>;
const INITIAL: &str = "{\n  \"name\": \"conference-demo\",\n  \"version\": \"1.0.0\"\n}\n";
const MAX_CONTENT: usize = 32 * 1024;
const MAX_PROPOSALS: usize = 64;
const MAX_RECORDS: usize = 512;
const POLICY: &str =
    "gaap-goose-demo/v1;release.json-only;exact-operator-approval;once;max-content=32768";

pub fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
fn canonical<T: Serialize>(value: &T) -> Result<String> {
    serde_jcs::to_vec(value)
        .map(|v| digest(&v))
        .map_err(|e| e.to_string())
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn block(code: &str) -> Decision {
    Decision {
        outcome: Outcome::Block,
        code: code.into(),
        effects: vec!["stop_action".into()],
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Identity {
    device: u64,
    inode: u64,
    mode: u32,
}
fn identity(path: &Path, directory: bool) -> Result<Identity> {
    let m = fs::symlink_metadata(path).map_err(err)?;
    if m.file_type().is_symlink()
        || if directory {
            !m.is_dir()
        } else {
            !m.is_file() || m.nlink() != 1
        }
    {
        return Err("symlinks, hardlinks and unexpected file types are not allowed".into());
    }
    Ok(Identity {
        device: m.dev(),
        inode: m.ino(),
        mode: m.mode() & 0o777,
    })
}
fn directory(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for part in path.components() {
        current.push(part);
        identity(&current, true)?;
    }
    Ok(())
}
fn trusted_parent(path: &Path) -> Result<()> {
    directory(path)?;
    let mut current = PathBuf::new();
    let owner = rustix::process::geteuid().as_raw();
    for part in path.components() {
        current.push(part);
        let metadata = fs::symlink_metadata(&current).map_err(err)?;
        if (metadata.uid() != owner && metadata.uid() != 0)
            || (metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0)
        {
            return Err("session parents must be owned by you or root, and shared writable parents must be sticky; use a private directory or /tmp".into());
        }
    }
    Ok(())
}
fn open_regular(path: &Path, write: bool) -> Result<File> {
    identity(path, false)?;
    let flags = if write { OFlags::RDWR } else { OFlags::RDONLY };
    let fd = rustix::fs::open(
        path,
        flags | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(err)?;
    let file = File::from(fd);
    let m = file.metadata().map_err(err)?;
    if !m.is_file()
        || m.nlink() != 1
        || identity(path, false)?
            != (Identity {
                device: m.dev(),
                inode: m.ino(),
                mode: m.mode() & 0o777,
            })
    {
        return Err("file identity changed while opening".into());
    }
    Ok(file)
}
fn read(path: &Path, limit: usize) -> Result<(String, Identity)> {
    let f = open_regular(path, false)?;
    let m = f.metadata().map_err(err)?;
    let mut content = String::new();
    f.take((limit + 1) as u64)
        .read_to_string(&mut content)
        .map_err(err)?;
    if content.len() > limit {
        return Err("file exceeds size limit".into());
    }
    Ok((
        content,
        Identity {
            device: m.dev(),
            inode: m.ino(),
            mode: m.mode() & 0o777,
        },
    ))
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FileView {
    pub note: Option<String>,
    pub path: String,
    pub content: String,
    pub digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Proposal {
    session: String,
    path: String,
    before: Option<String>,
    before_identity: Option<Identity>,
    content: String,
    capability_digest: String,
    policy_digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    proposal: Proposal,
    approved: bool,
    execution_status: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    schema_version: String,
    session: String,
    workspace: PathBuf,
    workspace_identity: Identity,
    operator_identity: Identity,
    capability_digest: String,
    policy_digest: String,
    proposals: BTreeMap<String, Entry>,
    records: Vec<Value>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Reply {
    pub request_id: String,
    pub decision: Decision,
    pub execution_status: String,
    pub observed_post_digest: Option<String>,
    pub note: Option<String>,
}
impl Reply {
    pub fn code(&self) -> &str {
        &self.decision.code
    }
}

#[derive(Clone)]
pub struct Store {
    state_dir: PathBuf,
    workspace: PathBuf,
    #[cfg(test)]
    fault: Option<&'static str>,
}
impl Store {
    pub fn initialize(root: &Path) -> Result<Self> {
        let parent = root
            .parent()
            .ok_or("session requires parent directory")?
            .canonicalize()
            .map_err(err)?;
        trusted_parent(&parent)?;
        let root = parent.join(root.file_name().ok_or("session requires a name")?);
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .map_err(err)?;
        let workspace = root.join("workspace");
        let state_dir = root.join("operator");
        fs::create_dir(&workspace).map_err(err)?;
        fs::create_dir(&state_dir).map_err(err)?;
        fs::set_permissions(&state_dir, fs::Permissions::from_mode(0o700)).map_err(err)?;
        let store = Self {
            state_dir,
            workspace,
            #[cfg(test)]
            fault: None,
        };
        File::create_new(store.state_dir.join("lock"))
            .map_err(err)?
            .sync_all()
            .map_err(err)?;
        let mut fixture = File::create_new(store.workspace.join("release.json")).map_err(err)?;
        fixture.write_all(INITIAL.as_bytes()).map_err(err)?;
        fixture.sync_all().map_err(err)?;
        File::open(&store.workspace)
            .map_err(err)?
            .sync_all()
            .map_err(err)?;
        let state = State {
            schema_version: "gaap-goose-decision-log/v1".into(),
            session: digest(
                format!(
                    "{}:{}",
                    root.display(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(err)?
                        .as_nanos()
                )
                .as_bytes(),
            ),
            workspace: store.workspace.clone(),
            workspace_identity: identity(&store.workspace, true)?,
            operator_identity: identity(&store.state_dir, true)?,
            capability_digest: crate::protocol::capability_digest(),
            policy_digest: digest(POLICY.as_bytes()),
            proposals: BTreeMap::new(),
            records: vec![],
        };
        if let Some(warning) = store.save(&state, "initialize")? {
            eprintln!("{warning}");
        }
        File::open(root).map_err(err)?.sync_all().map_err(err)?;
        Ok(store)
    }
    pub fn open(state_dir: &Path) -> Result<Self> {
        directory(state_dir)?;
        let state_dir = state_dir.canonicalize().map_err(err)?;
        let workspace = state_dir
            .parent()
            .ok_or("missing session root")?
            .join("workspace");
        let store = Self {
            state_dir,
            workspace,
            #[cfg(test)]
            fault: None,
        };
        let _lock = store.lock()?;
        store.load()?;
        Ok(store)
    }
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }
    fn hit(&self, point: &str) -> Result<()> {
        #[cfg(test)]
        if self
            .fault
            .is_some_and(|faults| faults.split(",").any(|fault| fault == point))
        {
            return Err(format!("injected failure at {point}"));
        }
        let _ = point;
        Ok(())
    }
    fn lock(&self) -> Result<File> {
        directory(&self.state_dir)?;
        let f = open_regular(&self.state_dir.join("lock"), true)?;
        f.lock().map_err(err)?;
        Ok(f)
    }
    fn load(&self) -> Result<State> {
        self.hit("state_read")?;
        let (text, _) = read(&self.state_dir.join("state.json"), 8 * 1024 * 1024)?;
        let state: State = serde_json::from_str(&text).map_err(err)?;
        self.validate(&state)?;
        Ok(state)
    }
    fn validate(&self, state: &State) -> Result<()> {
        directory(&self.workspace)?;
        directory(&self.state_dir)?;
        if state.schema_version != "gaap-goose-decision-log/v1"
            || state.workspace != self.workspace
            || state.workspace_identity != identity(&self.workspace, true)?
            || state.operator_identity != identity(&self.state_dir, true)?
            || state.capability_digest != crate::protocol::capability_digest()
            || state.policy_digest != digest(POLICY.as_bytes())
        {
            return Err(
                "session identity, policy or capability changed; create a fresh session".into(),
            );
        }
        for (id, entry) in &state.proposals {
            if canonical(&entry.proposal)? != *id
                || entry.proposal.session != state.session
                || entry.proposal.policy_digest != state.policy_digest
                || entry.proposal.capability_digest != state.capability_digest
                || ![
                    "pending",
                    "executing",
                    "executed",
                    "aborted",
                    "unknown_outcome",
                ]
                .contains(&entry.execution_status.as_str())
            {
                return Err("invalid or altered immutable proposal".into());
            }
        }
        Ok(())
    }
    fn save(&self, state: &State, phase: &str) -> Result<Option<String>> {
        self.hit(&format!("{phase}_state_create"))?;
        let mut temp = tempfile::NamedTempFile::new_in(&self.state_dir).map_err(err)?;
        self.hit(&format!("{phase}_state_write"))?;
        temp.write_all(serde_json::to_string_pretty(state).map_err(err)?.as_bytes())
            .map_err(err)?;
        self.hit(&format!("{phase}_state_flush"))?;
        temp.flush().map_err(err)?;
        self.hit(&format!("{phase}_state_sync"))?;
        temp.as_file().sync_all().map_err(err)?;
        self.hit(&format!("{phase}_state_rename"))?;
        temp.persist(self.state_dir.join("state.json"))
            .map_err(err)?;
        let durable = self.hit(&format!("{phase}_state_dir_sync")).and_then(|()| {
            File::open(&self.state_dir)
                .map_err(err)?
                .sync_all()
                .map_err(err)
        });
        Ok(durable.err().map(|error| format!("state snapshot is visible; directory durability is uncertain: {error}; inspect before continuing")))
    }
    fn current(&self, state: &State) -> Result<(String, Identity)> {
        self.validate(state)?;
        self.hit("target_read")?;
        read(&self.workspace.join("release.json"), MAX_CONTENT)
    }
    fn record(state: &mut State, kind: &str, value: impl Serialize) -> Result<()> {
        if state.records.len() >= MAX_RECORDS {
            return Err("session decision-record limit reached; use a fresh session".into());
        }
        state
            .records
            .push(json!({"sequence":state.records.len()+1,"kind":kind,"data":value}));
        Ok(())
    }
    fn permission(state: &State, id: &str, entry: &Entry) -> Decision {
        RunCoordinator.evaluate(Gate::Permission, &json!({
            "action_digest":id,
            "policy_decision": if entry.proposal.path == "release.json" { "ask" } else { "deny" },
            "risk_tags":[], "wrapper_chain":[],
            "approval": if entry.approved { json!({"status":"approved","subject_digest":id}) } else { Value::Null },
            "policy_digest":state.policy_digest,
        }))
    }
    fn reply(id: &str, decision: Decision, status: &str) -> Reply {
        Reply {
            request_id: id.into(),
            decision,
            execution_status: status.into(),
            observed_post_digest: None,
            note: None,
        }
    }
    pub fn read_file(&self, path: &str) -> Result<FileView> {
        if path != "release.json" {
            return Err("only release.json is readable; traversal is forbidden".into());
        }
        let _lock = self.lock()?;
        let mut state = self.load()?;
        let (content, _) = self.current(&state)?;
        let mut view = FileView {
            note: None,
            path: path.into(),
            digest: digest(content.as_bytes()),
            content,
        };
        Self::record(
            &mut state,
            "read_observation",
            json!({"path":path,"digest":view.digest}),
        )?;
        view.note = self.save(&state, "read")?;
        Ok(view)
    }
    pub fn propose_write(&self, path: &str, content: &str) -> Result<Reply> {
        if content.len() > MAX_CONTENT || path.len() > 256 {
            return Err("proposal exceeds size limit".into());
        }
        let _lock = self.lock()?;
        let mut state = self.load()?;
        if state.proposals.len() >= MAX_PROPOSALS {
            return Err("proposal limit reached; use a fresh session".into());
        }
        let before = if path == "release.json" {
            Some(self.current(&state)?)
        } else {
            None
        };
        let proposal = Proposal {
            session: state.session.clone(),
            path: path.into(),
            before: before.as_ref().map(|p| p.0.clone()),
            before_identity: before.map(|p| p.1),
            content: content.into(),
            capability_digest: state.capability_digest.clone(),
            policy_digest: state.policy_digest.clone(),
        };
        let id = canonical(&proposal)?;
        let entry = state
            .proposals
            .entry(id.clone())
            .or_insert(Entry {
                proposal,
                approved: false,
                execution_status: "pending".into(),
            })
            .clone();
        let decision = if entry.execution_status != "pending" {
            block("demo.request_consumed")
        } else {
            Self::permission(&state, &id, &entry)
        };
        let mut reply = Self::reply(&id, decision, "not_executed");
        Self::record(&mut state, "proposal_decision", &reply)?;
        reply.note = self.save(&state, "proposal")?;
        Ok(reply)
    }
    /// Operator-only; deliberately absent from the MCP tool table.
    pub fn approve(&self, id: &str) -> Result<Value> {
        let _lock = self.lock()?;
        let mut state = self.load()?;
        let entry = state.proposals.get(id).ok_or("unknown request_id")?.clone();
        if entry.execution_status != "pending" || entry.proposal.path != "release.json" {
            return Err("request is denied or already consumed".into());
        }
        if !self.matches(&state, &entry)? {
            return Err("proposal is stale; no approval recorded".into());
        }
        let mut approval = json!({"request_id":id,"actor":"local_operator","subject_digest":id,"status":"approved"});
        if entry.approved {
            return Ok(approval);
        }
        state.proposals.get_mut(id).unwrap().approved = true;
        Self::record(&mut state, "operator_approval", &approval)?;
        if let Some(warning) = self.save(&state, "approval")? {
            approval["durability_warning"] = json!(warning);
        }
        Ok(approval)
    }
    fn matches(&self, state: &State, entry: &Entry) -> Result<bool> {
        let (contents, identity) = self.current(state)?;
        Ok(entry.proposal.before.as_ref() == Some(&contents)
            && entry.proposal.before_identity.as_ref() == Some(&identity))
    }
    pub fn inspect(&self) -> Result<Value> {
        let _lock = self.lock()?;
        let state = self.load()?;
        let mut value = serde_json::to_value(&state).map_err(err)?;
        value["execution_count"] = json!(
            state
                .proposals
                .values()
                .filter(|p| p.execution_status == "executed")
                .count()
        );
        value["uncertain_requests"] = json!(
            state
                .proposals
                .iter()
                .filter(|(_, p)| ["executing", "unknown_outcome"]
                    .contains(&p.execution_status.as_str()))
                .map(|(id, _)| id)
                .collect::<Vec<_>>()
        );
        value["record_type"] =
            json!("MCP boundary decision records; not AgentRunEngine terminal receipts");
        value["model_usage"] = Value::Null;
        value["current_observation"] = match self.current(&state) {
            Ok((content, identity)) => {
                json!({"content":content,"digest":digest(content.as_bytes()),"identity":identity})
            }
            Err(error) => json!({"error":error}),
        };
        Ok(value)
    }
    pub fn apply_change(&self, id: &str) -> Result<Reply> {
        let _lock = self.lock()?;
        let mut state = self.load()?;
        let Some(entry) = state.proposals.get(id).cloned() else {
            return Ok(Self::reply(
                id,
                block("demo.unknown_request"),
                "not_executed",
            ));
        };
        let mut decision = Self::permission(&state, id, &entry);
        if entry.execution_status != "pending" {
            decision = block("demo.request_consumed");
        }
        if decision.outcome == Outcome::Allow && !self.matches(&state, &entry)? {
            decision = block("demo.stale_subject");
        }
        let trust = RunCoordinator.evaluate(Gate::ToolTrust, &json!({"capability_name":"gaap-goose-mcp","capability_digest":crate::protocol::capability_digest(),"approval":{"status":"approved","subject_digest":state.capability_digest}}));
        if trust.outcome != Outcome::Allow {
            decision = trust.clone();
        }
        let mut reply = Self::reply(id, decision, "not_executed");
        Self::record(&mut state, "tool_trust_decision", trust)?;
        Self::record(&mut state, "apply_decision", &reply)?;
        if reply.decision.outcome != Outcome::Allow {
            reply.note = self.save(&state, "decision")?;
            return Ok(reply);
        }
        // Reserve capacity for the final observation before dispatch.
        if state.records.len() >= MAX_RECORDS {
            return Err("no capacity for execution observation".into());
        }
        state.proposals.get_mut(id).unwrap().execution_status = "executing".into();
        if let Some(warning) = self.save(&state, "dispatch")? {
            return Err(format!(
                "{warning}; attempt consumed, no file effect dispatched; do not retry"
            ));
        }
        // Persisting executing consumes the authority even if this process disappears.
        match self.commit(&state, &entry) {
            Ok(warning) => {
                reply.execution_status = "executed".into();
                reply.observed_post_digest = Some(digest(entry.proposal.content.as_bytes()));
                reply.note = warning;
            }
            Err((committed, error)) => {
                reply.execution_status = if committed {
                    "unknown_outcome"
                } else {
                    "aborted"
                }
                .into();
                reply.note = Some(error);
            }
        }
        state.proposals.get_mut(id).unwrap().execution_status = reply.execution_status.clone();
        Self::record(&mut state, "execution_observation", &reply)?;
        match self.save(&state, "result") {
            Ok(Some(warning)) => {
                reply.note = Some(match reply.note.take() {
                    Some(note) => format!("{note}; {warning}"),
                    None => warning,
                });
            }
            Ok(None) => {}
            Err(error) => {
                reply.execution_status = "unknown_outcome".into();
                reply.note = Some(format!(
                    "result persistence failed before snapshot commit: {error}; inspect the file and operator state; do not retry"
                ));
            }
        }
        Ok(reply)
    }
    fn cleanup_stage(&self, staged: tempfile::NamedTempFile, cause: String) -> String {
        let path = staged.path().to_owned();
        // Retain only our own staging path if cleanup fails; never remove a caller path.
        if let Err(error) = self.hit("target_cleanup") {
            let _ = staged.keep();
            return format!(
                "{cause}; staging cleanup failed: {error}; inspect {}",
                path.display()
            );
        }
        match staged.close() {
            Ok(()) => cause,
            Err(error) => format!(
                "{cause}; staging cleanup failed: {error}; inspect {}",
                path.display()
            ),
        }
    }
    fn commit(
        &self,
        state: &State,
        entry: &Entry,
    ) -> std::result::Result<Option<String>, (bool, String)> {
        self.hit("target_create").map_err(|e| (false, e))?;
        let mut staged =
            tempfile::NamedTempFile::new_in(&self.workspace).map_err(|e| (false, err(e)))?;
        let preparation = (|| -> Result<()> {
            self.hit("target_write")?;
            staged
                .write_all(entry.proposal.content.as_bytes())
                .map_err(err)?;
            self.hit("target_metadata")?;
            let original_mode = entry
                .proposal
                .before_identity
                .as_ref()
                .ok_or("missing original file identity")?
                .mode;
            staged
                .as_file()
                .set_permissions(fs::Permissions::from_mode(original_mode))
                .map_err(err)?;
            self.hit("target_flush")?;
            staged.flush().map_err(err)?;
            self.hit("target_sync")?;
            staged.as_file().sync_all().map_err(err)?;
            self.hit("precommit")?;
            #[cfg(test)]
            match self.fault {
                Some("concurrent_edit") => {
                    fs::write(self.workspace.join("release.json"), "operator edit\n")
                        .map_err(err)?
                }
                Some("concurrent_symlink") => {
                    let target = self.workspace.join("release.json");
                    fs::remove_file(&target).map_err(err)?;
                    std::os::unix::fs::symlink(self.state_dir.join("state.json"), target)
                        .map_err(err)?;
                }
                _ => {}
            }
            if !self.matches(state, entry)? {
                return Err("file changed before commit".into());
            }
            self.hit("target_rename")?;
            Ok(())
        })();
        if let Err(error) = preparation {
            return Err((false, self.cleanup_stage(staged, error)));
        }
        if let Err(error) = staged.persist(self.workspace.join("release.json")) {
            return Err((
                false,
                self.cleanup_stage(error.file, error.error.to_string()),
            ));
        }
        self.hit("postcommit_observation").map_err(|e| (true, e))?;
        let (observed, _) = self.current(state).map_err(|e| (true, e))?;
        if observed != entry.proposal.content {
            return Err((true, "postcommit file differs from proposal".into()));
        }
        let durable = self.hit("target_dir_sync").and_then(|()| {
            File::open(&self.workspace)
                .map_err(err)?
                .sync_all()
                .map_err(err)
        });
        Ok(durable.err().map(|e| {
            format!("file replacement observed, but directory durability is uncertain: {e}")
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn prepared() -> (tempfile::TempDir, Store, String) {
        let parent = tempfile::tempdir().unwrap();
        let store = Store::initialize(&parent.path().join("demo")).unwrap();
        let request = store
            .propose_write("release.json", "{\"version\":\"1.1.0\"}\n")
            .unwrap();
        store.approve(&request.request_id).unwrap();
        (parent, store, request.request_id)
    }
    #[test]
    fn dispatch_persistence_faults_never_execute() {
        for point in [
            "state_read",
            "target_read",
            "dispatch_state_create",
            "dispatch_state_write",
            "dispatch_state_flush",
            "dispatch_state_sync",
            "dispatch_state_rename",
            "dispatch_state_dir_sync",
        ] {
            let (_parent, mut store, id) = prepared();
            store.fault = Some(point);
            assert!(store.apply_change(&id).is_err(), "{point}");
            assert_eq!(
                fs::read_to_string(store.workspace.join("release.json")).unwrap(),
                INITIAL,
                "{point}"
            );
            store.fault = None;
            let value = store.inspect().unwrap();
            assert_eq!(value["execution_count"], 0, "{point}");
            if point == "dispatch_state_dir_sync" {
                assert_eq!(value["uncertain_requests"][0], id);
                assert_eq!(
                    store.apply_change(&id).unwrap().decision.outcome,
                    Outcome::Block
                );
            }
        }
    }
    #[test]
    fn staging_failures_preserve_target_and_consume_attempt() {
        for point in [
            "target_create",
            "target_write",
            "target_metadata",
            "target_flush",
            "target_sync",
            "precommit",
            "target_rename",
        ] {
            let (_parent, mut store, id) = prepared();
            store.fault = Some(point);
            let reply = store.apply_change(&id).unwrap();
            assert_eq!(reply.execution_status, "aborted", "{point}");
            assert_eq!(
                fs::read_to_string(store.workspace.join("release.json")).unwrap(),
                INITIAL,
                "{point}"
            );
            assert_eq!(
                fs::read_dir(&store.workspace).unwrap().count(),
                1,
                "owned staging cleanup: {point}"
            );
            store.fault = None;
            assert_eq!(
                Store::open(store.state_dir())
                    .unwrap()
                    .apply_change(&id)
                    .unwrap()
                    .code(),
                "demo.request_consumed"
            );
        }
    }
    #[test]
    fn postcommit_faults_cannot_authorize_replay() {
        for point in [
            "postcommit_observation",
            "result_state_create",
            "result_state_write",
            "result_state_flush",
            "result_state_sync",
            "result_state_rename",
        ] {
            let (_parent, mut store, id) = prepared();
            store.fault = Some(point);
            let reply = store.apply_change(&id).unwrap();
            assert_eq!(reply.execution_status, "unknown_outcome", "{point}");
            assert_eq!(
                fs::read_to_string(store.workspace.join("release.json")).unwrap(),
                "{\"version\":\"1.1.0\"}\n"
            );
            store.fault = None;
            assert_eq!(
                Store::open(store.state_dir())
                    .unwrap()
                    .apply_change(&id)
                    .unwrap()
                    .code(),
                "demo.request_consumed"
            );
        }
    }
    #[test]
    fn postcommit_directory_sync_failure_reports_observed_effect_and_warning() {
        let (_parent, mut store, id) = prepared();
        store.fault = Some("target_dir_sync");
        let reply = store.apply_change(&id).unwrap();
        assert_eq!(reply.execution_status, "executed");
        assert!(reply.note.unwrap().contains("durability is uncertain"));
        assert!(reply.observed_post_digest.is_some());
    }
    #[test]
    fn immutable_proposal_policy_and_capability_drift_fail_closed() {
        for field in ["content", "capability_digest", "policy_digest"] {
            let (_parent, store, id) = prepared();
            let path = store.state_dir.join("state.json");
            let mut state: Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            state["proposals"][&id]["proposal"][field] = json!("forged");
            fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
            assert!(Store::open(store.state_dir()).is_err());
            assert_eq!(
                fs::read_to_string(store.workspace.join("release.json")).unwrap(),
                INITIAL
            );
        }
    }
    #[test]
    fn final_recheck_preserves_an_edit_made_after_initial_authorization() {
        let (_parent, mut store, id) = prepared();
        store.fault = Some("concurrent_edit");
        let reply = store.apply_change(&id).unwrap();
        assert_eq!(reply.decision.outcome, Outcome::Allow);
        assert_eq!(reply.execution_status, "aborted");
        assert_eq!(
            fs::read_to_string(store.workspace.join("release.json")).unwrap(),
            "operator edit\n"
        );
    }
    #[test]
    fn final_recheck_rejects_symlink_introduced_after_initial_authorization() {
        let (_parent, mut store, id) = prepared();
        store.fault = Some("concurrent_symlink");
        let reply = store.apply_change(&id).unwrap();
        assert_eq!(reply.execution_status, "aborted");
        assert!(
            fs::symlink_metadata(store.workspace.join("release.json"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        store.fault = None;
        let state = store.inspect().unwrap();
        assert_eq!(state["execution_count"], 0);
        assert_eq!(state["proposals"][&id]["execution_status"], "aborted");
    }
    #[test]
    fn staging_cleanup_failure_is_reported_without_touching_target() {
        let (_parent, mut store, id) = prepared();
        store.fault = Some("target_write,target_cleanup");
        let reply = store.apply_change(&id).unwrap();
        assert_eq!(reply.execution_status, "aborted");
        assert!(reply.note.unwrap().contains("staging cleanup failed"));
        assert_eq!(
            fs::read_to_string(store.workspace.join("release.json")).unwrap(),
            INITIAL
        );
        assert_eq!(fs::read_dir(&store.workspace).unwrap().count(), 2);
        store.fault = None;
        assert_eq!(
            store.apply_change(&id).unwrap().code(),
            "demo.request_consumed"
        );
    }
    #[test]
    fn repeated_approval_does_not_consume_record_capacity() {
        let (_parent, store, id) = prepared();
        let before = store.inspect().unwrap()["records"]
            .as_array()
            .unwrap()
            .len();
        for _ in 0..MAX_RECORDS + 1 {
            store.approve(&id).unwrap();
        }
        assert_eq!(
            store.inspect().unwrap()["records"]
                .as_array()
                .unwrap()
                .len(),
            before
        );
        assert_eq!(
            store.apply_change(&id).unwrap().execution_status,
            "executed"
        );
    }
    #[test]
    fn effective_proposal_and_approval_return_durability_warnings() {
        let parent = tempfile::tempdir().unwrap();
        let mut store = Store::initialize(&parent.path().join("demo")).unwrap();
        store.fault = Some("proposal_state_dir_sync");
        let proposal = store
            .propose_write("release.json", "version 1.1.0\n")
            .unwrap();
        assert!(proposal.note.unwrap().contains("snapshot is visible"));
        store.fault = Some("approval_state_dir_sync");
        let approval = store.approve(&proposal.request_id).unwrap();
        assert!(
            approval["durability_warning"]
                .as_str()
                .unwrap()
                .contains("snapshot is visible")
        );
        store.fault = None;
        assert_eq!(
            store.inspect().unwrap()["proposals"][&proposal.request_id]["approved"],
            true
        );
    }
    #[test]
    fn result_directory_warning_preserves_the_observed_execution_status() {
        let (_parent, mut store, id) = prepared();
        store.fault = Some("result_state_dir_sync");
        let reply = store.apply_change(&id).unwrap();
        assert_eq!(reply.execution_status, "executed");
        assert!(reply.note.unwrap().contains("snapshot is visible"));
        store.fault = None;
        assert_eq!(
            store.inspect().unwrap()["proposals"][&id]["execution_status"],
            "executed"
        );
        assert_eq!(
            store.apply_change(&id).unwrap().code(),
            "demo.request_consumed"
        );
    }
    #[test]
    fn recorded_executing_attempt_remains_uncertain_after_restart() {
        let (_parent, store, id) = prepared();
        let _lock = store.lock().unwrap();
        let mut state = store.load().unwrap();
        state.proposals.get_mut(&id).unwrap().execution_status = "executing".into();
        store.save(&state, "dispatch").unwrap();
        drop(_lock);
        let reopened = Store::open(store.state_dir()).unwrap();
        assert_eq!(reopened.inspect().unwrap()["uncertain_requests"][0], id);
        assert_eq!(
            reopened.apply_change(&id).unwrap().code(),
            "demo.request_consumed"
        );
        assert_eq!(
            fs::read_to_string(store.workspace.join("release.json")).unwrap(),
            INITIAL
        );
    }
}

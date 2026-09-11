//! Real Goose proposals through the full engine; one bounded run per proposed change.
mod contracts;
pub mod mcp;
mod verify;
use crate::store::{Identity, digest, directory, identity, open_regular, read, trusted_parent};
use contracts::*;
use gaap::{contracts::*, runtime::*};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs::{self, File},
    io::Write,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    rc::Rc,
    time::Instant,
};

type Result<T> = std::result::Result<T, String>;
pub const POLICY: &str = "shipping-demo/v1: shipping.py writes allowed; all other writes denied; one effect per run; two accounted calls; ten seconds engine budget; five seconds verifier deadline; verifier accepts bounded arithmetic Python only";
pub const TASK: &str = "Implement shipping_quote(quantity, unit_price). Reject quantity <= 0 with ValueError. Compute subtotal. Quantities of five or more receive a 10% discount. Add delivery of 7 unless the discounted subtotal is at least 100. Round the final price to two decimals. Use only arithmetic, assignments, conditionals, round, and raise ValueError. After verified implementation, request shipping_enabled=true in deployment.json; GAAP evaluates production authority.\n";
const INITIAL: &str =
    "def shipping_quote(quantity, unit_price):\n    return quantity * unit_price + 7\n";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct SnapshotFile {
    content: String,
    identity: Identity,
}
type Snapshot = BTreeMap<String, SnapshotFile>;
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub path: String,
    pub content: String,
    pub plan: String,
    pub base_digest: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    schema: String,
    root: PathBuf,
    root_identity: Identity,
    workspace_identity: Identity,
    operator_identity: Identity,
    trusted_schema: String,
    policy: String,
    runs: Vec<Value>,
    inflight: Option<Value>,
}
#[derive(Clone)]
pub struct CodingStore {
    root: PathBuf,
    #[cfg(test)]
    fault: Option<&'static str>,
}
pub fn hash_json(value: &impl Serialize) -> Result<String> {
    serde_jcs::to_vec(value)
        .map(|b| digest(&b))
        .map_err(|e| e.to_string())
}
fn snapshot_digest(snapshot: &Snapshot) -> Result<String> {
    hash_json(snapshot)
}

fn validate_record(run: &Value) -> Result<()> {
    let request: AgentRunRequest =
        serde_json::from_value(run["request"].clone()).map_err(|e| e.to_string())?;
    let effect: ProtectedEffectRequest =
        serde_json::from_value(run["effect_request"].clone()).map_err(|e| e.to_string())?;
    let receipt: TerminalRunReceipt =
        serde_json::from_value(run["receipt"].clone()).map_err(|e| e.to_string())?;
    verify_terminal_receipt(&request, &support(), &receipt)
        .map_err(|e| format!("receipt integrity check failed: {e}"))?;
    let change = Change {
        path: run["path"].as_str().ok_or("missing path")?.into(),
        content: run["proposed"]
            .as_str()
            .ok_or("missing proposed content")?
            .into(),
        plan: run["plan"].as_str().ok_or("missing plan")?.into(),
        base_digest: effect.subject.digest.clone(),
    };
    let input_digest = hash_json(&change)?;
    if run["input_digest"] != input_digest
        || effect.input_digest != input_digest
        || request.task.instructions != change.plan
    {
        return Err("recorded proposal differs from the bound engine input".into());
    }
    let results: Vec<ProtectedEffectResult> =
        serde_json::from_value(run["protected_effect_results"].clone())
            .map_err(|e| e.to_string())?;
    for result in results {
        verify_protected_effect_result(&request, &support(), &effect, &result)
            .map_err(|e| e.to_string())?;
        for reference in &result.body.evidence {
            if reference.locator.as_deref() == Some("gaap:record/execution_evidence")
                && reference.digest != hash_json(&run["execution_evidence"])?
            {
                return Err("execution evidence differs from the protected effect result".into());
            }
        }
    }
    for event in receipt.body.events {
        if let RunEvent::Verification { evidence, .. } = event {
            for reference in evidence {
                if reference.evidence_type == EvidenceType::CommandOutput
                    && reference.digest != hash_json(&run["verification"])?
                {
                    return Err("verification evidence differs from the terminal receipt".into());
                }
            }
        }
    }
    Ok(())
}

impl CodingStore {
    pub fn workspace(&self) -> PathBuf {
        self.root.join("workspace")
    }
    fn operator(&self) -> PathBuf {
        self.root.join("operator")
    }
    pub fn initialize(root: &Path) -> Result<Self> {
        let root = std::path::absolute(root).map_err(|e| e.to_string())?;
        let parent = root
            .parent()
            .ok_or("session needs a parent")?
            .canonicalize()
            .map_err(|e| e.to_string())?;
        trusted_parent(&parent)?;
        let root = parent.join(root.file_name().ok_or("session needs a name")?);
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .map_err(|e| e.to_string())?;
        let store = Self {
            root,
            #[cfg(test)]
            fault: None,
        };
        for path in [store.workspace(), store.operator()] {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(path)
                .map_err(|e| e.to_string())?;
        }
        for (name, content) in [
            ("shipping.py", INITIAL),
            (
                "deployment.json",
                "{\n  \"environment\": \"production\",\n  \"shipping_enabled\": false\n}\n",
            ),
            ("TASK.md", TASK),
        ] {
            let mut f =
                File::create_new(store.workspace().join(name)).map_err(|e| e.to_string())?;
            f.write_all(content.as_bytes())
                .and_then(|_| f.sync_all())
                .map_err(|e| e.to_string())?;
        }
        File::create_new(store.operator().join("lock"))
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        let state = State {
            schema: "gaap-coding-session/v1".into(),
            root: store.root.clone(),
            root_identity: identity(&store.root, true)?,
            workspace_identity: identity(&store.workspace(), true)?,
            operator_identity: identity(&store.operator(), true)?,
            trusted_schema: mcp::capability_digest(),
            policy: POLICY.into(),
            runs: vec![],
            inflight: None,
        };
        store.save(&state, "init")?;
        Ok(store)
    }
    pub fn open(root: &Path) -> Result<Self> {
        directory(root)?;
        let store = Self {
            root: root.canonicalize().map_err(|e| e.to_string())?,
            #[cfg(test)]
            fault: None,
        };
        store.load()?;
        Ok(store)
    }
    fn hit(&self, point: &str) -> Result<()> {
        #[cfg(test)]
        if self.fault == Some(point) {
            return Err(format!("injected failure: {point}"));
        }
        let _ = point;
        Ok(())
    }
    fn load(&self) -> Result<State> {
        directory(&self.root)?;
        let state: State =
            serde_json::from_str(&read(&self.operator().join("state.json"), 8 * 1024 * 1024)?.0)
                .map_err(|e| e.to_string())?;
        if state.schema != "gaap-coding-session/v1"
            || state.root != self.root
            || state.policy != POLICY
            || state.root_identity != identity(&self.root, true)?
            || state.workspace_identity != identity(&self.workspace(), true)?
            || state.operator_identity != identity(&self.operator(), true)?
        {
            return Err("session identity or policy changed; preserve it for inspection and use a fresh session".into());
        }
        for run in &state.runs {
            validate_record(run)?;
        }
        Ok(state)
    }
    fn save(&self, state: &State, phase: &str) -> Result<Option<String>> {
        self.hit(&format!("{phase}_create"))?;
        let mut file =
            tempfile::NamedTempFile::new_in(self.operator()).map_err(|e| e.to_string())?;
        self.hit(&format!("{phase}_write"))?;
        file.write_all(&serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        self.hit(&format!("{phase}_flush"))?;
        file.flush().map_err(|e| e.to_string())?;
        self.hit(&format!("{phase}_sync"))?;
        file.as_file().sync_all().map_err(|e| e.to_string())?;
        self.hit(&format!("{phase}_rename"))?;
        file.persist(self.operator().join("state.json"))
            .map_err(|e| e.to_string())?;
        Ok(self
            .hit(&format!("{phase}_dir_sync"))
            .and_then(|()| {
                File::open(self.operator())
                    .and_then(|f| f.sync_all())
                    .map_err(|e| e.to_string())
            })
            .err()
            .map(|e| format!("snapshot visible; directory durability uncertain: {e}")))
    }
    fn snapshot(&self) -> Result<Snapshot> {
        self.load()?;
        ["TASK.md", "shipping.py", "deployment.json"]
            .into_iter()
            .map(|name| {
                read(&self.workspace().join(name), 32768).map(|(content, identity)| {
                    (name.to_string(), SnapshotFile { content, identity })
                })
            })
            .collect()
    }
    pub fn inspect(&self) -> Result<Value> {
        let state = self.load()?;
        let snapshot = self.snapshot()?;
        Ok(
            json!({"schema":state.schema,"workspace":self.workspace(),"task":TASK,"policy":POLICY,"subject_digest":snapshot_digest(&snapshot)?,"files":snapshot.iter().map(|(k,v)|(k.clone(),v.content.clone())).collect::<BTreeMap<_,_>>(),"runs":state.runs,"inflight":state.inflight,"model_usage":null,"usage_scope":"engine operations and verification only; Goose model tokens and cost are not measured","limits":{"effects_per_run":1,"accounted_calls_per_run":2,"engine_elapsed_ms":10000,"verifier_timeout_ms":5000,"session_runs":12}}),
        )
    }
    pub fn submit(&self, input: Value) -> Result<Value> {
        let change: Change = serde_json::from_value(input).map_err(|e| e.to_string())?;
        if change.plan.trim().is_empty()
            || change.plan.len() > 4096
            || change.content.len() > 32768
            || change.path.len() > 200
        {
            return Err(
                "plan is required; plan/path/content must stay within the declared limits".into(),
            );
        }
        let lock = open_regular(&self.operator().join("lock"), true)?;
        lock.lock().map_err(|e| e.to_string())?;
        let mut state = self.load()?;
        if state.inflight.is_some() {
            return Err("previous run has an uncertain outcome; inspect the files and use a fresh session; no replay dispatched".into());
        }
        let input_digest = hash_json(&change)?;
        if let Some(existing) = state
            .runs
            .iter()
            .find(|run| run["input_digest"] == input_digest)
        {
            let mut copy = existing.clone();
            copy["duplicate"] = json!(true);
            return Ok(copy);
        }
        if state.runs.len() >= 12 {
            return Err(
                "session run limit reached; no effect dispatched; use a fresh session".into(),
            );
        }
        let before = self.snapshot()?;
        let request = request(self, state.runs.len() + 1, &change, &before)?;
        let effect = effect(&request, &change)?;
        // Validate before the durable running marker; malformed input cannot strand a session.
        validate_protected_effect_request(&request, &support(), &effect)
            .map_err(|e| e.to_string())?;
        state.inflight = Some(
            json!({"run_id":request.run_id,"path":change.path,"plan":change.plan,"input_digest":input_digest,"subject_digest":request.subject.digest,"status":"running_or_uncertain_after_restart"}),
        );
        if let Some(warning) = self.save(&state, "dispatch")? {
            return Err(format!(
                "{warning}; no effect dispatched; inspect before continuing"
            ));
        }
        let start = Instant::now();
        let report = Rc::new(RefCell::new(Value::Null));
        let evidence = Rc::new(RefCell::new(Value::Null));
        let mut engine = AgentRunEngine::new(
            runtime_support(&change, &state.trusted_schema),
            config(),
            GooseProposal {
                change: change.clone(),
                effect: Some(effect.clone()),
            },
            FileExecutor {
                store: self.clone(),
                before: before.clone(),
                change: change.clone(),
                evidence: evidence.clone(),
            },
            verify::ShippingVerifier {
                store: self.clone(),
                report: report.clone(),
            },
        );
        let execution = engine
            .run(request.clone())
            .map_err(|e| format!("run outcome requires inspection: {e}"))?;
        verify_terminal_receipt(&request, &support(), &execution.receipt)
            .map_err(|e| e.to_string())?;
        for result in &execution.protected_effect_results {
            verify_protected_effect_result(&request, &support(), &effect, result)
                .map_err(|e| e.to_string())?;
        }
        let after = self.snapshot()?;
        let mut result = json!({"input_digest":input_digest,"path":change.path,"plan":change.plan,"before":before.get(&change.path).map(|v|&v.content),"proposed":change.content,"after":after.get(&change.path).map(|v|&v.content),"request":request,"effect_request":effect,"receipt":execution.receipt,"protected_effect_results":execution.protected_effect_results,"verification":report.borrow().clone(),"execution_evidence":evidence.borrow().clone(),"terminal_receipt_verified":true,"elapsed_ms":start.elapsed().as_millis() as u64,"model_usage":null,"duplicate":false});
        state.inflight = None;
        state.runs.push(result.clone());
        if let Some(warning) = self.save(&state, "result")? {
            result["durability_warning"] = json!(warning);
        }
        Ok(result)
    }
}

struct FileExecutor {
    store: CodingStore,
    before: Snapshot,
    change: Change,
    evidence: Rc<RefCell<Value>>,
}
impl FileExecutor {
    fn commit(&self) -> std::result::Result<Snapshot, (bool, String)> {
        let prepare = (|| -> Result<_> {
            if self.change.path != "shipping.py" {
                return Err("executor scope refused".into());
            }
            if self.store.snapshot()? != self.before {
                return Err("workspace changed before execution".into());
            }
            self.store.hit("target_create")?;
            let mut staged = tempfile::NamedTempFile::new_in(self.store.workspace())
                .map_err(|e| e.to_string())?;
            self.store.hit("target_write")?;
            staged
                .write_all(self.change.content.as_bytes())
                .map_err(|e| e.to_string())?;
            self.store.hit("target_mode")?;
            staged
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
            self.store.hit("target_flush")?;
            staged.flush().map_err(|e| e.to_string())?;
            self.store.hit("target_sync")?;
            staged.as_file().sync_all().map_err(|e| e.to_string())?;
            #[cfg(test)]
            if self.store.fault == Some("concurrent_edit") {
                fs::write(
                    self.store.workspace().join("shipping.py"),
                    "concurrent work\n",
                )
                .map_err(|e| e.to_string())?;
            }
            if self.store.snapshot()? != self.before {
                return Err("workspace changed before commit; concurrent work preserved".into());
            }
            self.store.hit("target_rename")?;
            staged
                .persist(self.store.workspace().join("shipping.py"))
                .map_err(|e| e.to_string())?;
            Ok(())
        })();
        prepare.map_err(|e| (false, e))?;
        self.store.hit("postcommit").map_err(|e| (true, e))?;
        let after = self.store.snapshot().map_err(|e| (true, e))?;
        if after["shipping.py"].content != self.change.content {
            return Err((true, "postcommit content differs".into()));
        }
        File::open(self.store.workspace())
            .and_then(|f| f.sync_all())
            .map_err(|e| (true, e.to_string()))?;
        Ok(after)
    }
}
impl ExecutorPort for FileExecutor {
    fn execute(
        &mut self,
        request: &ProtectedEffectRequest,
    ) -> std::result::Result<ExecutorObservation, RuntimePortError> {
        let started = Instant::now();
        let outcome = self.commit();
        let (status, after, reason) = match outcome {
            Ok(after) => (EffectExecutionStatus::Executed, Some(after), None),
            Err((committed, e)) => (
                if committed {
                    EffectExecutionStatus::UnknownOutcome
                } else {
                    EffectExecutionStatus::Failed
                },
                if committed {
                    None
                } else {
                    self.store.snapshot().ok()
                },
                Some(e),
            ),
        };
        let usage = effect_usage(started.elapsed().as_millis() as u64, 1);
        let record = json!({"before":self.before,"after":after,"execution_status":status,"reason":reason,"usage":usage,"executor":executor_identity(),"boundary":profile()});
        let evidence_digest = hash_json(&record).map_err(RuntimePortError::new)?;
        *self.evidence.borrow_mut() = record;
        let mut types = vec![
            EffectEvidenceType::Executor,
            EffectEvidenceType::Sandbox,
            EffectEvidenceType::Usage,
        ];
        match status {
            EffectExecutionStatus::Executed => types.extend([
                EffectEvidenceType::SubjectObservation,
                EffectEvidenceType::Mutation,
                EffectEvidenceType::Artifact,
            ]),
            EffectExecutionStatus::UnknownOutcome => types.push(EffectEvidenceType::UnknownOutcome),
            _ => types.extend([
                EffectEvidenceType::Failure,
                EffectEvidenceType::SubjectObservation,
                EffectEvidenceType::Artifact,
                EffectEvidenceType::Mutation,
            ]),
        }
        Ok(ExecutorObservation {
            execution_status: status,
            observed_post_effect_subject: after
                .as_ref()
                .map(|s| subject(&self.store, s))
                .transpose()
                .map_err(RuntimePortError::new)?,
            exit: None,
            usage,
            executor: Some(executor_identity()),
            sandbox_profile: Some(request.sandbox_profile.clone()),
            reason,
            evidence: types
                .into_iter()
                .map(|evidence_type| EffectEvidenceReference {
                    evidence_type,
                    digest: evidence_digest.clone(),
                    locator: Some("gaap:record/execution_evidence".into()),
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const GOOD: &str = "def shipping_quote(quantity, unit_price):\n    if quantity <= 0:\n        raise ValueError('invalid quantity')\n    subtotal = quantity * unit_price\n    if quantity >= 5:\n        subtotal = subtotal * 0.9\n    if subtotal < 100:\n        subtotal = subtotal + 7\n    return round(subtotal, 2)\n";
    fn input(store: &CodingStore) -> Value {
        json!({"path":"shipping.py","content":GOOD,"plan":"Implement shipping rules.","base_digest":store.inspect().unwrap()["subject_digest"]})
    }
    fn prepared() -> (tempfile::TempDir, CodingStore) {
        let parent = tempfile::tempdir().unwrap();
        let store = CodingStore::initialize(&parent.path().join("coding")).unwrap();
        (parent, store)
    }
    #[test]
    fn engine_budget_refuses_execution_before_any_write() {
        let (_parent, store) = prepared();
        let change: Change = serde_json::from_value(input(&store)).unwrap();
        let before = store.snapshot().unwrap();
        let mut request = request(&store, 1, &change, &before).unwrap();
        request.resource_budget.max_tool_calls = 0;
        let effect = effect(&request, &change).unwrap();
        let mut engine = AgentRunEngine::new(
            runtime_support(&change, &mcp::capability_digest()),
            config(),
            GooseProposal {
                change: change.clone(),
                effect: Some(effect),
            },
            FileExecutor {
                store: store.clone(),
                before,
                change,
                evidence: Rc::new(RefCell::new(Value::Null)),
            },
            verify::ShippingVerifier {
                store: store.clone(),
                report: Rc::new(RefCell::new(Value::Null)),
            },
        );
        let result = engine.run(request.clone()).unwrap();
        verify_terminal_receipt(&request, &support(), &result.receipt).unwrap();
        assert_eq!(result.receipt.body.terminal_status, AgentRunStatus::Blocked);
        assert!(result.receipt.body.terminal_reason.starts_with("runtime."));
        assert_eq!(
            fs::read_to_string(store.workspace().join("shipping.py")).unwrap(),
            INITIAL
        );
    }

    #[test]
    fn verification_rejects_a_change_after_execution() {
        let (_parent, mut store) = prepared();
        let change = input(&store);
        store.fault = Some("verification_drift");
        let result = store.submit(change).unwrap();
        assert_eq!(
            result["protected_effect_results"][0]["body"]["execution_status"],
            "executed"
        );
        assert_eq!(result["verification"]["fresh"], false);
        assert_eq!(result["verification"]["passed"], false);
        assert_eq!(result["receipt"]["body"]["terminal_status"], "blocked");
        assert_eq!(
            fs::read_to_string(store.workspace().join("shipping.py")).unwrap(),
            "operator edit during verification\n"
        );
    }

    #[test]
    fn dispatch_persistence_failures_cannot_write() {
        for point in [
            "dispatch_create",
            "dispatch_write",
            "dispatch_flush",
            "dispatch_sync",
            "dispatch_rename",
            "dispatch_dir_sync",
        ] {
            let (_parent, mut store) = prepared();
            let change = input(&store);
            store.fault = Some(point);
            assert!(store.submit(change).is_err(), "{point}");
            assert_eq!(
                fs::read_to_string(store.workspace().join("shipping.py")).unwrap(),
                INITIAL,
                "{point}"
            );
            let state = store.inspect().unwrap();
            assert_eq!(
                state["inflight"].is_null(),
                point != "dispatch_dir_sync",
                "{point}"
            );
        }
    }
    #[test]
    fn every_precommit_fault_preserves_original_and_records_failure() {
        for point in [
            "target_create",
            "target_write",
            "target_mode",
            "target_flush",
            "target_sync",
            "target_rename",
        ] {
            let (_parent, mut store) = prepared();
            let change = input(&store);
            store.fault = Some(point);
            let result = store.submit(change).unwrap();
            assert_eq!(
                result["protected_effect_results"][0]["body"]["execution_status"], "failed",
                "{point}: {result}"
            );
            assert_eq!(
                fs::read_to_string(store.workspace().join("shipping.py")).unwrap(),
                INITIAL,
                "{point}"
            );
            assert!(store.inspect().unwrap()["inflight"].is_null());
        }
    }
    #[test]
    fn result_persistence_failure_leaves_uncertain_attempt_and_never_replays() {
        for point in [
            "result_create",
            "result_write",
            "result_flush",
            "result_sync",
            "result_rename",
        ] {
            let (_parent, mut store) = prepared();
            let change = input(&store);
            store.fault = Some(point);
            assert!(store.submit(change.clone()).is_err(), "{point}");
            assert_eq!(
                fs::read_to_string(store.workspace().join("shipping.py")).unwrap(),
                GOOD
            );
            store.fault = None;
            assert!(!store.inspect().unwrap()["inflight"].is_null());
            assert!(
                store
                    .submit(change)
                    .unwrap_err()
                    .contains("uncertain outcome")
            );
        }
    }
    #[test]
    fn postcommit_uncertainty_and_durability_warning_are_distinct() {
        for point in ["postcommit", "result_dir_sync"] {
            let (_parent, mut store) = prepared();
            let change = input(&store);
            store.fault = Some(point);
            let result = store.submit(change).unwrap();
            assert_eq!(
                fs::read_to_string(store.workspace().join("shipping.py")).unwrap(),
                GOOD
            );
            if point == "postcommit" {
                assert_eq!(
                    result["protected_effect_results"][0]["body"]["execution_status"],
                    "unknown_outcome"
                );
            } else {
                assert_eq!(result["receipt"]["body"]["terminal_status"], "completed");
                assert!(result["durability_warning"].is_string());
            }
        }
    }
    #[test]
    fn stale_and_concurrent_inputs_preserve_operator_work() {
        let (_parent, store) = prepared();
        let change = input(&store);
        fs::write(store.workspace().join("shipping.py"), "operator edit\n").unwrap();
        let result = store.submit(change).unwrap();
        assert_eq!(
            result["receipt"]["body"]["terminal_reason"],
            "protected_effect.stale_subject"
        );
        assert_eq!(
            fs::read_to_string(store.workspace().join("shipping.py")).unwrap(),
            "operator edit\n"
        );
        let (_parent, mut store) = prepared();
        let change = input(&store);
        store.fault = Some("concurrent_edit");
        let result = store.submit(change).unwrap();
        assert_eq!(
            result["protected_effect_results"][0]["body"]["execution_status"],
            "failed"
        );
        assert_eq!(
            fs::read_to_string(store.workspace().join("shipping.py")).unwrap(),
            "concurrent work\n"
        );
    }
    #[test]
    fn symlinks_hardlinks_and_relocated_sessions_are_rejected() {
        let (parent, store) = prepared();
        let change = input(&store);
        let source = store.workspace().join("shipping.py");
        fs::remove_file(&source).unwrap();
        std::os::unix::fs::symlink(parent.path().join("unowned"), &source).unwrap();
        assert!(store.submit(change).is_err());
        fs::remove_file(&source).unwrap();
        fs::write(&source, INITIAL).unwrap();
        fs::hard_link(&source, parent.path().join("second-link")).unwrap();
        assert!(store.inspect().is_err());
        fs::remove_file(parent.path().join("second-link")).unwrap();
        let moved = parent.path().join("moved");
        fs::rename(&store.root, &moved).unwrap();
        assert!(CodingStore::open(&moved).is_err());
    }
    #[test]
    fn tool_trust_is_bound_to_advertised_schema() {
        let (_parent, store) = prepared();
        let change = input(&store);
        let mut state = store.load().unwrap();
        state.trusted_schema = digest(b"different schema");
        store.save(&state, "test").unwrap();
        let result = store.submit(change).unwrap();
        assert_eq!(result["receipt"]["body"]["terminal_status"], "blocked");
        assert!(
            result["receipt"]["body"]["terminal_reason"]
                .as_str()
                .unwrap()
                .starts_with("tool_trust.")
        );
        assert_eq!(
            fs::read_to_string(store.workspace().join("shipping.py")).unwrap(),
            INITIAL
        );
    }
    #[test]
    fn parallel_duplicate_submission_executes_exactly_once() {
        let (_parent, store) = prepared();
        let change = input(&store);
        let other = store.clone();
        let second = change.clone();
        let thread = std::thread::spawn(move || other.submit(second).unwrap());
        let first = store.submit(change).unwrap();
        let second = thread.join().unwrap();
        assert_ne!(first["duplicate"], second["duplicate"]);
        assert_eq!(
            store.inspect().unwrap()["runs"].as_array().unwrap().len(),
            1
        );
    }
}

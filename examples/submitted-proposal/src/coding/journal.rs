//! One engine invocation, from fresh journal admission through publication of its sealed receipt.
use super::*;
use gaap_journal::{
    Error, Result as JResult, handoff,
    live::{self, Admission, Witness},
    writer::{self, Submission},
};
use std::time::Duration;

mod files;

pub fn submit(key: &str) -> JResult<Value> {
    let host = handoff::host_config()?;
    if !host.can_read {
        return Err(Error::new("forbidden", "current host permission denied"));
    }
    let health = handoff::inherited(4)?;
    let proposal = handoff::caller_proposal()?;
    writer::with_submission(&host, &health, key, &proposal, |tx| run(tx, key, &proposal))
}

fn dedicated_workspace(tx: &Submission<'_>) -> JResult<()> {
    let path = Path::new(&tx.host().slot.workspace.path);
    if path.file_name().is_some_and(|name| name == "workspace") {
        // A legacy session owns ROOT/workspace through ROOT/operator. Its locks
        // are independent of this journal. coding-init requires a fresh ROOT.
        let sibling = path.with_file_name("operator");
        let exists = tx.target_io("check_legacy_session", || {
            match fs::symlink_metadata(sibling) {
                Ok(_) => Ok(true),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(e),
            }
        })?;
        if exists {
            return Err(Error::new("forbidden", "legacy_session_workspace"));
        }
    }
    Ok(())
}

fn run(tx: &mut Submission<'_>, key: &str, proposal: &Value) -> JResult<()> {
    gaap_journal::recovery::seal_gate(tx.host())?;
    tx.check_workspace()?;
    dedicated_workspace(tx)?;
    let enrolled = &tx.host().profile;
    let schema = mcp::capability_digest();
    let runtime = config();
    if enrolled.implementer_id != runtime.implementer_id
        || enrolled.verifier_id != VERIFIER_ID
        || enrolled.executor != runtime.executor_identity
        || enrolled.policies.as_slice() != [policy()]
        || enrolled.capability != capability(&schema)
        || enrolled.tool_schema_digest != schema
        || enrolled.sandbox != profile()
    {
        return Err(Error::unsupported("incompatible_shipping_profile"));
    }
    let change: Change = serde_json::from_value(proposal.clone())
        .map_err(|_| Error::new("invalid_request", "invalid_shipping_proposal"))?;
    let before = files::snapshot(tx)?;
    let subject = subject_at(Path::new(&tx.host().slot.workspace.path), &before)
        .map_err(|_| Error::invalid("shipping_subject_digest"))?;
    if subject.digest != change.base_digest {
        return Err(Error::new("forbidden", "stale_shipping_subject"));
    }
    let proposal_digest = gaap_journal::digest(&gaap_journal::canonical(proposal)?);
    let admission = live::admit(tx.host(), key, &proposal_digest, &subject)?;
    let nonce = nonce().map_err(|_| Error::unavailable("run_identity"))?;
    let request = bound_request(
        &change,
        subject,
        &nonce,
        admission.approval.clone(),
        admission.budget.clone(),
    );
    let effect = effect(&request, &change)
        .map_err(|_| Error::new("invalid_request", "shipping_effect_request"))?;
    let control = RunControl::new(Duration::from_millis(
        request.resource_budget.max_elapsed_ms.min(10_000),
    ));
    let adapter = SubmittedProposalAdapter::new(
        SubmittedProposal::new(&change, &change.plan, &change.base_digest)
            .map_err(|_| Error::new("invalid_request", "shipping_proposal"))?,
        &request,
        effect.clone(),
        &support(),
        control.clone(),
    )
    .map_err(|_| Error::new("invalid_request", "shipping_adapter_request"))?;
    let attempt = &nonce[..32];
    let binding = tx.begin(attempt, &request, &effect)?;
    let workspace = JournalWorkspace {
        tx: RefCell::new(tx),
        admission,
        attempt,
        binding,
        run: &request,
        effect: &effect,
        change: &change,
        witness: RefCell::new(None),
        dispatch: RefCell::new(None),
        verified_snapshot: RefCell::new(None),
        completion_usage: RefCell::new(None),
        failure: RefCell::new(None),
    };
    let result = AgentRunEngine::new(
        runtime_support(&change, &schema),
        runtime,
        adapter,
        FileExecutor {
            store: &workspace,
            before,
            change: change.clone(),
            evidence: Rc::new(RefCell::new(Value::Null)),
            control,
        },
        verify::ShippingVerifier {
            store: &workspace,
            report: Rc::new(RefCell::new(Value::Null)),
        },
    )
    .run(request.clone());
    if let Some(error) = workspace.failure.take() {
        return Err(error);
    }
    let execution = result.map_err(|_| Error::unavailable("shipping_engine_checkpoint"))?;
    let result = match execution.protected_effect_results.as_slice() {
        [] => None,
        [only] => Some(only),
        _ => return Err(Error::invalid("extra_engine_results")),
    };
    workspace
        .tx
        .into_inner()
        .publish(&execution.receipt, result)
}

struct JournalWorkspace<'a, 's> {
    tx: RefCell<&'a mut Submission<'s>>,
    admission: Admission,
    attempt: &'a str,
    binding: String,
    run: &'a AgentRunRequest,
    effect: &'a ProtectedEffectRequest,
    change: &'a Change,
    witness: RefCell<Option<Witness>>,
    dispatch: RefCell<Option<(ResourceUsage, EffectUsage)>>,
    verified_snapshot: RefCell<Option<Value>>,
    completion_usage: RefCell<Option<ResourceUsage>>,
    failure: RefCell<Option<Error>>,
}

impl JournalWorkspace<'_, '_> {
    // Storage failures must survive the string-based executor/verifier observations.
    // A later engine receipt can't convert a failed durable checkpoint into success.
    fn remember(&self, error: Error) -> String {
        let message = error.reason.to_owned();
        self.failure.borrow_mut().get_or_insert(error);
        message
    }

    fn checkpoint(
        &self,
        operation: impl FnOnce() -> JResult<()>,
    ) -> std::result::Result<(), RuntimeError> {
        operation().map_err(|error| RuntimeError::Checkpoint(self.remember(error)))
    }
}

impl ShippingWorkspace for JournalWorkspace<'_, '_> {
    type Stage = files::Stage;

    fn path(&self) -> PathBuf {
        PathBuf::from(&self.tx.borrow().host().slot.workspace.path)
    }
    fn snapshot(&self) -> Result<Snapshot> {
        files::snapshot(&self.tx.borrow()).map_err(|error| self.remember(error))
    }

    fn confirm_dispatch(
        &self,
        effect: &ProtectedEffectRequest,
        events: &[RunEvent],
        usage: &ResourceUsage,
        reserved: &EffectUsage,
    ) -> std::result::Result<(), RuntimeError> {
        self.checkpoint(|| {
            let mut tx = self.tx.borrow_mut();
            let proposed = digest(self.change.content.as_bytes());
            let witness = Witness::arm(&tx, &self.binding, self.attempt, &proposed)?;
            let grants = self.admission.dispatch_check(
                &tx,
                &self.binding,
                self.run,
                effect,
                usage,
                reserved,
            )?;
            let decisions: Vec<_> = events
                .iter()
                .filter(|event| matches!(event, RunEvent::ProtectedEffectDecision { .. }))
                .collect();
            tx.dispatch(json!({
                "before": witness.before(),
                "executor": executor_identity(),
                "decisions": decisions,
                "checked_at_ms": grants.checked_at_ms,
                "checks": grants.checks,
                "reserved": reserved,
                "usage_floor": usage,
            }))?;
            gaap_journal::recovery::enter(tx.host(), self.attempt, &self.binding)?;
            self.witness.replace(Some(witness));
            self.dispatch
                .replace(Some((usage.clone(), reserved.clone())));
            Ok(())
        })
    }

    fn stage(&self, content: &str) -> Result<Self::Stage> {
        let mut witness = self.witness.borrow_mut();
        let witness = witness.as_mut().ok_or("shipping witness missing")?;
        files::Stage::prepare(&self.tx.borrow(), self.attempt, content, witness)
            .map_err(|error| self.remember(error))
    }

    fn replace(
        &self,
        stage: &mut Self::Stage,
        before: &Snapshot,
        control: &RunControl,
    ) -> Result<()> {
        let tx = self.tx.borrow();
        let dispatch = self.dispatch.borrow();
        let (usage, reserved) = dispatch.as_ref().ok_or("dispatch reservation missing")?;
        qualify("target.before_rename");
        let permit = tx
            .target_mutation("target_rename", || {
                // Return authority errors separately so the health guard sees the syscall's original errno.
                let result = self.admission.with_rename_permit(
                    tx.host(),
                    self.run,
                    self.effect,
                    reserved,
                    || {
                        qualify("target.authority_held_before_rename");
                        dedicated_workspace(&tx)?;
                        if control.cancellation().is_some() {
                            return Err(Error::new("forbidden", "shipping_cancelled"));
                        }
                        let current = files::snapshot(&tx)?;
                        if current != *before {
                            return Err(Error::new("forbidden", "stale_shipping_subject"));
                        }
                        stage.recheck(&tx, &self.change.content)?;
                        let subject =
                            subject_at(Path::new(&tx.host().slot.workspace.path), &current)
                                .map_err(|_| Error::invalid("shipping_subject_digest"))?;
                        Ok((subject, usage.clone()))
                    },
                    || stage.rename(tx.workspace_dir()),
                );
                match result {
                    Ok(raw) => raw.map(Ok),
                    Err(denied) => Ok(Err(denied)),
                }
            })
            .map_err(|error| self.remember(error))?;
        permit.map_err(|error| {
            if error.status == "forbidden" {
                error.reason.to_owned()
            } else {
                self.remember(error)
            }
        })?;
        qualify("target.after_rename");
        Ok(())
    }

    fn abandon(&self, stage: Self::Stage, primary: String) -> String {
        let failure = self.failure.take();
        if failure.as_ref().is_some_and(Error::health_hold) {
            self.failure.replace(failure);
            return primary;
        }
        // Preserve the operation and cleanup diagnostics. A failed health authority
        // takes precedence because the host must withdraw the volume.
        let failure = match (stage.remove(&self.tx.borrow()), failure) {
            (Ok(()), original) => original,
            (Err(cleanup), Some(original)) if cleanup.health_hold() => {
                Some(cleanup.with_warning(original.reason))
            }
            (Err(_), Some(original)) => Some(original.with_warning("owned_stage_cleanup_failed")),
            (Err(cleanup), None) => Some(cleanup.with_warning("target_commit_aborted")),
        };
        self.failure.replace(failure);
        primary
    }

    fn sync_workspace(&self) -> Result<()> {
        let tx = self.tx.borrow();
        tx.target_writeback("target_dir_sync", || tx.workspace_dir().sync_all())
            .map_err(|error| self.remember(error))
    }

    fn confirm_effect(
        &self,
        result: &ProtectedEffectResult,
        usage: &ResourceUsage,
        record: &Value,
    ) -> std::result::Result<(), RuntimeError> {
        if self.failure.borrow().is_some() {
            return Err(RuntimeError::Checkpoint("shipping operation failed".into()));
        }
        self.checkpoint(|| {
            let mut tx = self.tx.borrow_mut();
            let witness = self
                .witness
                .take()
                .ok_or_else(|| Error::invalid("shipping_witness_missing"))?;
            let observed = witness.finish(&tx, &self.admission)?;
            live::retain_executor(&tx, &self.binding, record)?;
            tx.observed(json!({
                "disposition": observed.disposition,
                "after": observed.after,
                "source": observed.source,
                "usage_floor": usage,
            }))?;
            if observed.disposition == "unresolved"
                && result.body.execution_status == EffectExecutionStatus::Executed
            {
                return Err(Error::unavailable("shipping_effect_unresolved"));
            }
            Ok(())
        })
    }

    fn verifier_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        self.verified_snapshot.replace(Some(
            serde_json::to_value(snapshot).map_err(|e| e.to_string())?,
        ));
        Ok(())
    }

    fn confirm_verification(
        &self,
        event: &RunEvent,
        usage: &ResourceUsage,
        report: &Value,
    ) -> std::result::Result<(), RuntimeError> {
        self.checkpoint(|| {
            let mut tx = self.tx.borrow_mut();
            let snapshot = self
                .verified_snapshot
                .take()
                .ok_or_else(|| Error::invalid("shipping_verifier_snapshot_missing"))?;
            live::retain_verifier(&tx, &self.binding, self.attempt, event, report, &snapshot)?;
            tx.verified(json!({
                "event": event,
                "usage_floor": usage,
            }))
        })
    }

    fn check_completion(&self, subject: &Subject, usage: &ResourceUsage) -> Result<()> {
        let current = subject_at(&self.path(), &self.snapshot()?)?;
        if &current != subject {
            return Err("shipping subject changed after verification".into());
        }
        self.completion_usage.replace(Some(usage.clone()));
        Ok(())
    }

    fn claim_completion(&self) -> Result<()> {
        let usage = self
            .completion_usage
            .take()
            .ok_or("missing final shipping usage")?;
        self.admission
            .check_completion(self.tx.borrow().host(), &usage)
            .map_err(|error| {
                if error.status == "forbidden" {
                    error.reason.to_owned()
                } else {
                    self.remember(error)
                }
            })
    }
}

fn qualify(point: &'static str) {
    #[cfg(feature = "qualification")]
    if std::env::var("GAAP_JOURNAL_POINT").as_deref() == Ok(point)
        && std::env::var("GAAP_JOURNAL_ACTION").as_deref() == Ok("stop")
    {
        eprintln!("journal qualification stop {point}");
        rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::STOP)
            .expect("qualification stop must reach this worker");
    }
    #[cfg(not(feature = "qualification"))]
    let _ = point;
}

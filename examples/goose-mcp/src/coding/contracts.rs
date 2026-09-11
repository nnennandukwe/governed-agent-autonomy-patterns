use super::*;

pub fn policy() -> PolicyIdentity {
    PolicyIdentity {
        name: "shipping-demo-policy".into(),
        version: "1".into(),
        digest: digest(POLICY.as_bytes()),
    }
}
pub fn capability(schema_digest: &str) -> CapabilityIdentity {
    CapabilityIdentity {
        name: "gaap.submit_change".into(),
        version: "1".into(),
        digest: schema_digest.into(),
    }
}
pub fn executor_identity() -> ExecutorIdentity {
    ExecutorIdentity {
        name: "gaap-coding-filesystem-executor".into(),
        version: "1".into(),
        digest: digest(b"exact snapshot recheck; once; atomic file replacement"),
    }
}
pub fn profile() -> SandboxProfileIdentity {
    SandboxProfileIdentity {
        name: "mediated-workspace-and-bounded-python".into(),
        version: "1".into(),
        digest: digest(verify::PROGRAM.as_bytes()),
    }
}
pub fn support() -> ContractSupport {
    ContractSupport::new([policy()])
}
pub fn resources(elapsed_ms: u64, tool_calls: u64) -> ResourceUsage {
    ResourceUsage {
        cost_micros: 0,
        model_tokens: 0,
        elapsed_ms,
        tool_calls,
    }
}
pub fn effect_usage(elapsed_ms: u64, tool_calls: u64) -> EffectUsage {
    EffectUsage {
        cost_micros: 0,
        model_tokens: 0,
        elapsed_ms,
        tool_calls,
    }
}
pub fn subject(store: &CodingStore, snapshot: &Snapshot) -> Result<Subject> {
    Ok(Subject {
        kind: SubjectKind::Repository,
        locator: store.workspace().display().to_string(),
        digest: snapshot_digest(snapshot)?,
    })
}

pub fn request(
    store: &CodingStore,
    number: usize,
    change: &Change,
    before: &Snapshot,
) -> Result<AgentRunRequest> {
    let subject = subject(store, before)?;
    Ok(AgentRunRequest {
        schema_version: AGENT_RUN_REQUEST_SCHEMA.into(),
        request_id: format!("coding-{number}"),
        run_id: format!("coding-{number}"),
        subject: subject.clone(),
        requested_capability: capability(&mcp::capability_digest()),
        task: TaskSpec {
            instructions: change.plan.clone(),
            constraints: vec![POLICY.into()],
        },
        policies: vec![policy()],
        resource_budget: ResourceBudget {
            max_cost_micros: 1_000_000,
            max_model_tokens: 100_000,
            max_elapsed_ms: 10_000,
            max_tool_calls: 2,
        },
        approval_context: vec![ApprovalReference {
            approval_id: format!("preauthorized-local-policy-{number}"),
            actor_id: "local-demo-policy".into(),
            scope: "bounded workspace task under shipping-demo-policy; production writes denied"
                .into(),
            subject_digest: subject.digest,
            evidence: EvidenceReference {
                evidence_type: EvidenceType::Approval,
                digest: policy().digest,
                locator: Some("gaap:session/policy".into()),
            },
        }],
        required_verification: VerificationRequirement {
            independence: VerificationIndependence::DifferentActor,
            evidence_types: vec![EvidenceType::CommandOutput, EvidenceType::Artifact],
        },
    })
}
pub fn effect(request: &AgentRunRequest, change: &Change) -> Result<ProtectedEffectRequest> {
    let mut subject = request.subject.clone();
    subject.digest = change.base_digest.clone();
    Ok(ProtectedEffectRequest {
        schema_version: PROTECTED_EFFECT_REQUEST_SCHEMA.into(),
        effect_id: format!("{}/write", request.run_id),
        effect_sequence: 1,
        run_id: request.run_id.clone(),
        agent_run_request_digest: validate_request(request, &support())
            .map_err(|e| e.to_string())?,
        subject,
        operation_family: OperationFamily::Filesystem,
        normalized_operation: "filesystem.write".into(),
        capability: request.requested_capability.clone(),
        tool_schema_digest: Some(mcp::capability_digest()),
        input_digest: hash_json(change)?,
        input_metadata: vec![InputMetadataEntry {
            name: "path".into(),
            value: change.path.clone(),
        }],
        requested_scopes: vec![RequestedScope::Filesystem {
            root: request.subject.locator.clone(),
            access: vec![FilesystemAccess::Read, FilesystemAccess::Modify],
            recursive: false,
        }],
        policies: request.policies.clone(),
        approval_context: vec![],
        resource_budget_digest: canonical_resource_budget_digest(&request.resource_budget)
            .map_err(|e| e.to_string())?,
        sandbox_profile: profile(),
        idempotency_key: format!("{}/write", request.run_id),
        repeatability: Repeatability::Idempotent,
        expected_effect_class: EffectClass::Mutation,
    })
}
pub fn runtime_support(change: &Change, trusted_schema: &str) -> RuntimeSupport {
    RuntimeSupport::new(
        support(),
        vec![capability(trusted_schema)],
        PermissionPolicy {
            policy_decision: Some(if change.path == "shipping.py" {
                PolicyDecision::Allow
            } else {
                PolicyDecision::Deny
            }),
            risk_tags: vec![],
            wrapper_chain: vec![],
            approvals: vec![],
        },
        effect_usage(1, 1),
        vec![],
        vec!["independent-shipping-oracle".into()],
    )
}
pub fn config() -> RuntimeConfig {
    RuntimeConfig {
        implementer_id: "goose-proposal-adapter".into(),
        executor_identity: executor_identity(),
        max_effects: 1,
        budget_thresholds: BudgetThresholds {
            warn_micros: 800_000,
            approval_micros: 900_000,
            hard_stop_micros: 1_000_000,
        },
    }
}
pub struct GooseProposal {
    pub change: Change,
    pub effect: Option<ProtectedEffectRequest>,
}
impl AgentAdapter for GooseProposal {
    fn plan(
        &mut self,
        _: &AgentRunRequest,
    ) -> std::result::Result<PortObservation<PlanProposal>, RuntimePortError> {
        Ok(PortObservation {
            value: PlanProposal {
                plan_digest: hash_json(&self.change).map_err(RuntimePortError::new)?,
            },
            usage: resources(0, 0),
        })
    }
    fn next_effect(
        &mut self,
        _: AgentRunContext<'_>,
    ) -> std::result::Result<PortObservation<AgentStep>, RuntimePortError> {
        Ok(PortObservation {
            value: self.effect.take().map_or(AgentStep::Finish, |request| {
                AgentStep::ProtectedEffect(Box::new(ProtectedEffectProposal { request }))
            }),
            usage: resources(0, 0),
        })
    }
}

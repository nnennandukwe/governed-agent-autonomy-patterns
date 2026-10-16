//! Additive interactive conformance seam; no corpus or expected outcomes live here.
use std::cell::RefCell;
use std::io::{self, BufRead, Write};
use std::rc::Rc;

use gaap::contracts::*;
use gaap::runtime::*;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const VERSION: &str = "run-invariant.runtime/0.2.0";
const MAX_BYTES: u64 = 4 * 1024 * 1024;

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
fn bytes(value: &Value) -> Result<Vec<u8>, String> {
    serde_jcs::to_vec(value).map_err(|error| error.to_string())
}
fn frame(value: &Value) -> Result<Vec<u8>, String> {
    let mut result = bytes(value)?;
    result.push(b'\n');
    Ok(result)
}
struct Transport {
    reader: io::BufReader<io::Stdin>,
    transcript: Vec<u8>,
    sequence: u64,
}
impl Transport {
    fn read(&mut self) -> Result<(Value, Vec<u8>), String> {
        let mut line = Vec::new();
        (&mut self.reader)
            .take(MAX_BYTES + 1)
            .read_until(b'\n', &mut line)
            .map_err(|error| error.to_string())?;
        if line.is_empty() || line.len() as u64 > MAX_BYTES || line.last() != Some(&b'\n') {
            return Err("invalid frame size or framing".into());
        }
        let value: Value = serde_json::from_slice(&line).map_err(|error| error.to_string())?;
        if frame(&value)? != line {
            return Err("expected canonical JSON plus LF".into());
        }
        Ok((value, line))
    }
    fn write(&mut self, value: &Value) -> Result<Vec<u8>, String> {
        let line = frame(value)?;
        let mut output = io::stdout().lock();
        output
            .write_all(&line)
            .and_then(|()| output.flush())
            .map_err(|error| error.to_string())?;
        Ok(line)
    }
    fn call(&mut self, port: &str, input: Value) -> Result<Value, String> {
        self.sequence += 1;
        let call = json!({"kind":"call", "sequence":self.sequence,"port":port,"input":input});
        let line = self.write(&call)?;
        self.transcript.extend(line);
        let (reply, line) = self.read()?;
        self.transcript.extend(line);
        if reply.get("kind") != Some(&json!("reply"))
            || reply.get("sequence") != Some(&json!(self.sequence))
            || reply.as_object().map(|value| value.len()) != Some(3)
        {
            return Err("invalid reply envelope".into());
        }
        reply
            .get("value")
            .cloned()
            .ok_or_else(|| "missing reply value".into())
    }
}
use std::io::Read;
type Wire = Rc<RefCell<Transport>>;
fn call(wire: &Wire, port: &str, input: Value) -> Result<Value, RuntimePortError> {
    wire.borrow_mut()
        .call(port, input)
        .map_err(RuntimePortError::new)
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, RuntimePortError> {
    serde_json::from_value(value).map_err(|error| RuntimePortError::new(error.to_string()))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    kind: String,
    protocol: String,
    request: AgentRunRequest,
    support: Support,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Support {
    policies: Vec<PolicyIdentity>,
    capabilities: Vec<CapabilityIdentity>,
    tool_schema_digests: Vec<String>,
    permission: String,
    executor: ExecutorIdentity,
    verifier_ids: Vec<String>,
    max_effects: u64,
}
struct Agent {
    wire: Wire,
    clock: u64,
}
impl Agent {
    fn tick(&mut self) -> Result<ResourceUsage, RuntimePortError> {
        let now: u64 = decode(call(&self.wire, "clock.now", json!({}))?)?;
        let elapsed = now
            .checked_sub(self.clock)
            .ok_or_else(|| RuntimePortError::new("clock moved backwards"))?;
        self.clock = now;
        Ok(ResourceUsage {
            cost_micros: 0,
            elapsed_ms: elapsed,
            model_tokens: 0,
            tool_calls: 0,
        })
    }
}
impl AgentAdapter for Agent {
    fn plan(
        &mut self,
        request: &AgentRunRequest,
    ) -> Result<PortObservation<PlanProposal>, RuntimePortError> {
        let usage = self.tick()?;
        let plan_digest: String = decode(call(&self.wire, "agent.plan", json!(request))?)?;
        Ok(PortObservation {
            value: PlanProposal { plan_digest },
            usage,
        })
    }
    fn next_effect(
        &mut self,
        context: AgentRunContext<'_>,
    ) -> Result<PortObservation<AgentStep>, RuntimePortError> {
        let usage = self.tick()?;
        let value = call(
            &self.wire,
            "agent.next",
            json!({"subject":context.current_subject,"usage":context.usage,"effects_seen":context.effects_seen}),
        )?;
        let step = match value.get("step").and_then(Value::as_str) {
            Some("effect") => AgentStep::ProtectedEffect(Box::new(ProtectedEffectProposal {
                request: decode(value["request"].clone())?,
            })),
            Some("finish") => AgentStep::Finish,
            Some("cancel") => AgentStep::Cancel(Cancellation {
                actor_id: Some("fixture-host".into()),
                reason: "cancelled".into(),
                evidence: decode(value["evidence"].clone())?,
            }),
            Some("error") => return Err(RuntimePortError::new("scripted agent failure")),
            _ => return Err(RuntimePortError::new("unsupported agent step")),
        };
        Ok(PortObservation { value: step, usage })
    }
}
struct Executor(Wire);
impl ExecutorPort for Executor {
    fn execute(
        &mut self,
        request: &ProtectedEffectRequest,
    ) -> Result<ExecutorObservation, RuntimePortError> {
        let value = call(&self.0, "executor.execute", json!(request))?;
        if value.get("error").is_some() {
            return Err(RuntimePortError::new("scripted executor response lost"));
        }
        Ok(ExecutorObservation {
            execution_status: decode(value["execution_status"].clone())?,
            observed_post_effect_subject: decode(value["observed_post_effect_subject"].clone())?,
            exit: decode(value["exit"].clone())?,
            usage: decode(value["usage"].clone())?,
            executor: decode(value["executor"].clone())?,
            sandbox_profile: decode(value["sandbox_profile"].clone())?,
            reason: decode(value["reason"].clone())?,
            evidence: decode(value["evidence"].clone())?,
        })
    }
}
struct Verifier(Wire);
impl VerifierPort for Verifier {
    fn verify(
        &mut self,
        context: VerificationContext<'_>,
    ) -> Result<PortObservation<VerificationReport>, RuntimePortError> {
        let value = call(
            &self.0,
            "verifier.verify",
            json!({"subject":context.current_subject,"usage":context.usage}),
        )?;
        if value.get("error").is_some() {
            return Err(RuntimePortError::new("scripted verifier failure"));
        }
        Ok(PortObservation {
            value: VerificationReport {
                subject_digest: decode(value["subject_digest"].clone())?,
                verifier_id: decode(value["verifier_id"].clone())?,
                verdict: decode(value["verdict"].clone())?,
                evidence: decode(value["evidence"].clone())?,
            },
            usage: ResourceUsage {
                cost_micros: 0,
                elapsed_ms: 0,
                model_tokens: 0,
                tool_calls: 0,
            },
        })
    }
}
pub fn run() -> Result<(), String> {
    let wire = Rc::new(RefCell::new(Transport {
        reader: io::BufReader::new(io::stdin()),
        transcript: vec![],
        sequence: 0,
    }));
    let (value, start_bytes) = wire.borrow_mut().read()?;
    let start: Start = serde_json::from_value(value).map_err(|error| error.to_string())?;
    if start.kind != "start" || start.protocol != VERSION {
        return Err("unsupported runtime protocol".into());
    }
    let permission = match start.support.permission.as_str() {
        "allow" => PolicyDecision::Allow,
        "ask" => PolicyDecision::Ask,
        "deny" => PolicyDecision::Deny,
        _ => return Err("unsupported permission".into()),
    };
    let support = RuntimeSupport::new(
        ContractSupport::new(start.support.policies),
        start.support.capabilities,
        PermissionPolicy {
            policy_decision: Some(permission),
            risk_tags: vec![],
            wrapper_chain: vec![],
            approvals: vec![],
        },
        EffectUsage {
            cost_micros: 10,
            elapsed_ms: 1,
            model_tokens: 0,
            tool_calls: 1,
        },
        vec![],
        start.support.verifier_ids,
    )
    .with_trusted_tool_schemas(start.support.tool_schema_digests);
    let max_cost = start.request.resource_budget.max_cost_micros;
    let config = RuntimeConfig {
        implementer_id: "gaap-runtime".into(),
        executor_identity: start.support.executor,
        max_effects: start.support.max_effects,
        budget_thresholds: BudgetThresholds {
            warn_micros: max_cost,
            approval_micros: max_cost,
            hard_stop_micros: max_cost,
        },
    };
    let request = start.request.clone();
    let mut engine = AgentRunEngine::new(
        support,
        config,
        Agent {
            wire: wire.clone(),
            clock: 0,
        },
        Executor(wire.clone()),
        Verifier(wire.clone()),
    );
    let execution = engine
        .run(start.request)
        .map_err(|error| error.to_string())?;
    verify_terminal_receipt(
        &request,
        &ContractSupport::new(request.policies.clone()),
        &execution.receipt,
    )
    .map_err(|error| error.to_string())?;
    let binding = json!({"request_digest":digest(&start_bytes),"receipt_digest":execution.receipt.receipt_digest});
    let signature = wire.borrow_mut().call("signer.sign", binding)?;
    let mut transport = wire.borrow_mut();
    let response = json!({"kind":"terminal","protocol":VERSION,"request_digest":digest(&start_bytes),"transcript_digest":digest(&transport.transcript),"subject":{"name":"gaap","version":env!("CARGO_PKG_VERSION"),"implementation":"rust"},"receipt":execution.receipt,"effects":execution.protected_effect_results,"signature":signature});
    transport.write(&response)?;
    // After terminal output the only valid host action is EOF. No further work is admitted.
    let mut extra = Vec::new();
    (&mut transport.reader)
        .take(MAX_BYTES + 1)
        .read_to_end(&mut extra)
        .map_err(|error| error.to_string())?;
    if !extra.is_empty() {
        return Err("terminal run refuses additional input".into());
    }
    Ok(())
}

use gaap::runtime::AgentRunExecution;
use gaap_telemetry::{Correlation, QueueConfig, Telemetry, project};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    env,
    error::Error,
    io::{self, Write},
    time::{Duration, SystemTime},
};

fn main() -> Result<(), Box<dyn Error>> {
    let inspection: Value = serde_json::from_reader(io::stdin().lock())?;
    let runs = inspection["runs"].as_array().ok_or("missing native runs")?;
    let [run] = runs.as_slice() else {
        return Err("expected one run in this disposable fixture".into());
    };
    if run["terminal_receipt_verified"] != true {
        return Err("native receipt validation failed".into());
    }
    let execution = AgentRunExecution {
        receipt: serde_json::from_value(run["receipt"].clone())?,
        protected_effect_results: serde_json::from_value(run["protected_effect_results"].clone())?,
        // The submitted-proposal host runs without a static policy adapter.
        static_policy_evaluations: vec![],
    };
    let correlation = Correlation {
        workflow_run: Some("viewer-fixture-workflow".into()),
        action_request: Some("viewer-fixture-action".into()),
        execution_claim: Some("viewer-fixture-claim".into()),
        attempt: Some("viewer-fixture-attempt".into()),
    };
    let args: Vec<_> = env::args().skip(1).collect();
    match args.as_slice() {
        [] => {
            let time = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?;
            let payload = project(&execution, &correlation, time.as_nanos().try_into()?)?;
            io::stdout().lock().write_all(&payload)?;
        }
        [option, endpoint] if option == "--send" => {
            let before = serde_jcs::to_vec(&execution.receipt.body)?;
            if format!("sha256:{:x}", Sha256::digest(&before)) != execution.receipt.receipt_digest {
                return Err("receipt digest mismatch before export".into());
            }
            let effects = serde_json::to_vec(&execution.protected_effect_results)?;
            let mut telemetry = Telemetry::otlp(
                endpoint.parse()?,
                Duration::from_secs(2),
                QueueConfig::default(),
            )?;
            let submission = telemetry.observe(&execution, &correlation);
            let shutdown = telemetry.shutdown(Duration::from_secs(4));
            let stats = telemetry.statistics();
            let after = serde_jcs::to_vec(&execution.receipt.body)?;
            println!(
                "{}",
                json!({
                    "receipt_digest": execution.receipt.receipt_digest,
                    "receipt_body_bytes": before.len(),
                    "receipt_bytes_unchanged": before == after,
                    "effect_result_bytes_unchanged": effects == serde_json::to_vec(&execution.protected_effect_results)?,
                    "submission": format!("{submission:?}"),
                    "shutdown": format!("{shutdown:?}"),
                    "queued": stats.queued, "delivered": stats.delivered, "partial": stats.partial,
                    "failed": stats.failed, "dropped": stats.dropped, "sampled_out": stats.sampled_out,
                })
            );
        }
        _ => return Err("expected no arguments or --send LOOPBACK_IP:PORT".into()),
    }
    Ok(())
}

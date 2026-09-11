use super::*;
use std::{
    io::Read,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub const PROGRAM: &str = include_str!("verify.py");
pub const CASES: &[(i64, f64, Option<f64>, &str)] = &[
    (1, 20.0, Some(27.0), "single item + delivery"),
    (4, 20.0, Some(87.0), "below bulk threshold"),
    (5, 20.0, Some(97.0), "bulk discount before delivery"),
    (5, 25.0, Some(112.5), "discounted total gets free delivery"),
    (1, 100.0, Some(100.0), "exact free-delivery threshold"),
    (6, 19.99, Some(107.95), "rounding to cents"),
    (0, 20.0, None, "zero quantity rejected"),
    (-1, 20.0, None, "negative quantity rejected"),
];

pub fn evaluate(source: &str) -> Result<Value> {
    evaluate_program(source, PROGRAM, Duration::from_secs(5))
}
fn evaluate_program(source: &str, program: &str, timeout: Duration) -> Result<Value> {
    let mut child = Command::new("/usr/bin/python3")
        .args(["-I", "-S", "-B", "-c", program])
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("verifier spawn failed: {e}"))?;
    let payload = json!({"source":source,"inputs":CASES.iter().map(|(q,p,_,_)| json!([q,p])).collect::<Vec<_>>()});
    let input_result = child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes());
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let output = thread::spawn(move || {
        let mut b = Vec::new();
        out.take(32769).read_to_end(&mut b).map(|_| b)
    });
    let errors = thread::spawn(move || {
        let mut b = Vec::new();
        err.take(8193).read_to_end(&mut b).map(|_| b)
    });
    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if started.elapsed() > timeout {
            timed_out = true;
            let _ = child.kill();
            break child.wait().map_err(|e| e.to_string())?;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let bytes = output
        .join()
        .map_err(|_| "verifier output thread failed")?
        .map_err(|e| e.to_string())?;
    let stderr = errors
        .join()
        .map_err(|_| "verifier stderr thread failed")?
        .map_err(|e| e.to_string())?;
    if timed_out
        || !status.success()
        || input_result.is_err()
        || bytes.len() > 32768
        || !stderr.is_empty()
    {
        return Ok(
            json!({"passed":false,"reason":if timed_out {"verifier time limit exceeded"} else {"verifier process failed"},"elapsed_ms":started.elapsed().as_millis() as u64,"cases":[]}),
        );
    }
    let observed: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid verifier response: {e}"))?;
    let cases: Vec<Value> = CASES.iter().enumerate().map(|(index,(q,p,expected,name))| {
        let actual = &observed["observations"][index];
        let passed = match expected {
            Some(expected) => actual["value"].as_f64().is_some_and(|v| (v-expected).abs() < 0.00001),
            None => actual["error"] == "ValueError",
        };
        json!({"name":name,"quantity":q,"unit_price":p,"expected":expected.map_or(json!("ValueError"), |n|json!(n)),"observed":actual,"passed":passed})
    }).collect();
    Ok(
        json!({"passed":cases.iter().all(|c|c["passed"]==true),"cases":cases,"validation_error":observed["validation_error"],"elapsed_ms":started.elapsed().as_millis() as u64,"verifier":"independent-shipping-oracle","execution_policy":"bounded arithmetic function; no imports, attributes, loops, or arbitrary calls"}),
    )
}

pub struct ShippingVerifier {
    pub store: CodingStore,
    pub report: Rc<RefCell<Value>>,
}
impl VerifierPort for ShippingVerifier {
    fn verify(
        &mut self,
        context: VerificationContext<'_>,
    ) -> std::result::Result<PortObservation<VerificationReport>, RuntimePortError> {
        let start = Instant::now();
        let result = (|| -> Result<_> {
            let before = self.store.snapshot()?;
            let observed_digest = snapshot_digest(&before)?;
            let mut report = evaluate(&before["shipping.py"].content)?;
            let after = self.store.snapshot()?;
            let fresh = before == after && observed_digest == context.current_subject.digest;
            report["subject_digest"] = json!(observed_digest);
            report["fresh"] = json!(fresh);
            report["passed"] = json!(report["passed"] == true && fresh);
            let output_digest = hash_json(&report)?;
            *self.report.borrow_mut() = report.clone();
            Ok(PortObservation {
                value: VerificationReport {
                    subject_digest: observed_digest.clone(),
                    verifier_id: "independent-shipping-oracle".into(),
                    verdict: if report["passed"] == true {
                        VerificationVerdict::Pass
                    } else {
                        VerificationVerdict::Fail
                    },
                    evidence: vec![
                        EvidenceReference {
                            evidence_type: EvidenceType::CommandOutput,
                            digest: output_digest,
                            locator: Some("gaap:record/verification".into()),
                        },
                        EvidenceReference {
                            evidence_type: EvidenceType::Artifact,
                            digest: observed_digest,
                            locator: Some("gaap:record/after".into()),
                        },
                    ],
                },
                usage: resources(start.elapsed().as_millis() as u64, 1),
            })
        })();
        result.map_err(RuntimePortError::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verifier_deadline_terminates_the_child() {
        let started = Instant::now();
        let report = evaluate_program("", "while True: pass", Duration::from_millis(50)).unwrap();
        assert_eq!(report["passed"], false);
        assert_eq!(report["reason"], "verifier time limit exceeded");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn dangerous_syntax_and_unbounded_integer_arithmetic_cannot_escape() {
        for source in [
            "import os\ndef shipping_quote(quantity, unit_price):\n    return 0\n",
            "def shipping_quote(quantity, unit_price):\n    return ().__class__\n",
            "def shipping_quote(quantity, unit_price):\n    return round.__globals__\n",
            "def shipping_quote(quantity, unit_price):\n    while True: pass\n",
            "def shipping_quote(quantity, unit_price):\n    return 2 ** 9999\n",
        ] {
            assert!(evaluate(source).unwrap()["validation_error"].is_string());
        }
        let report=evaluate("def shipping_quote(quantity, unit_price):\n    if quantity <= 0:\n        raise ValueError\n    return quantity * unit_price\n").unwrap();
        assert!(report["validation_error"].is_null());
    }
}

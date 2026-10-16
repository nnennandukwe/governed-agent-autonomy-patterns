#!/usr/bin/env python3
"""Compile real runtime mutants in a disposable copy; never edit the caller's source."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
MUTANTS = {
    "execute_after_ask_or_block": ("if permission_decision.outcome != Outcome::Allow {", "if permission_decision.outcome != Outcome::Allow { let _ = self.executor.execute(&proposal.request);"),
    "transition_after_terminal": (
        "ledger.transition(terminal_status, Some(reason.clone()));",
        "ledger.transition(terminal_status, Some(reason.clone()));\n        ledger.transition(AgentRunStatus::Planning, None);"),
    "duplicate_terminal_receipt_event": (
        "ledger.transition(terminal_status, Some(reason.clone()));",
        "ledger.transition(terminal_status, Some(reason.clone()));\n        ledger.transition(terminal_status, Some(reason.clone()));"),
    "stale_verification_becomes_current": (
        "let report = report_observation.value;",
        "let mut report = report_observation.value; report.subject_digest = ledger.current_subject.digest.clone();"),
    "self_verification_becomes_independent": (
        "let report = report_observation.value;",
        'let mut report = report_observation.value; report.verifier_id = "independent".into();'),
    "failed_verification_becomes_pass": (
        "let report = report_observation.value;",
        "let mut report = report_observation.value; report.verdict = VerificationVerdict::Pass;"),
    "usage_disappears": ("self.usage = next_usage;", "self.usage = zero_resource_usage();"),
    "changed_content_keeps_digest": ("Ok(digest) => digest,", 'Ok(_) => format!("sha256:{}", "0".repeat(64)),'),
    "unknown_effect_silently_continues": (
        'Some((AgentRunStatus::Failed, "protected_effect.unknown_outcome"))', "None"),
}


def main():
    target = ROOT / "target"
    target.mkdir(exist_ok=True)
    logs = target / "state-machine-mutants"
    logs.mkdir(exist_ok=True)
    environment = dict(os.environ, CARGO_TARGET_DIR=str(target / "state-machine-mutant-build"), CARGO_PROFILE_DEV_DEBUG="0",
                       CARGO_PROFILE_TEST_DEBUG="0", CARGO_INCREMENTAL="0", GAAP_PROPTEST_PROFILE="ci")
    environment.pop("GAAP_PROPTEST_REPLAY", None)
    environment["GAAP_PROPTEST_SEED"] = "18020261015"
    command = ["cargo", "test", "--locked", "--test", "state_machine",
               "generated_agent_run_histories", "--", "--nocapture"]
    with tempfile.TemporaryDirectory(prefix="gaap-18-mutants-") as directory:
        checkout = Path(directory)
        for name in ["Cargo.toml", "Cargo.lock", "src", "tests", "examples", "rust-toolchain.toml"]:
            source = ROOT / name
            if source.is_dir():
                shutil.copytree(source, checkout / name, ignore=shutil.ignore_patterns("target", "__pycache__"))
            else:
                shutil.copy2(source, checkout / name)
        source = checkout / "src/runtime.rs"
        original = source.read_text()
        # Infrastructure/build failures don't count as detected semantic mutants.
        baseline = subprocess.run(command, cwd=checkout, env=environment, capture_output=True, text=True, timeout=300)
        (logs / "baseline.log").write_text(baseline.stdout + baseline.stderr)
        if baseline.returncode:
            raise SystemExit("mutant baseline failed; inspect target/state-machine-mutants/baseline.log")
        for name, (before, after) in MUTANTS.items():
            if original.count(before) != 1:
                raise SystemExit(f"{name}: mutation anchor must match exactly once")
            source.write_text(original.replace(before, after))
            result = subprocess.run(command, cwd=checkout, env=environment, capture_output=True, text=True, timeout=300)
            output = result.stdout + result.stderr
            (logs / f"{name}.log").write_text(output)
            if result.returncode == 0 or "GAAP_MINIMIZED_HISTORY=" not in output or "error[E" in output:
                raise SystemExit(f"{name}: survived or couldn't execute; inspect target/state-machine-mutants/{name}.log")
            print(f"DETECTED {name}", flush=True)
            for line in output.splitlines():
                if line.startswith(("GAAP_PROPTEST_SEED=", "GAAP_MINIMIZED_HISTORY=")):
                    print(line, flush=True)
            source.write_text(original)
        wire_source = checkout / "src/runtime_subject.rs"
        wire_original = wire_source.read_text()
        wire_before = "transport.write(&response)?;"
        if wire_original.count(wire_before) != 1:
            raise SystemExit("duplicate_terminal_envelope: mutation anchor must match exactly once")
        wire_command = ["cargo", "test", "--locked", "--test", "runtime_subject",
                        "runtime_subject_binds_real_port_calls_and_refuses_post_terminal_input", "--", "--nocapture"]
        baseline = subprocess.run(wire_command, cwd=checkout, env=environment, capture_output=True, text=True, timeout=300)
        (logs / "wire-baseline.log").write_text(baseline.stdout + baseline.stderr)
        if baseline.returncode:
            raise SystemExit("wire baseline failed; inspect target/state-machine-mutants/wire-baseline.log")
        wire_source.write_text(wire_original.replace(wire_before, wire_before + "\n    " + wire_before))
        result = subprocess.run(wire_command, cwd=checkout, env=environment, capture_output=True, text=True, timeout=300)
        output = result.stdout + result.stderr
        (logs / "duplicate_terminal_envelope.log").write_text(output)
        if result.returncode == 0 or "test result: FAILED" not in output or "error[E" in output:
            raise SystemExit("duplicate_terminal_envelope: survived or couldn't execute")
        print("DETECTED duplicate_terminal_envelope; replay named runtime_subject fixture", flush=True)
    print(f"Detected {len(MUTANTS) + 1}/{len(MUTANTS) + 1} source mutants", flush=True)


if __name__ == "__main__":
    main()

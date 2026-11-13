# Govern one submitted coding proposal

This executable reference accepts a proposed file replacement over MCP, evaluates it through GAAP's existing engine, independently checks the resulting shipping function and saves the actual effect and terminal receipt. A successful code change can be followed by a denied deployment request.

The reusable [submitted-proposal adapter](../../adapters/submitted-proposal/README.md) contains the one-shot agent port. This example owns the shipping policy, filesystem executor, verifier, host-issued identity, local ledger and inbound MCP transport. Each admitted proposal creates one bounded run. The external conversation remains outside that run.

## Run the reference

Use the repository's pinned Rust 1.96 toolchain and Python 3 on macOS or Linux:

```bash
cargo build --manifest-path examples/submitted-proposal/Cargo.toml --locked
examples/submitted-proposal/target/debug/gaap-submitted-proposal-reference coding-init /tmp/my-new-proposal-session
examples/submitted-proposal/target/debug/gaap-submitted-proposal-reference coding-serve /tmp/my-new-proposal-session
```

The last command serves MCP JSON-RPC on stdin/stdout. Attach an MCP client instead of typing into that stream. `coding-inspect ROOT` prints the validated saved history; `coding-submit ROOT` accepts one JSON object on stdin and prints its result. `--help` lists these commands. Initialization refuses an existing root.

The server exposes `read_project`, `submit_change` and `run_status`. Read the project first, then supply its exact subject digest:

```json
{
  "path": "shipping.py",
  "content": "def shipping_quote(quantity, unit_price):\n    return 0\n",
  "plan": "Replace the shipping function and check the task's cases.",
  "base_digest": "sha256:replace-with-the-exact-read-project-digest"
}
```

This sample function fails verification after its write executes. Inspect `protected_effect_results`, `verification` and `receipt.body.terminal_status` separately. A blocked completion never implies rollback. Requests to write `deployment.json` or another path receive a denied effect without changing that file.

Only those four input fields are accepted. The caller can't submit an approval, run ID, policy decision, verifier identity, completion status or ThreadLoop claim. The host creates separate run/request IDs from 256 bits of OS randomness. The local approval describes host policy over the exact observed subject. These IDs provide correlation; this reference's local policy and receipt hashes don't authenticate a remote principal.

The capability identity hashes the advertised descriptors, including schemas and descriptions. A changed descriptor or policy invalidates the saved session's trust before duplicate lookup. Existing roots are preserved for inspection; there's no automatic trust refresh or session relocation.

## Delivery and recovery

| Condition | Behavior |
| --- | --- |
| Exact duplicate | Return the saved historical receipt with `duplicate: true`; no write or verification replay. |
| Concurrent requests | A session lock serializes admission; a different proposal with a stale base is rejected. |
| Extra fields or stale subject | Reject before the in-flight marker or any effect. |
| Caller cancellation, disconnect or ten-second deadline | Request cooperative interruption; retain any actual executed effect and drain the blocking worker into the saved ledger. |
| Verification failure | Keep the actual file and executed effect; block completion. |
| Admission marker directory sync fails | Clear the marker only while execution is known never to have started. Failed abort publication preserves the visible state for inspection. |
| Lost effect visibility or an interrupted result publication | Keep the in-flight marker and refuse further dispatch; inspect the files and retain the session. |

The current lock wait drains on acquisition; cancellation and deadline checks stop work after that wait. The verifier has its own five-second child-process deadline. The run allows one effect and two accounted calls. Twelve saved runs bound a session. These limits describe host operations and verification. `model_usage: null` marks external model tokens and cost as unavailable.

The subject binds the three fixture files and their local filesystem identities. The writer stages a replacement, rechecks the snapshot and replaces the target. This assumes controlled local writers; it doesn't isolate arbitrary agent system calls. The bounded Python verifier accepts restricted arithmetic syntax. See [the commit-point and fault checks](TRANSACTION.md).

## Deterministic acceptance

```bash
cargo test --manifest-path examples/submitted-proposal/Cargo.toml --locked
python3 -B -m unittest discover -s examples/submitted-proposal/tests -p 'test_*.py'
```

The Rust tests migrate the conference coding regressions: gated writes, failed verification with retained effects, stale subjects, schema drift, concurrent duplicates, filesystem identity checks and publication failures. The independent Python client uses JSON-RPC directly and checks discovery, completed and denied proposals, restart readback, forged fields, concurrent delivery, cancellation, disconnect and timeout.

## Deterministic Goose acceptance

```bash
python3 examples/governed-goose/install_goose.py /tmp/my-new-goose-1.54.0
python3 -B examples/submitted-proposal/check_goose.py \
  --goose /tmp/my-new-goose-1.54.0/goose \
  --output /tmp/my-new-submitted-goose-evidence
```

The runner checks the existing Goose 1.54.0 binary pin, uses a disposable profile and exposes only the three GAAP tools. Its local provider emits deterministic tool calls over the OpenAI-compatible protocol. Real Goose reads the subject through MCP, submits a passing shipping implementation, rereads the subject and submits a denied deployment change. The retained provider transcript, Goose output and host ledger support this client-interoperability result. There's no model inference or paid provider call. Live-model acceptance remains unverified.

The required `submitted_proposal` CI lane runs both client paths and the locked Rust checks. Local conference files, inspector work and saved sessions remain in the original checkout. This example uses a new path and session schema, with no migration of those artifacts. Durable crash continuation and reconciliation remain #32; authenticated receipt exchange remains the attestation/ThreadLoop work; stronger runtime isolation remains the sandbox profiles.

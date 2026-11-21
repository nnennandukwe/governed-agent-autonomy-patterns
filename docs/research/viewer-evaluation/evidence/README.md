# Look up a trace in native GAAP evidence

Both viewers received a trace for one synthetic shipping change. Its
`gaap.receipt.digest` matched the receipt that `coding-inspect` validated.
The evidence card reads the inspection output, shows the actual applied diff
and lists each verifier observation. The native ledger stays with its session.
[Captured card](paired-receipt.png) · [Measured identifiers](measurement.json)

The host supplied four synthetic ThreadLoop identities for this trace. The
projector hashes them through the same adapter as the other eight fixtures.
The receipt and effect digests come from the engine. A fresh session binds its
own filesystem identities and verifier seed, so its digests can differ.

After both viewer restarts, all five native files matched their earlier SHA-256
hashes and a fresh `coding-inspect` read returned identical bytes. The real
adapter recorded one delivered batch with Jaeger, one partial acknowledgement
with Aspire and one failed delivery on a local port without a listener. The
5,821 canonical receipt body bytes and serialized effect results matched before
and after each call. The fixture card returned 200 for its digest and 404 for
an unknown digest. Its port refused connections after shutdown.

## Record a fresh fixture

Run these commands from the repository root with Rust, Python 3 and `jq`.
Use a fresh private directory for the session and native output. The host's
existing shipping fixture provides the proposal. Provision the locked crates
before offline playback.

```bash
scratch=$(mktemp -d)
repo=$(pwd -P)
evidence="$repo/docs/research/viewer-evaluation/evidence"
export CARGO_TARGET_DIR="$scratch/build"
cargo build --locked --manifest-path examples/submitted-proposal/Cargo.toml
host="$CARGO_TARGET_DIR/debug/gaap-submitted-proposal-reference"
"$host" coding-init "$scratch/session" > "$scratch/initial.json"
python3 - "$scratch/initial.json" > "$scratch/proposal.json" <<'PY'
import json, runpy, sys
from pathlib import Path

initial = json.loads(Path(sys.argv[1]).read_text())
source = runpy.run_path("examples/submitted-proposal/check_goose.py")["GOOD"]
print(json.dumps({
    "path": "shipping.py", "content": source,
    "plan": "Correct the shipping quote and verify the fixture cases.",
    "base_digest": initial["subject_digest"],
}))
PY
"$host" coding-submit "$scratch/session" < "$scratch/proposal.json" > "$scratch/submitted.json"
"$host" coding-inspect "$scratch/session" > "$scratch/inspected.json"
```

Build the fixture projector with the exact dependency lockfile used for the measured run. The
standalone scratch crate keeps this research helper outside the GAAP build.

```bash
mkdir -p "$scratch/project/src"
cp "$evidence/project-paired.rs" "$scratch/project/src/main.rs"
cp "$evidence/projector.lock" "$scratch/project/Cargo.lock"
cat > "$scratch/project/Cargo.toml" <<TOML
[package]
name = "gaap-viewer-fixture"
version = "0.0.0"
edition = "2024"
publish = false
[dependencies]
gaap = { path = "$repo" }
gaap-telemetry = { path = "$repo/adapters/telemetry" }
serde_json = "=1.0.151"
serde_jcs = "=0.2.0"
sha2 = "=0.10.9"
[workspace]
TOML
cargo build --locked --manifest-path "$scratch/project/Cargo.toml"
projector="$CARGO_TARGET_DIR/debug/gaap-viewer-fixture"
"$projector" < "$scratch/inspected.json" > "$scratch/paired.json"
```

## Verify the link

Read the digest from the projected trace and use it to select the native run.
An empty result means the digest doesn't match a receipt in this session.

```bash
digest=$(jq -er '.resourceSpans[0].scopeSpans[0].spans[0].attributes[] |
    select(.key == "gaap.receipt.digest") | .value.stringValue' "$scratch/paired.json")
"$host" coding-inspect "$scratch/session" |
    jq -e --arg d "$digest" '.runs[] | select(.receipt.receipt_digest == $d) |
    {digest: .receipt.receipt_digest, terminal_receipt_verified, model_usage,
     cases: (.verification.cases | length), passed: .verification.passed}'
"$host" coding-inspect "$scratch/session" |
    python3 "$evidence/serve_card.py" "$digest"
```

The last command serves one read-only page until Ctrl-C. Open
`http://127.0.0.1:19490/receipts/<64-hex-digest>` using the digest's hex
portion. An unknown digest returns 404. This fixture endpoint accepts requests
from local processes without authentication. The page contains the synthetic
diff, case outcomes and a digest check. The native session has no verified
signature and records model cost and token usage as unavailable.

To test a provisioned loopback receiver, run
`"$projector" --send 127.0.0.1:PORT < "$scratch/inspected.json"` in a second
shell with the same paths. The helper calls the actual telemetry adapter,
drains the queue and prints its delivery counters. It compares the canonical
receipt body and serialized effect results before and after the call. Repeat
the native digest lookup after restarting the viewer to check its independence
from the viewer's retention.

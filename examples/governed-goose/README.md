# Governed Goose reference

This reference runs pinned Goose 1.54.0 as the real ACP agent. One GAAP Agent Run
owns the whole conversation. Goose proposes a shipping-code repair, a fixed
terminal observation and a local MCP observation. Every proposal waits for an
exact Protected Effect Request before the host's separate MCP executor acts.
An independent arithmetic verifier checks the changed source and host-owned
expected results before completion.

The provider is a local deterministic HTTP fixture. It emits tool calls without
model inference, credentials or external billing. This proves protocol and gate
behavior; it isn't an evaluation of a model's coding ability.

## Reproduce

Requires Rust 1.88+, Python 3.9+, Git and either macOS arm64 or Linux x86_64.
Run from the repository root:

```sh
python3 examples/governed-goose/install_goose.py /tmp/gaap-goose-1.54.0
python3 examples/governed-goose/run.py \
  --goose /tmp/gaap-goose-1.54.0/goose \
  --output /tmp/gaap-goose-evidence
```

Both destination directories must be new. The installer checks the release
archive and executable against fixed SHA-256 pins. It doesn't install globally.
The runner checks the executable pin again, builds with the committed lockfile
and creates disposable Git repositories containing public fixture code. It
retains those repositories for inspection; paths appear in the output evidence.

Nine cases produce JSON transcripts, sealed receipts, full protected effect requests and results, raw proposal inputs,
ACP trace hashes, MCP call evidence and independent verification output:

| Case | Expected outcome |
| --- | --- |
| allow | Three admitted effects; independently verified completion |
| ask | One-shot blocked; no effects |
| block | Permission denied; no effects |
| bad_patch | Three effects remain recorded; independent verification fails |
| report_only | Agent's completion claim fails without host effects |
| cancel_before_dispatch | Interrupted, zero effects, with the pending refusal retained |
| unreported, unreported_directory, unreported_mode | File creation, an empty directory or a permission change outside the admitted requests blocks completion |

The host supplies a fixture subject approval. That approval isn't a human
approval record and can't authorize a changed subject. Permission decisions
remain host-owned for each proposal. All calls use the same discovered MCP
capability, with full raw operation and content bound into its input digest.
The MCP operation family describes the dispatch boundary; file and terminal
work happen inside the explicitly approved local backend.

## Pinned execution profile

ACP uses official SDK 3.3.0 and stable protocol v1. Its `acp.goose-proposals` profile identity binds the approved agent and ingress launches; the separate MCP capability binds protected effects. Goose's vendor method
`_goose/unstable/session/extensions/list` is explicitly pinned to Goose 1.54.0;
it isn't a portable ACP capability. `session/new` supplies a full
`_meta.enabledExtensions` list containing only the proposal ingress. The host
checks Goose's returned extension configuration before it releases the prompt.
The deterministic provider also rejects any tool list other than that ingress.
Developer, terminal delegation, filesystem delegation and ambient extensions
aren't enabled. The ingress uses a private local socket and can only submit
proposals; the separate official MCP client owns effect dispatch.

MCP uses official SDK 3.5.1 and protocol 2025-11-25. The independent server rejects
Goose's newer `server/discover` probe instead of silently adopting another
lifecycle. The approved backend exposes one fixed descriptor and three bounded
operations. Agent content never becomes a command or arbitrary file path.

The shipping verifier carries forward the conference's restricted Python AST
validator and eight host-owned cases. Imports, attributes, loops and arbitrary
calls are rejected. A bounded snapshot of the whole working tree, excluding `.git` and including directory entries and permission bits, must match both the pre-check
snapshot and the engine's current subject. Terminal and MCP marker files are
also checked independently of the agent's completion message.

## Evidence limits

The default stdio profile trusts pinned local executable bytes and serialized
host files. It doesn't provide OS isolation or prevent a malicious trusted
process from using unrelated syscalls. The runner owns a new process group and kills it on host timeout. ACP cancellation reaps Goose; arbitrary
child-process termination and interrupted external effects still need host
reconciliation. The ingress has its own finite socket deadline. The host uses an absolute proposal-read deadline capped by the run budget. Refusals return a structured no-dispatch response to the ingress before shutdown; a successful write isn't an acknowledgement from Goose.

External model spend isn't estimated as zero: there's no external model in this
fixture. Using a paid provider would need independent host billing and token
measurement. Production isolation (#17/#21), durable recovery (#32) and
ThreadLoop launch, independent admission and persistence (#111) remain separate from this reference.

The unreported-mutation case is deliberate fault injection: after all three
admitted effects, the provider fixture writes an additional repository file, creates an empty directory or changes permissions
without a Protected Effect Request. The shipping calculations still pass, but
the independent verifier rejects the changed subject. This tests completion
admission after an actual bypass; it doesn't claim to prevent the bypass.

An ACP dispatch claim records admission at the host boundary. It isn't execution
proof: the MCP executor can still refuse on its own repeated checks. Its
`dispatched` evidence, executor observation and sealed result report attempts;
claims don't create execution events or usage.

Ingress refusal messages report only proven non-dispatch. The sealed receipt and
effect result provide the actual gate reason and authority outcome; the runner
checks those fields for Ask and deny. A case label never becomes an authority
decision. Pending proposals stay owned until dispatch admission succeeds, so
binding failures and cancellation still get a no-dispatch response during cleanup.

Cancellation can close the peer before a refusal is written. The host still
retains the refusal with `written: false`. That delivery failure doesn't erase
the zero-dispatch outcome or imply that Goose received the message.

## Optional local OCI profile

Select `--oci` on the runner to execute the same outcome matrix with Docker-compatible Linux containers. Install the immutable image first using the [sandbox guide](../../adapters/sandbox/README.md). The selected engine, runc version, image, limits, programs and mount profile are bound into host adoption; a missing or mismatched runtime fails instead of falling back.

Each protected operation receives one preexisting writable subject file. The arithmetic verifier receives only a read-only shipping file. The signed archive retains original sandbox observations, stdout/stderr and output artifacts; Protected Effect Results reference their execution observations. The host's oracle and evidence remain outside writable mounts. This is a reproducibility boundary, and it doesn't make ordinary containers sufficient isolation for hostile code.

## Signed fixtures and offline verification

Each run now retains portable snapshot manifests, original file bytes, effect inputs, tool schemas, MCP outputs and verifier output alongside its unsigned bundle. The runner builds the [attestation adapter](../../adapters/attestation/README.md), emits a separate `CASE-signed` directory and verifies it in a new process. The directory contains `envelope.json`, content-addressed `blobs`, explicitly named fixture trust/policy files and the offline verification result. Its signing key is public test data; don't configure it as production trust.

The first signed subject is the final observed workspace. The receipt still reports the last admitted subject; unreported changes keep those digests different and remain blocked. Failed and interrupted runs also keep their status after signing. Offline verification doesn't rerun the shipping oracle or authorize ThreadLoop lifecycle progression.

## Binding a controller's request

A controller needs the freshly observed workspace and MCP capability before it can bind its own request and run IDs.
The reference host exposes those fixed parameters as a canonical Agent Run Request:

```bash
gaap-governed-goose --describe CONFIG.json > template.json
```

Discovery launches the configured MCP backend for inventory, reads the portable workspace snapshot and exits. It doesn't
start Goose, open the proposal socket or produce a Terminal Run Receipt. The reference runner checks that discovery
leaves the workspace and provider transcript unchanged.

The trusted host configuration can then include an optional `agent_run_request` object. Only `request_id` and `run_id`
may differ from the freshly discovered template. The host validates the full request against its selected contract
support and compares every other canonical field before starting the conversation. A changed subject, capability,
policy, task, constraint, approval, budget or verification requirement is rejected. Existing configurations keep their
case-derived identities. Discovery always returns the fresh template, even when the configuration still contains a
previously bound request; executing that stale request still fails.

The runner reproduces the original nine outcome cases plus an additional completed run with caller-selected identities.
Each run first checks discovery, and the extra run signs and verifies its own output like the original cases. This
supports [ThreadLoop #111](https://github.com/nnennandukwe/threadloop/issues/111); ThreadLoop still owns claim fencing,
signature trust, durable evidence admission and its lifecycle guards. The reference remains a fixed three-operation task;
[GAAP #31](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/31) owns reusable submitted proposals.

The OCI profile budgets ten seconds per execution and two seconds for cleanup.
Its MCP response budget includes cleanup and publication; the host allows twenty
seconds per MCP call and 120 seconds for the run's three effects, independent
verification and launch checks. The outer runner allows 150 seconds so the host
can retain its terminal evidence. The ordinary stdio profile keeps its existing
40-second run budget. No timeout retries a protected effect.

Development builds optimize SHA-256 for launch-byte checks. They still rehash
all approved bytes before launch; the existing setup deadlines stay enforced.

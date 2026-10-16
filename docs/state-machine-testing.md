# Generated Agent Run histories

GAAP owns one bounded Agent Run and its protected effects. These tests drive the
real `AgentRunEngine` through its agent, executor and verifier ports, then compare
its observations and sealed receipts with an independent state model. They cover
sequences that hand-written examples don't combine reliably.

`tests/state_machine/model.rs` uses its own state and outcome enums, arithmetic,
subject revisions and identity set. It doesn't import GAAP types, validators,
coordinator decisions or transition helpers. `tests/state_machine/sut.rs` translates
history inputs into accepted v0.1 requests and host observations. The assertions in
`tests/state_machine.rs` compare both sides. Sharing input enums doesn't share an
oracle. The adapter uses JCS and SHA-256 directly to check content bindings, without
calling production sealers or validators for expected digests.

The whole-run API owns lifecycle advancement internally. Histories prescribe
external observations; they can't instruct the engine to enter a lifecycle state.
The oracle predicts accepted, planning, executing, awaiting-authority, verifying
and terminal states, and compares the emitted transition sequence. Operations left
after cancellation, failure or Finish must never reach another callback. The
existing `runtime_subject` process test separately checks exactly one terminal
output envelope and rejection of additional input.

## Run and replay

From the repository root:

```bash
cargo test --locked --test state_machine
python3 -B -m unittest discover -s scripts -p 'test_state_machine_mutants.py'
python3 scripts/test-state-machine-mutants.py
GAAP_PROPTEST_PROFILE=deep cargo test --locked --test state_machine generated_agent_run_histories -- --nocapture
```

The ordinary profile uses seed `18020261015`, 256 cases and 1-32 operations per
history. The deeper profile uses 10,000 cases and 1-256 operations. Both bound
shrinking to 100,000 iterations without a wall-clock cutoff. Change the seed with
`GAAP_PROPTEST_SEED=123`. Unknown profile names and malformed seeds fail visibly.
The dependency graph and seed are pinned; the generator isn't stable across
Proptest versions. Serialized histories remain the portable reproduction artifact.

On failure, CI prints `GAAP_PROPTEST_SEED` and `GAAP_MINIMIZED_HISTORY`. Proptest
shrinks operations and their parameters. A final deletion pass reaches a fixed
point where removing any single operation no longer preserves the failure. This
is local minimality; it doesn't prove a globally shortest history. Save the JSON
after `GAAP_MINIMIZED_HISTORY=` as `history.json`, then run:

```bash
GAAP_PROPTEST_REPLAY=history.json cargo test --locked --test state_machine generated_agent_run_histories -- --nocapture
```

Every generated history includes its explicit final Finish input in serialized
operations. The model and adapter never invent a passing Finish on exhaustion.
Shrinking preserves a history that reaches a terminal state; malformed incomplete
replays fail with an input diagnostic.

A replay bypasses generation and executes exactly that serialized input. Keep any
newly found runtime defect as a named test and committed JSON fixture before fixing
it. Failure logs include the expected state and observed status, usage, subject and
executor calls. The checked-in `duplicate-stale-subject.json` preserves a discovered
two-proposal failure: stale-subject refusal must precede duplicate/max-effect
admission so the result satisfies the frozen drift-denial contract.

[Proptest's official guide](https://proptest-rs.github.io/proptest/proptest/state-machine.html)
describes the separate-model approach. We checked `proptest-state-machine` too;
GAAP's synchronous whole-run API suits a Proptest vector strategy without adding a
step API or a second state-machine dependency. Proptest 1.10.0 supports Rust 1.84,
within this crate's declared minimum. No runtime dependency was added.

## Acceptance and evidence

| Requirement | Executable evidence |
| --- | --- |
| Ask/block never dispatch; post-terminal operations stop | Oracle compares executor calls, external-attempt trace, next-step count and close count; explicit permission fixtures |
| Terminal states stay terminal; one terminal receipt | Exact transitions, one terminal event, no later events; existing separate-process terminal-envelope fixture |
| Completion uses independent current verification and latest subject | All eight verifier observations crossed with all ten effect outcomes; subject and verifier callback assertions; every terminal reason code; agent callback subject/usage contexts |
| Usage never disappears | All four cumulative axes, event monotonicity, reservation refusal, actual overage and exhaustion fixtures |
| Identity binds canonical content | Independent JCS hashes for request/result/receipt; changed input under reused identity is refused |
| Unknown effects don't retry; denied/unverified work can't complete | Unknown, lost-reply, denial and verification-failure histories with later work left pending |
| Shrinking and replay work | Mutant failures must emit minimized JSON; committed regression replays the discovered runtime defect |

Reservations are the existing trusted pre-dispatch usage estimate, not a new
reservation API. Permission is fixed for a run; the accepted one-shot Ask behavior
terminalizes as blocked. No approval-resume operation has been invented. Failed
executor responses preserve the known minimum tool attempt. Timeout is supplied as
an interrupted host observation; the model doesn't simulate wall-clock scheduling.

## Source mutants

The Python command copies source and tests into a disposable directory, checks the
baseline, then changes one source expression at a time. Its separate build directory
prevents mutated artifacts from entering ordinary tests. It never edits the caller's
source. Compile failures, timeouts and missing anchors don't count as detections.
Logs remain in `target/state-machine-mutants/`. Each Cargo command has a 180-second
limit within one 420-second suite budget, leaving time for preceding tests in the
10-minute runtime lane. CI records a shared deadline before the first runtime
Cargo command. The runner uses the smaller of its 420-second limit and the time
remaining before that deadline, reserving 60 seconds for job overhead/cleanup.
An exhausted budget fails the proof rather than counting
unfinished mutants as detected. The POSIX runner stops the whole owned process
group before collecting output on timeout; its Python regression leaves a child
holding pipes after its launcher exits and confirms bounded cleanup.

The thirteen runtime mutants violate permission admission, terminal immutability,
terminal-event cardinality, verifier freshness, verifier actor independence,
verification verdict, retained usage, content digests, unknown-effect termination and stale subject/usage supplied to the agent, and incorrect verification/effect refusal reasons.
Every runtime mutant must produce a generated, shrunk failing history. A fourteenth
mutant duplicates the terminal wire envelope; its deterministic existing process
fixture must fail and supplies a named test command for replay. The guard isn't a
claim about all possible mutants or mutation score across the repository.

## Seed regression boundaries

The issue's conference persistence regressions are generalized as port observations.
The conference implementation isn't on `main`; these fixtures don't import or modify
that unpublished work.

| Historical seed | Runtime fixture and boundary |
| --- | --- |
| Failed intent persistence prevents dispatch | `failed_intent_persistence_prevents_dispatch`: proven pre-dispatch error, no external attempt |
| Failure before replacement preserves original | `failure_before_replacement_preserves_original`: failed observation retains original subject |
| Failure after replacement may be unknown | `failure_after_replacement_retains_unknown_effect`: changed subject retained with unknown result |
| Failed verification retains mutation | `failed_verification_retains_mutation_and_blocks_completion` |
| Mutation during/after verification invalidates freshness | Two named freshness fixtures use the host completion check |
| Receipt publication fails, uncertainty remains, replay refused | `receipt_publication_uncertainty_refuses_same_run_replay`: host reports unknown effect, subsequent proposals remain unconsumed |
| Post-publication directory-sync warning isn't rollback | `known_publication_with_directory_sync_warning_retains_success`: host's known success retains its changed subject |
| Duplicate/concurrent proposals can't execute again | Changed-content identity fixture exercises sequential duplicate delivery; concurrent delivery remains an integration responsibility |
| Changed descriptor/schema, stale subject, links, relocated identity fail closed | Schema/descriptor/subject fixtures plus `host_link_or_relocated_session_refusal_never_dispatches`; host filesystem/session validation remains external |

The publication-warning and link/session fixtures check what the engine does with
trusted host observations. They don't inject filesystem sync failures, create an
uncertainty marker, prove actual path validation, or test cross-process replay.
Those are explicit host integration coverage gaps. These fixtures preserve the
engine's response without inventing recovery semantics from #32.

## Proof limits

This is sampled sequential exploration, not exhaustive verification or certification.
A fixed seed offers repeatability, not independent discovery on each CI run. Inputs
use small numeric ranges and bounded identity pools; the existing focused tests
retain safe-integer and malformed-contract coverage. Generated histories don't
explore real scheduler interleavings, crashes, persistent recovery, hostile-code
isolation, cryptographic trust or model quality. OCI and adapter integration tests
remain required, including their native denied-access, interruption and lost-response
paths. The local OCI profile remains a reproducibility boundary.

RunInvariant owns the independent public black-box corpus. These tests complement
it and don't import its oracle. ThreadLoop owns outer workflow lifecycle advancement;
none of its states or authority enter this model. Frozen ACP, MCP, effect and receipt
contracts remain unchanged.

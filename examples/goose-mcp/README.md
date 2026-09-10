# Governed agent autonomy: engine and goose demo

This example shows an agent proposing a real file change, GAAP requesting
approval, and an operator approving the exact operation. Goose then performs
the approved change through an MCP server. Changing the file invalidates the
earlier approval. The demonstration makes authority visible at the point where
an agent requests a consequential operation.

There are two explicitly different views:

| View | Executes | Evidence |
| --- | --- | --- |
| **Engine view** | Existing `AgentRunEngine` with deterministic agent, in-memory executor, and verifier ports | Sealed and locally verified Terminal Run Receipt and Protected Effect Results; usage and evidence references are fixtures |
| **Goose view** | Real goose 1.50.0 + OpenAI `gpt-5.4-mini`, calling a Rust MCP server that uses the existing `RunCoordinator` | Actual `release.json` replacement and persistent MCP boundary decision records |

The goose example does not run the full `AgentRunEngine`. Its decision records
are not Terminal Run Receipts. Core contracts, schemas, and runtime behavior are
unchanged. This is an independent example crate with its own pinned lockfile.

## Setup on the conference laptop

Requires Rust 1.96.0, Python 3.9 or later, and an OpenAI API account with access
to `gpt-5.4-mini`. From this directory:

```sh
python3 demo.py build
python3 demo.py engine
python3 install_goose.py
python3 demo.py configure
python3 demo.py doctor
```

The engine command works without a provider key or goose. The installer pins
the [official v1.50.0 release](https://github.com/aaif-goose/goose/releases/tag/v1.50.0)
for Apple Silicon macOS and checks its published archive digest. Existing and
new executable bytes are checked against the digest derived from that verified
archive before execution or receiving the provider key. Installation publishes
a fully synced file atomically without overwriting an existing destination. It installs
under `~/.local/share/gaap-demo/tools/goose-1.50.0/` without changing PATH.

`configure` prompts with terminal echo disabled and saves the key to
`~/.config/gaap-demo/openai-key` with owner-only permissions. Enter the key in
your terminal, never in chat or command arguments. `doctor` checks model access
through the OpenAI API. An accessible model ID is a setup check; a successful
real goose run is the integration check.

The Rust example supports Unix hosts. CI tests the MCP server without goose
or API credentials. The goose launcher and bundled installer target this
Apple Silicon conference setup; other hosts need the appropriate official
v1.50.0 artifact and verification of its published digest.

## Engine view: roughly 90 seconds

```sh
python3 demo.py engine completed
python3 demo.py engine approval-required
python3 demo.py engine stale-verification
python3 demo.py engine completed --json
```

The default terminal view displays ordered events, usage, terminal status, and
reason. `--json` gives the complete receipt and explicitly identifies fixture
evidence. In the approval-required
scenario, the in-memory artifact stays at `1.0.0`. In stale verification, the
artifact changes but completion is blocked. This demonstrates the distinction
between authorizing an effect and authorizing completion.

Execution order: request → plan → authorize operation → execute → account for
usage → independently verify the current subject → authorize completion →
produce a receipt. The verifier has a separate actor identity and reads the
in-memory artifact. This fixture does not establish process isolation or
independently measured model cost.

## Goose view: the live sequence

Use two terminal panes: goose requests in one, operator inspection and approval
in the other. Both panes can use this directory as their working directory.
Choose a fresh session name every time; initialization refuses an existing
path. Use a private parent or sticky `/tmp`; shared non-sticky parents and
ancestors owned by other non-root users are rejected. The commands below assume that `conference-demo-1` does not exist.

```sh
python3 demo.py init /tmp/conference-demo-1
python3 demo.py goose /tmp/conference-demo-1 --text \
  'Read release.json. Propose changing only its version to 1.1.0 using propose_write. Return the request_id and decision, then stop for operator approval.'
python3 demo.py status /tmp/conference-demo-1
```

The file remains at `1.0.0`. Copy the exact `sha256:...` request ID returned by
goose or `status`, then run the operator command:

```sh
python3 demo.py approve /tmp/conference-demo-1 REQUEST_ID
```

This displays a unified diff and the immutable proposal before prompting for
`approve`. Terminal control characters and backslashes are displayed as escapes;
the immutable proposal retains its exact original bytes. The Rust command rechecks the stored proposal and current file
before saving authority. No MCP tool can approve anything.

```sh
python3 demo.py goose /tmp/conference-demo-1 --text \
  'Call apply_change for request_id REQUEST_ID. Then read release.json and report the observed result.'
python3 demo.py status /tmp/conference-demo-1
```

Replace `REQUEST_ID` in both commands with the actual complete ID. `status`
shows `permission.approved_exception`, `executed`, and version `1.1.0`.
Repeating `apply_change` for the consumed ID returns `demo.request_consumed`.

For the audience prediction, ask goose to propose version `1.2.0`, approve
that new ID, then edit the disposable `release.json` in your operator terminal
before applying it. For example, change the version to `9.0.0`. Ask:

> I approved this request. The file has changed since then. What should the
> harness do with the same request ID?

Ask goose to call `apply_change` for that ID exactly once. GAAP's adapter blocks
with `demo.stale_subject`; the operator's `9.0.0` file remains intact. Proposing
different content also creates a different ID and does not inherit approval.

## Automated real rehearsal and network fallback

```sh
python3 demo.py rehearse /tmp/conference-rehearsal-1
python3 demo.py rehearse /tmp/conference-rehearsal-2
```

Each command creates a fresh fixture and makes four genuine model-driven
goose calls. Between calls, the script acts as the operator: it explicitly
approves with `--yes` and makes the later file change. It asserts unchanged
bytes before approval, one observed approved execution, a recorded stale block,
and preservation of the operator edit. A rehearsal passes only under seven
minutes. It does not substitute canned model responses.

Every goose call saves a clearly labeled real transcript, with terminal control
characters and backslashes visibly escaped, under the session's
`transcripts/` directory. A successful rehearsal writes `rehearsal.json` with
duration and request IDs. Keep these beside the operator decision log. If the
venue network fails, display a **recorded real goose run** and state that it was
captured earlier. The deterministic engine view remains runnable offline.

`goose run` uses `--no-profile`, an explicit single MCP extension, six maximum
turns, two maximum identical tool repetitions, and a 90-second process timeout
per call. The provider key is passed to goose only. The MCP server starts with
a cleared environment. Configuration, plugins, data, and session state use a
fresh `GOOSE_PATH_ROOT` outside the fixture. Keyring access is disabled. No
Developer or extension-management extension is loaded. The launcher does not
resume existing goose sessions; goose may still persist internal session data
under its isolated root despite `--no-session`.

## The transferable architecture

- **Planning:** an immutable proposal names the target, original content,
  replacement content, session, policy, and advertised tool schema identity.
- **Permissions:** `RunCoordinator` returns `ask`, `allow`, or `block`; an
  operator approval binds to that exact server-computed proposal ID.
- **Tool trust:** the pinned capability digest derives from the same tool
  descriptors actually returned by MCP `tools/list`. It is checked again at
  application time.
- **Verification:** the file adapter reads the resulting bytes after replacement;
  the engine view separately demonstrates subject-bound independent verification
  and completion authorization.
- **Runtime accountability:** consumed attempts survive restart; decision,
  observed effect, aborted attempt, and uncertain outcome remain distinct.
  The boundary limits proposal size/count and decision-record count. It does
  not meter OpenAI cost or tokens; model usage is explicitly unavailable.

The MCP schemas accept only data: `read_file(path)`,
`propose_write(path, content)`, and `apply_change(request_id)`. Unknown fields
are rejected. Provider and transport types stay in this example. GAAP owns
normalized decisions; the adapter owns its local filesystem transaction.

| Source | Responsibility |
| --- | --- |
| `src/protocol.rs` | MCP schemas, advertised tools, strict argument decoding |
| `src/store.rs` | Immutable proposals, operator state, locking, preconditions, atomic replacement, decision records |
| `src/engine.rs` | Deterministic `AgentRunEngine` walkthrough with real contract validation |
| `src/main.rs` | Rust operator CLI and stdio server |
| `demo.py` | Isolated goose launcher, operator UX, real rehearsal/transcript capture |
| `TRANSACTION.md` | Commit points, fault matrix, and recovery boundaries |
| `TALK.md` | The 25-minute case study and demo narration |

## Scope and recovery

This is a reference implementation for a deliberately restricted tool session.
It is not an operating-system sandbox against another native process running
as the same user. The operator state lives outside the MCP-accessible workspace;
its authority relies on that configured tool boundary. A same-user process can
edit local state. Signed receipts, production isolation, revocation, delegation,
and full ACP orchestration remain separate work.

Only `release.json` is readable/writable through these tools. Traversal,
symlinks, hardlinks, non-regular files, and changed directory identities are
rejected. Cooperating commands share an exclusive filesystem lock; bytes and
file identity (including permission bits) are rechecked immediately before rename.
The replacement preserves the fixture's approved permission bits. A hostile native process
racing that final check is outside this example's guarantee.

The file rename and the decision-log rename are distinct commits. An
`executing` marker is persisted before file dispatch and consumes that request.
A restart with that marker means **uncertain**, not “try again.” `inspect`
provides the current file observation alongside the record. A failed result
write before its snapshot rename can leave an applied file and uncertain
evidence. If only the directory sync fails after the result snapshot is renamed,
the observed status remains visible and the response reports a durability warning. Inspect both before
continuing; create a fresh session for recovery or another rehearsal. There is
no reset command that erases evidence.

These logs are local, unsigned, and owner-writable. A digest verifies content
binding, not signer identity. The fixture has no production release effects.

## Checks

Run the root repository's required checks and, separately:

```sh
cargo fmt --manifest-path examples/goose-mcp/Cargo.toml --check
cargo clippy --locked --manifest-path examples/goose-mcp/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path examples/goose-mcp/Cargo.toml
cargo build --locked --manifest-path examples/goose-mcp/Cargo.toml
python3 -m unittest discover -s examples/goose-mcp/tests -p 'test_*.py' -v
```

Run those commands from the repository root. Root CI runs the example
checks in the required baseline lane. The tests include an actual stdio MCP
client/server exchange, concurrent duplicate application, stale subjects,
forged arguments, denied paths, symlinks, persistence faults, pre/post-commit
failures, and restart with uncertain execution. Provider-backed rehearsal is
separate and requires the local key.

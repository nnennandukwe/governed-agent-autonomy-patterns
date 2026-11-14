# Cedar static authorization adapter

GAAP checks every protected effect before dispatch. This adapter adds local Cedar policies to that check: it evaluates the exact run, approved capability, requested scopes and normalized input, then returns typed evidence to RunCoordinator. The coordinator still checks subject freshness, approval, tool trust, budgets, runtime conditions and independent verification.

Cedar 4.14.1 and its dependency graph are pinned in this separate crate. It requires Rust 1.89; GAAP's repository toolchain is 1.96.0. Cedar SDK types and entity layout stay inside the adapter.

## Inspect the local packs

From the repository root:

```bash
cargo run --manifest-path adapters/cedar/Cargo.toml --locked --example packs -- --help
cargo run --manifest-path adapters/cedar/Cargo.toml --locked --example packs -- --check
```

The second command validates four committed policy packs, rejects source or pin drift, and prints their policies, generated schemas, approved actions, scope boundaries and input preimages as JSON. It uses local files and deterministic fixtures. Its output is an inspection artifact; the embedding host separately approves and activates policy.

| Pack | Exact authority demonstrated |
| --- | --- |
| `packs/filesystem.cedar` | Modify `/workspace/report.txt`, with recursion disabled and an exact input digest |
| `packs/process.cedar` | Run `/usr/bin/git` in `/workspace` with ordered arguments `["status", "--short"]` |
| `packs/network.cedar` | HTTPS to `example.invalid:443`, with the exact method/path input |
| `packs/mcp.cedar` | One `lookup` descriptor, server digest, protocol version and argument object |

Each pack also forbids the blocked subject locator `/workspace/blocked`. The public tests exercise unrestricted explicit denial too. Paths, executable, working directory, protocol, host, port, server, tool and argument changes require new exact authority. Multiple scopes must each allow.

For intentional edits to these demonstration fixtures, review the scope/input definitions in `examples/packs.rs`, then run `--write` explicitly to regenerate policy sources and pins. Preserve saved edits before regenerating. The default inspection and `--check` paths never rewrite files.

## Author and validate a pack

The host supplies approved actions and measured normalized input preimages. For native operations, construct `ApprovedAction::native` with the existing capability identity and operation family/name. For MCP, call `ApprovedAction::mcp(server_digest, protocol, descriptor)` with the exact discovered descriptor. It derives the same capability and input-schema digests as the outbound MCP adapter; the stdio integration test compares them through the public discovery/preparation interfaces.

Use `exact_permit(run_id, &action, &scope, &input_digest)` to render a narrow Cedar permit. Add Cedar `when`/`unless` conditions or explicit `forbid` statements to reduce its authority. `PreparedPack::prepare(name, version, run_id, source, actions)` then:

1. Generates the schema from the approved action inventory and MCP input descriptors.
2. Parses the policy set and validates it against that schema in strict mode.
3. Requires each permit to name the exact run, action and content-addressed resource with equality.
4. Returns separate content-addressed policy and schema identities for review.

Templates, empty policy sets, wildcard permits and incompatible policy/schema combinations fail preparation. A broad `forbid` can narrow authority. Required context is always constructed and schema-checked before evaluation, even when a permit omits a `when` clause. The exact resource identity already binds the whole ordered input, so removing a condition can't grant other arguments or paths.

The first profile supports exact grants. A policy that intentionally covers many resources needs separately reviewed exact permits. This keeps authority reviewable while the canonical effect vocabulary stays frozen.

## Pin and explicitly activate

Preparation has no execution authority. After reviewing the source, generated schema and identities, the host stores both approved identities in its own trusted configuration. Pass those previously approved identities to:

```text
prepared.activate(&approved_policy_and_schema_pins, bound_inputs)
```

`bound_inputs` is an immutable map from input digest to its canonical JSON preimage. A mismatched pin, incorrect preimage hash, oversized input or unsupported numeric value prevents activation. Policy, input and descriptor inventories have bounded sizes. No policy is downloaded, discovered from the environment, or activated by the example command.

Install the returned adapter explicitly with:

```text
runtime_support.with_static_policy_adapter(Arc::new(adapter))
```

Set the host permission policy to `Some(PolicyDecision::Allow)` only when the host intends to delegate static scope checks to this pinned adapter. An unset host policy keeps the existing unknown-action approval request; explicit ask and deny decisions also retain precedence.

Include both identities in the accepted Agent Run Request's existing `policies` list and in `ContractSupport`. Protected Effect Requests must carry the same list. Keep independently approved capabilities and tool-schema pins in `RuntimeSupport`; Cedar approval doesn't replace those pins.

A changed policy source, run identity, approved descriptor or generated schema produces a new pin. Start a fresh bounded run after approval; this adapter has no policy hot-swap or run-resume operation.

## Request mapping and host responsibilities

| Cedar component | Source and binding |
| --- | --- |
| Principal | The canonical effect's `run_id`, checked against the explicitly configured accepted run |
| Action | Content identity of the approved operation, capability and optional MCP descriptor |
| Resource | Typed filesystem/process/network/service scope; identity hashes the action, entire scope and input digest |
| Context | Required run ID, subject locator/digest, effect digest, input digest/canonical JSON, capability, sandbox and policy/schema digests; MCP adds typed arguments |

The input digest represents external normalized input in the frozen contract. The trusted host must measure the same preimage that its executor will use. Native examples hash their displayed JSON directly. MCP uses the existing preimage `{"server": server_digest, "tool": tool_name, "arguments": arguments}`. The adapter checks that MCP server, tool and subject scope match the approved descriptor and the proposed effect.

Filesystem paths and process executable/working-directory paths must be absolute lexical paths without dot segments or wildcard characters. Network destinations are exact protocol/host/port tuples. The executor owns filesystem alias/link checks, connection resolution, actual argument dispatch, sandbox enforcement and measured subject observations. Static policies evaluate the proposed identities before those runtime checks.

The adapter handles each requested scope separately and requires every result to allow. Missing bound input, unknown operation/capability/entity, changed tool schema, wrong policy/schema pins, missing required arguments and Cedar evaluation errors return deny. Any diagnostic error overrides an allow from another policy. Immutable evaluation supports concurrent reads without mutable policy state.

## MCP schema profile

The generated context supports strings, booleans, JCS-safe integers, arrays and nested closed objects. Objects must declare `properties` and `additionalProperties: false`; `required` controls required fields. Titles and descriptions are accepted metadata. Unsupported types, references, unions, validation keywords or open objects prevent preparation, so their semantics can't disappear during translation. Input depth and size are bounded.

Cedar represents arrays as sets. The resource identity and `context.input_json` retain the original ordered JSON, including duplicate elements. Reordered process arguments or MCP arrays therefore can't reuse an exact permit. The host can add conditions on typed MCP argument fields within that exact input authority.

Schema identity includes the mapping version, accepted run, complete approved action/descriptor inventory and generated schema. Same-name MCP input-schema drift changes its capability digest, tool-schema digest and generated schema identity. Description-only descriptor changes also change capability and generated schema identity. The public MCP adapter remains independently responsible for checking current server descriptors before invocation.

## Evidence and unchanged contracts

Both policy and schema pins appear in the existing run/effect `policies` lists. Each evaluated effect produces a provider-neutral `StaticPolicyEvaluation` with its exact effect-request digest, pins, decision, reason, determining policy IDs and errors. `AgentRunExecution.static_policy_evaluations` returns these records for the host to retain beside its effect transcript.

The permission decision's existing `effects` strings contain `static_policy.policy:<digest>`, `static_policy.schema:<digest>` and `static_policy.evidence:<digest>`. Those strings are sealed into the existing Terminal Run Receipt decision event. Denied permission decisions also retain the references in the Protected Effect Result's decisive decision. The evidence digest is JCS/SHA-256 over the complete typed evaluation. Consumers retrieve the retained record, recompute its digest, and compare its request/policy/schema bindings with the receipt and saved request.

Retain these evaluation blobs with the host's other content-addressed evidence, including after denied effects. The host must include them in any external evidence export it promises. The current attestation and submitted-proposal references keep their existing profiles; they don't install Cedar automatically. No frozen v0.1 fields, enums, canonicalization rules or generated artifacts change.

Cedar deny enters the existing permission gate as deny. Cedar allow satisfies static policy; existing unset/ask/deny host policy, dangerous-input approval, subject freshness, trusted capability/schema, budget, runtime and verification checks retain their own decisions. Malformed or unbound adapter evidence is treated as deny by the engine. Correct local inputs, revalidate and approve new pins before starting another run.

## Verification

```bash
cargo fmt --manifest-path adapters/cedar/Cargo.toml --check
cargo clippy --manifest-path adapters/cedar/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path adapters/cedar/Cargo.toml --locked
cargo build --manifest-path adapters/cedar/Cargo.toml --locked
cargo run --manifest-path adapters/cedar/Cargo.toml --locked --example packs -- --check
cargo audit --file adapters/cedar/Cargo.lock
```

The required `cedar` CI lane runs these checks and tests Rust 1.89. Public tests cover scope/input changes, explicit forbids, missing/invalid policy and context, unknown identity/action, incompatible pins/schema, descriptor drift, evaluation errors, concurrent reads and evidence binding. Engine tests check allowed filesystem writes and blocked writes; an independent stdio MCP fixture proves actual allow/deny dispatch using the existing outbound adapter. Existing runtime/state-machine and submitted-proposal suites remain regression checks. Linux OCI/gVisor acceptance stays in its existing hosted lanes.

The API choices follow the [Cedar Rust API](https://docs.rs/cedar-policy/4.14.1/cedar_policy/) and [strict validation guidance](https://docs.cedarpolicy.com/policies/validation.html). [Cedar for Agents](https://github.com/cedar-policy/cedar-for-agents) also supplies an MCP schema generator; this adapter uses its own closed mapping to preserve GAAP's existing capability identities and exact-input resource binding.

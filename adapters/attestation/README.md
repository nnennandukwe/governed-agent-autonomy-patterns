# Offline signed GAAP receipts

This adapter signs a complete run ledger in an [in-toto Statement v1](https://github.com/in-toto/attestation/blob/main/spec/v1/statement.md), carried by a [DSSE v1.0.2 envelope](https://github.com/secure-systems-lab/dsse/blob/v1.0.2/protocol.md). An external controller can verify the signature, frozen GAAP contracts and retained content without GAAP's process memory. This doesn't authorize a controller transition or prove that a trusted signer reported the truth.

The [contribution guide](../../CONTRIBUTING.md#final-review-gate) lists the full local CI matrix. The standalone crate requires Rust 1.88+. Tests also require Node.js for an independent Ed25519/JCS consumer. Dependency versions and the crate's own lockfile are pinned.

```sh
cargo test --manifest-path adapters/attestation/Cargo.toml --locked
cargo run --manifest-path adapters/attestation/Cargo.toml --locked -- --help
```

## Verify a retained fixture

From the repository root:

```sh
cargo run --manifest-path adapters/attestation/Cargo.toml --locked -- verify \
  adapters/attestation/fixtures/goose/allow/envelope.json \
  adapters/attestation/fixtures/goose/allow/fixture-trust.json \
  adapters/attestation/fixtures/goose/allow/fixture-policies.json \
  adapters/attestation/fixtures/goose/allow/blobs
```

The example deliberately configures a **public fixture key**, derived from the publicly documented `[7; 32]` test seed. Don't use that key as production trust. Production hosts supply their own `Signer` and `SignatureVerifier` adapters, key access and explicit supported policy identities. The envelope can't add trust keys or supported policies. `keyid` is an unauthenticated lookup hint; verified evidence reports the fingerprint of the actual configured public key.

The CLI exits nonzero for invalid signatures, versions, contracts, missing bytes or digest mismatches. Successful output preserves `terminal_status`, every effect outcome, physical and ledger subject digests, and unresolved metadata references. `controller_authority` remains `none`. Signature verification doesn't rerun the shipping oracle or establish OS isolation.

## Signed profile

`payloadType` is `application/vnd.in-toto+json`. `_type` is `https://in-toto.io/Statement/v1`. The exact predicate type is:

`https://github.com/nnennandukwe/governed-agent-autonomy-patterns/attestation/terminal-run/v1`

The [versioned schema](schema/terminal-run-v1.json) includes the full Agent Run Request, Terminal Run Receipt, ordered Protected Effect Request/Result pairs, all four frozen contract versions and the host implementation's version/binary digest. Those contracts preserve plan events, policy/capability identities, approvals, budgets, sandbox profiles, independent verification, usage and non-success outcomes. Runtime-only signing details don't change the frozen core schemas.

The first and only in-toto subject binds the **actual final observed repository/artifact snapshot**. The receipt separately retains the last admitted subject. Those can differ after an unreported mutation or uncertain effect. Completion requires equality; signed blocked, failed and interrupted runs retain their original outcome and uncertainty. An external controller must still apply its own expected-request, signer, policy, evidence and lifecycle rules.

Verification authenticates raw DSSE payload/type bytes first. It then rejects duplicate JSON keys and unsupported versions, validates the frozen contracts, correlates ordered effects/decisions/execution/result digests, requires all dispatch gates to allow before execution, checks mutation chains and cumulative usage lower bounds, and resolves the required content. Permission decisions and workflow mutation decisions have different IDs; the frozen ledger validator checks each against its own gate. Run usage includes planning, agent and verifier work, so effect usage isn't required to equal total usage.

## Retained content

Snapshot profile `gaap.working-tree-snapshot/1` hashes RFC 8785 canonical JSON for relative paths, file byte digests, directory entries (including empty directories) and Unix permission bits. It includes the root, excludes `.git`, and rejects symlinks/special files. Bounds are 64 entries beneath the root, depth eight and 8 KiB per file. Local inode, device and launch-path safety identities aren't portable content identities.

Offline verification requires manifests for the initial, each admitted and final observed subject, plus every file's original bytes. It also requires effect-input and requested/observed tool-schema bytes and recorded verifier evidence. All listed supplemental blobs are hash-checked. Other approval references from request contexts or ledger events, protocol, meter and executor references remain signed metadata claims; `unresolved_reference_digests` lists references without retained bytes. Don't interpret that list as independently validated underlying evidence. The adapter never fetches evidence locators or opens workspace paths from a receipt.

Blob lookup uses SHA-256 filenames under a host-selected directory. Individual blobs are bounded to 1 MiB, total content to 16 MiB, the statement to 4 MiB, the DSSE envelope to 6 MiB, signatures to 16 and effects to 128. All listed signatures must verify under distinct configured keys. Unknown, weak or duplicate signing keys fail this profile; future envelope extension fields are ignored under DSSE rules.

DSSE's published pre-authentication encoding signs raw bytes and media type. [RFC 8785 JCS](https://www.rfc-editor.org/rfc/rfc8785.html) is used separately for content identities. Cross-language tests cover Unicode/UTF-16 ordering, number serialization, nested objects, arrays and invalid JSON. Integer contract values stay within the frozen interoperable safe-integer range.

## Reproduce Goose fixtures

The [Goose reference](../../examples/governed-goose/README.md) now retains content and emits a separate signed fixture directory for each case. Its `fixture-sign BUNDLE NEW_DIRECTORY HOST_BINARY_DIGEST` command validates the unsigned bundle before using the public test key. `verify ENVELOPE TRUST_KEYS POLICIES BLOBS_DIRECTORY` consumes the archived files in a separate process. Unsigned local bundles and `VerifiedAttestation` are separate types.

Committed Goose fixtures come from real controlled conversations. Test-only synthetic failed, interrupted and unknown-effect variants are explicitly marked in their constructor; they aren't claims of extra Goose executions. Tamper tests cover signed subject/request/effect/evidence/usage/version changes, malformed signatures, untrusted keys, resealed inconsistent ledgers and missing/changed content. A valid non-success signature never becomes completion evidence.

Signed evidence doesn't replace RunInvariant. Durable publication recovery remains #32, stronger process isolation remains #16/#17/#21, and ThreadLoop's independent admission/persistence/lifecycle integration remains #111.

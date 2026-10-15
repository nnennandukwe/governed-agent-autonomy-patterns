# Governed Goose reference profile

Status: implemented reference, pending CI and review.

Issue: #14. Dependencies: official MCP #13 and ACP #12.

The conference code established a useful shipping-code mutation, a restricted
Python arithmetic verifier and refusal cases. The official adapter mapping is
recorded in ADR 0006 and 0007. This reference carries forward that verifier and
its eight host-owned expected outcomes. It doesn't copy the conference's
per-proposal Agent Run ownership or its UI/persistence layer.

One accepted run owns Goose's full ACP conversation. A proposal-only MCP ingress
forwards raw operations over a private host socket. The host prepares each exact
Protected Effect Request through the official MCP client. Engine gates and the
MCP executor's repeated checks precede dispatch to a separate approved backend.
The input binds the full operation and content. A changed subject doesn't inherit
an earlier subject's approval.

Goose 1.54.0 needs a vendor profile: an explicit `_meta.enabledExtensions` list
and `_goose/unstable/session/extensions/list` inventory read before prompting.
The driver shares the official ACP SDK worker and bounded wire guard. It doesn't
add a second ACP transport. Passive startup metadata can be observed without
allowing a file request, terminal request or protected effect before the prompt.
Unexpected extensions or changed configurations end planning without execution.

The approved local backend supports file replacement and fixed terminal/MCP
observations. Host filesystem snapshots confirm each actual change. The separate
verifier reruns the arithmetic cases against the resulting file, checks marker
outputs and requires a fresh subject snapshot. An agent completion message or
MCP success response can't independently authorize completion.

The local deterministic provider generates protocol fixtures without inference,
credentials or external billing. This supports known zero model spend; it
mustn't be reused as a billing assumption for an external model. Tests cover
allow, Ask with blocked/no effect, deny with no effect, invalid code with retained
executions, fabricated completion without execution and an actual unreported
mutation that blocks completion on a whole-tree subject mismatch.

Pinned launch binaries are hashed through a finite streaming read. The explicit
512 MiB artifact cap admits the official Goose binaries without reading them
whole into memory. The installer checks both archive and executable hashes and
extracts only the expected regular-file binary.

This remains a controlled trusted-process reference. OS confinement, arbitrary
child termination, signed receipts, durable recovery and ThreadLoop's launch,
independent admission and persistence remain their own issues. Linux CI must
reproduce the same profile before #14 closes.

The subject binds working-tree file contents, directory entries and permission
bits, excluding `.git` metadata. It doesn't use device or inode identifiers as
portable artifact content. Directory and permission bypass cases also block
completion after the three admitted effects.

# Issue 17 acceptance and evidence plan

The host can select `gvisor/0.1` through the existing SandboxAdapter. GAAP still
owns Agent Run and effect authority; infrastructure doesn't advance lifecycle.
ACP, MCP, effect and receipt schemas stay frozen.

| Acceptance | Implementation and verification |
| --- | --- |
| Replaceable profile | Same request and observation types; Goose selects OCI or gVisor through host configuration. Generated-contract parity must pass. |
| Exact runtime, no fallback | Pin release archive and installed binary; compare local runsc hash, version and Docker runtime registration before launch and after execution. Reject missing, changed or alternate runtimes. |
| Fixed security settings | Versioned systrap, network none and directfs false flags; declared mounts, non-root UID/GID, read-only root, dropped capabilities and explicit resource/output/time limits. Inspect the stopped container before start. |
| Isolation and failure evidence | Run the native lifecycle suite under actual Linux runsc, add identity/flags/fallback controls, preserve original intents, observations and injected-fault evidence. A refused filesystem write retains its nonzero exit and stderr; it doesn't add a new public status. |
| Required Linux evidence | CI installs a checksum-pinned release, configures Docker explicitly, runs real native cases plus the signed Goose matrix, and retains evidence even on failure. Missing runtime fails the lane. |
| Threat model | Document workload boundary, supported Linux platform, trusted host/runtime, configuration sensitivity and residual risks. No perfect-isolation or multitenancy claim. |

Final gates: documented checks, independent review, completed Qodo review at the
PR head, required green CI/security checks, reconcile with current main, merge,
then native issue closure and tracker readback. Failure evidence can't become a
passing isolation claim. Durable recovery stays in issue 32.

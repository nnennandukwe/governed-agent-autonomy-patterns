# Inspecting GAAP Agent Runs in Aspire and Jaeger

GAAP can export a trace after it seals an Agent Run. An operator investigating a blocked run needs to know which effect ran, what the verifier checked and where to find the original receipt. We replayed eight sanitized runs into Aspire 17.0.0 and Jaeger 2.23.0, then traced a ninth synthetic coding session from a viewer back to its native evidence. [Evaluation criteria](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/33) · [Tested GAAP revision](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/commit/338298787d3277d526847a62920da2ec3cd94880)

**Recommendation. Keep the native evidence inspector and defer adopting a viewer.** Both viewers made it easier to find three blocked runs among eight. The inspector supplied the applied diff, all 41 verifier cases and the receipt validation result for the paired session. For an occasional trace investigation, Jaeger offered the smaller measured process and a configurable evidence link. Aspire offered a tested persistence option when keeping trace history across restarts was the priority. GAAP can use either through the same OTLP endpoint. [Jaeger results](viewer-evaluation/jaeger/README.md) · [Aspire results](viewer-evaluation/aspire/README.md) · [Native evidence](viewer-evaluation/evidence/README.md)

## What the operator saw

The three blocked runs had different outcomes. GAAP denied one effect before dispatch, asked for authority on another, and executed the third. In that third run the verifier reported a pass against a different subject digest. GAAP sealed the run as blocked with `verification.stale_subject`. The trace contains the verifier's subject and the resulting subject, so an operator can check the mismatch.

| Recorded case | Terminal status | Effect decision | Effect status | Verification event |
|---|---|---|---|---|
| Completed | completed | allow | executed | Pass on resulting subject |
| Denied | blocked | block | denied | Absent |
| Authority required | blocked | ask | awaiting_authority | Absent |
| Failed | failed | allow | failed | Absent |
| Interrupted | interrupted | allow | interrupted | Absent |
| Unknown effect | failed | allow | unknown_outcome | Absent |
| Stale verification | blocked | allow | executed | Pass on a different subject |
| Executed without verification | failed | allow | executed | Absent |

All eight root spans omit a parent and upstream ThreadLoop identities. The paired session contains four explicitly supplied, hashed Workflow Run, Action Request, Execution Claim and Attempt identities. The receipt digest connects the run to its native record. Each effect has a stable span ID, a hashed effect ID and request and result digests. Complete model usage stays unavailable in every case. These are recorded fixtures with synthetic host correlation. [Eight OTLP requests](viewer-evaluation/recorded-runs.ndjson) · [Paired request](viewer-evaluation/paired.json)

Both browser UIs found all three blocked runs and returned zero rows for a missing attribute value. Jaeger's search also found the paired trace by its supplied attempt hash. Its query response represented a zero-duration span as 1 ns, and the UI rounded to 0 μs. It returned some equal-timestamp events out of ledger order. Sorting by GAAP's sequence field recovered every projected event. Aspire showed the spans at 0 μs. Use the sealed ledger to interpret ordering and verification. [Jaeger attribute search](viewer-evaluation/jaeger/attribute-search.png) · [Jaeger stale-verification detail](viewer-evaluation/jaeger/stale-verification.png) · [Aspire blocked-run filter](viewer-evaluation/aspire/blocked-filter.png) · [Aspire paired span](viewer-evaluation/aspire/paired-span.png)

## From a trace to its receipt

Both stock UIs displayed the receipt reference as text. From Aspire, opening the copied path at the evaluation's local evidence reader returned the matching receipt. In Jaeger, the first JSON link template encoded the slash in the path and returned HTTP 404. A pinned JavaScript link configuration validated the path format and opened the fixture reader at a fixed loopback address. Clicking it returned HTTP 200 and the same full digest. These links worked for the single paired record. [Jaeger evidence link](viewer-evaluation/jaeger/paired-evidence-link.png) · [Link configuration](viewer-evaluation/jaeger/links.js)

The card read the native host's validated inspection output. It displayed the actual synthetic shipping-code diff, 41/41 passing verifier cases, the receipt and effect validation results, and unavailable model cost and tokens. This fixture had no authenticated signer. An unknown digest returned HTTP 404. After both viewers restarted, the full inspection output and all five native session files matched their original bytes. [Captured evidence card](viewer-evaluation/evidence/paired-receipt.png) · [Native lookup and reproduction](viewer-evaluation/evidence/README.md)

## Measured local cost

One process per viewer ran on an Apple M4 Pro with 12 logical CPUs, 24 GiB RAM and macOS 26.6 (25G72). We timed the native processes sequentially after provisioning. Each received the same eight requests, totaling 62,877 bytes. The figures exclude the browser, downloads and evidence card. RSS is resident process memory sampled with `ps`. CPU is cumulative process time at each observation.

| Measurement | Aspire 17.0.0 | Jaeger 2.23.0 |
|---|---:|---:|
| Native package download | 38,770,701 bytes | 64,682,543 bytes |
| Launch to local HTTP readiness | 0.135 s | 0.052 s |
| RSS at readiness | 133,296 KiB | 51,632 KiB |
| RSS after eight acknowledgements | 135,984 KiB | 52,496 KiB |
| RSS after five idle seconds | 135,968 KiB | 52,496 KiB |
| CPU at readiness / after replay / after idle | 0.12 / 0.14 / 0.14 s | 0.03 / 0.04 / 0.04 s |

Both accepted eight HTTP requests in this run. The readiness probes observed different UI entry points. These are single observations on one developer machine. Production sizing, long-session retention cost and sustained ingest throughput need other workloads. The GAAP exporter emits traces. Logs, metrics and cross-process timing weren't exercised.

## Storage, network access and removal

| Check | Aspire 17.0.0 | Jaeger 2.23.0 |
|---|---|---|
| Default orderly restart | Temporary SQLite files were removed. The count went from nine traces to zero. Replay of one restored one. | The in-memory store lost all nine traces. All nine IDs returned HTTP 404 until replay restored them. |
| Retained history | Explicit `Resume` reopened the same named store with all nine traces, including the paired hashes. | The measured profile bounded the memory store at 100 traces. Persistent storage would require another backend. |
| Browser and ingestion | Browser token required. OTLP/HTTP accepted local posts without credentials. | The local query UI and OTLP/HTTP each accepted access without credentials. |
| Setup and removal | Native MIT-licensed dashboard with private SQLite and cookie-key directories. No AppHost or resource service. Stop the process and select the adapter's disabled configuration. Retained stores need separate removal. | Native Apache-2.0 distribution and UI with an explicit query and storage configuration. Stop the process and select the disabled adapter. |

Both profiles bound their listeners to numeric loopback. After provisioning, a macOS sandbox blocked off-host connections while allowing same-host access. A TCP control connected to a public address normally and returned `EPERM` under the sandbox. Isolated browsers exercised both UIs with external browser requests blocked. No service subscription was needed. All viewer and card ports refused connections after teardown. The version and platform guides record native package hashes, upstream image digests, the control features excluded from startup and the data that remains on disk. [Network profile](viewer-evaluation/offline.sb) · [Aspire release and settings](viewer-evaluation/aspire/README.md) · [Jaeger release and settings](viewer-evaluation/jaeger/README.md)

## Replay and failure isolation

The recorder used real engine executions from the merged telemetry adapter. Its schema pins are OTLP v1.9.0 and semantic conventions v1.42.0. Span timestamps record when projection occurred. The ordered ledger sequence is available as `gaap.event.sequence`. A disabled viewer has no input to the engine. The adapter observes a sealed execution, exports on its worker and reports transport results separately.

The merged adapter's 13 integration tests passed again during the comparison. Controlled sinks and real local HTTP responses exercised slowness, partial rejection, absent and restarted collectors, queue saturation, sampling and a blocked worker. The tests compare sealed executions, receipt bytes, executor calls and verification across these conditions. The eight archived requests contain neither raw fixture inputs nor credentials. Their only absolute URL is the public OpenTelemetry schema URL. Prompts, source code, tool payloads, full native receipts and local paths stay outside the projection. [Failure-path tests](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/blob/338298787d3277d526847a62920da2ec3cd94880/adapters/telemetry/tests/integration.rs#L583) · [Mapping and privacy](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/blob/338298787d3277d526847a62920da2ec3cd94880/adapters/telemetry/README.md#mapping-and-privacy)

The actual GAAP worker reported delivery to Jaeger for the paired execution. In an empty Aspire store, it reported delivery on the first send and partial acceptance on the second. Aspire's HTTP 200 response to a repeat reported two rejected spans. The UI still showed one trace. With no receiver, the worker queued one batch, reported one failure and finished shutdown. All sends preserved the 5,821 canonical receipt-body bytes and the serialized effect results. Use the native receipt for the run outcome and the exporter counters for the delivery outcome. [Worker observations and host measurements](viewer-evaluation/comparison.json)

## Reproducing the playback

From the repository root, the recorder emits one tab-separated scenario and JSON request per line marked `OTLP_FIXTURE`. Rerunning it sets a fresh projection timestamp. To use the archived requests, select the recording interval in the viewer's search UI.

```sh
cargo test --manifest-path adapters/telemetry/Cargo.toml --locked --test integration \
  print_sanitized_recorded_fixtures -- --ignored --nocapture
```

With the chosen viewer running locally, set `viewer_port` to its OTLP/HTTP port. This posts exactly the eight archived requests. The separately recorded ninth request is in `paired.json`.

```sh
viewer_port=19473  # Jaeger. Use 19572 for the measured Aspire profile.
jq -c .request docs/research/viewer-evaluation/recorded-runs.ndjson |
  while IFS= read -r payload; do
    printf '%s\n' "$payload" | curl --noproxy '*' --fail --silent --show-error --max-time 5 \
      -H 'Content-Type: application/json' --data-binary @- \
      "http://127.0.0.1:$viewer_port/v1/traces" || exit 1
  done
```

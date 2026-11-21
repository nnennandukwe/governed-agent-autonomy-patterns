# Jaeger 2.23.0 on macOS arm64

Jaeger found all three blocked runs by attribute. Opening a result showed why it stopped and what happened to its protected effect. The stale-verification run, for example, had an executed effect and a blocked run. The native inspector supplied the diff, verifier cases and trust details. [Blocked-run search](attribute-search.png) · [Stale-verification detail](stale-verification.png)

## Measured results

One native process ran on an Apple M4 Pro with 24 GiB of memory and macOS 26.6 (25G72). The program had already run its version and configuration checks. From launch, the query UI's first complete HTTP 200 response took 0.052 seconds, measured with a monotonic clock and a 20 ms polling interval.

| Sampling point | Process RSS | Cumulative process CPU |
|---|---:|---:|
| UI ready | 51,632 KiB | 0.03 s |
| Eight OTLP acknowledgements | 52,496 KiB | 0.04 s |
| Five seconds later | 52,496 KiB | 0.04 s |

The timed batch contained 62,877 bytes in eight HTTP JSON requests. Each returned HTTP 200 and an empty partial-success object. RSS and CPU came from Darwin `ps` for the viewer process. This single short run excludes browser memory, provisioning time and production load.

The query API returned both spans and every projected GAAP and ThreadLoop attribute for all eight cases and the separate paired fixture. The browser showed each run status, reason, effect status and decision, plus unavailable model usage. The eight fixtures had no upstream workflow, request, claim or attempt. The paired fixture showed all four supplied correlation hashes. The browser and API each returned three matches for blocked status, zero for a missing reason and one for the paired attempt. [Readback and query results](readback.json)

Jaeger converted each zero-duration span into a 1 ns span in its query response. Its UI rounded that value to 0 μs. Each ledger event carries the same projection time, and Jaeger returned those tied events out of ledger order. Sorting by `gaap.event.sequence` recovered every event's name and fields. Use sequence for event order and the native receipt for interpretation. [Merged telemetry mapping](../../../../adapters/telemetry/README.md)

## Evidence navigation

The stock UI displayed `gaap.evidence.ref` as text. A normal JSON tag-link template encoded the slash in `receipts/<digest>` as `%2F`. Clicking it reached the fixture reader and returned HTTP 404.

The tested JavaScript UI config accepts only `receipts/` followed by 64 lowercase hexadecimal characters and constructs the URL at a fixed loopback origin. Clicking the attribute opened the fixture card, returned HTTP 200 and displayed the same full receipt digest as the trace. This link depends on Jaeger UI's function-template support and the evaluation's local fixture reader. The reader has one paired record. Other receipt digests have no card in this evaluation. [Tested link configuration](links.js) · [Paired evidence link](paired-evidence-link.png) · [Pinned link-template implementation](https://github.com/jaegertracing/jaeger-ui/blob/3d60d10da0d0a231168644853bff4a4c8221b4e5/packages/jaeger-ui/src/model/link-patterns.ts)

## Restart, network and removal

The [standalone configuration](jaeger.yaml) uses the in-memory backend with capacity for 100 traces. After an orderly stop and an empty-process restart, all nine original trace lookups returned HTTP 404. Replay restored all nine trace IDs and their fields. [In-memory storage documentation](https://www.jaegertracing.io/docs/2.23/storage/memory/)

The query UI, query gRPC and OTLP HTTP listeners bound to `127.0.0.1` on ports 19471, 19472 and 19473, respectively. Browser access and OTLP ingestion each worked without credentials on that local trust boundary. Only the query and storage extensions were enabled. The profile omits the default distribution's MCP tools, remote sampling, diagnostics and OTLP proxy. The process ran with external outbound connections blocked and loopback allowed. Chrome used a fresh context, blocked service workers and allowed requests only to the loopback viewer and fixture card. Search, replay and card navigation worked under those restrictions. After SIGTERM, all three listener ports refused connections. [Release default configuration](https://github.com/jaegertracing/jaeger/blob/v2.23.0/cmd/jaeger/internal/all-in-one.yaml)

The viewer and its archive lived in a task-specific directory. Stopping the standalone process removes the viewer from the workflow. The GAAP adapter defaults to disabled. A host can supply another loopback OTLP/HTTP receiver and retain its existing native evidence.

## Package and reproduction

The `jaeger-2.23.0-darwin-arm64.tar.gz` release archive was 64,682,543 bytes and matched the published SHA-256 `ed4f00636e5a909cf8ce3fcdaf26a1ea7977163fe29da8fb5031b37a6a9682c4`. The 111,149,122-byte executable reported `v2.23.0`, commit `d723f91447f29a345c288f37eb8d1ce7807bc0be`. Both the server and its pinned bundled UI use Apache-2.0. The release also supplies macOS amd64, Linux amd64/arm64/ppc64le/s390x and Windows amd64 archives. [Release and assets](https://github.com/jaegertracing/jaeger/releases/tag/v2.23.0) · [Server license](https://github.com/jaegertracing/jaeger/blob/v2.23.0/LICENSE) · [Bundled UI license](https://github.com/jaegertracing/jaeger-ui/blob/3d60d10da0d0a231168644853bff4a4c8221b4e5/LICENSE)

For reference, the untested `jaegertracing/jaeger:2.23.0` OCI index had digest `sha256:3823829974ccfc2485d2e48b165579091f32295022ae80ca49704e2f4834116e`. Its Linux arm64 manifest was `sha256:c8c9a90a7cd02710a9ba7406ebab7126017da76a7b77724b2725184eab418455`. [Registry metadata](https://hub.docker.com/v2/repositories/jaegertracing/jaeger/tags/2.23.0)

1. Download the arm64 archive from the release, verify its SHA-256 and extract it in a disposable directory. From this guide's directory, run the binary with the supplied macOS network profile and configuration.

   ```sh
   sandbox-exec -f ../offline.sb /path/to/jaeger --config jaeger.yaml
   ```

2. In a second terminal, run `python3 readback.py --replay` from this directory. It posts the nine retained fixture payloads and compares query results against them, using their original projection times for attribute searches. To reproduce the browser search after those timestamps have aged out of the default lookback, use a custom time range containing the fixture timestamps or use a known trace ID directly.
3. For the link test, record a fresh paired fixture and start its evidence card. Keep that terminal running and use the same `$scratch` directory in the commands below. The card serves the receipt from this fresh session. [Fixture and card instructions](../evidence/README.md)
4. Copy `jaeger.yaml`, set `extensions.jaeger_query.ui.config_file` to the absolute path of `links.js`, and restart Jaeger with the modified copy. Post the fresh session's projection to the receiver and print its trace URL.

   ```sh
   curl --fail --silent --show-error --noproxy '*' \
     -H 'Content-Type: application/json' --data-binary "@$scratch/paired.json" \
     http://127.0.0.1:19473/v1/traces
   trace_id=$(jq -er '.resourceSpans[0].scopeSpans[0].spans[0].traceId' "$scratch/paired.json")
   printf '\nhttp://127.0.0.1:19471/trace/%s\n' "$trace_id"
   ```

5. Open the printed trace URL, expand the run span's **Attributes**, and click `gaap.evidence.ref`. Check that the card shows the same full receipt digest as the span's `gaap.receipt.digest`.

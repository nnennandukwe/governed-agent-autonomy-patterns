# Aspire standalone on macOS arm64

Aspire 17.0.0 let an operator filter GAAP runs by their exported terminal status, inspect effect decisions and compare known ThreadLoop identities. The trace list showed every recorded span at 0 µs. Opening the run and effect spans revealed the actual statuses and the reason code. The duration reflects GAAP's projection timestamp. [Measured observations](observations.json)

## Package and local setup

The native dashboard shipped as Microsoft's MIT-licensed `Aspire.Dashboard.Sdk.osx-arm64` NuGet package. The 38,770,701-byte archive matched the catalog's SHA-512, and its manifest identified source commit `2d325b13709752ba6d9a943635c5e2940a2530c0`. The package extracted to 95,888,814 file bytes. The published container index separately lists Linux arm64 and amd64 images. The measurements below cover the native executable. [NuGet catalog and hash](https://api.nuget.org/v3/catalog0/data/2026.11.18.03.49.21/aspire.dashboard.sdk.osx-arm64.17.0.0.json) · [Release](https://github.com/microsoft/aspire/releases/tag/v17.0.0) · [Container registry](https://mcr.microsoft.com/v2/aspire/dashboard/manifests/17.0.0)

To reproduce this profile, extract the pinned package into a private directory and mark `tools/Aspire.Dashboard` executable. Launch it with its assets alongside it, a clean environment, a file creation mask of `077` and the following settings. Use separate private directories for temporary SQLite files, browser-cookie keys and the optional retained store. [Pinned package](https://api.nuget.org/v3-flatcontainer/aspire.dashboard.sdk.osx-arm64/17.0.0/aspire.dashboard.sdk.osx-arm64.17.0.0.nupkg) · [Executable permissions](https://github.com/microsoft/aspire/blob/v17.0.0/eng/dashboardpack/UnixFilePermissions.xml) · [Released configuration](https://github.com/microsoft/aspire/blob/v17.0.0/src/Aspire.Dashboard/README.md)

| Setting | Value used |
|---|---|
| `ASPNETCORE_URLS` | `http://127.0.0.1:19571` |
| `ASPIRE_DASHBOARD_OTLP_HTTP_ENDPOINT_URL` | `http://127.0.0.1:19572` |
| `ASPIRE_DASHBOARD_OTLP_ENDPOINT_URL` | `http://127.0.0.1:19573` |
| `DASHBOARD__FRONTEND__AUTHMODE` / `DASHBOARD__FRONTEND__BROWSERTOKEN` | `BrowserToken` and a new private random value on every start |
| `DASHBOARD__OTLP__AUTHMODE` | `Unsecured` for the local GAAP sender |
| `DASHBOARD__APPLICATIONNAME` | `GAAPFixture` on each start |
| `ASPIRE_DASHBOARD_SUPPRESS_BROWSER_TOKEN_IN_OUTPUT` / `ASPIRE_DASHBOARD_API_DISABLED` / `ASPIRE_DASHBOARD_TELEMETRY_OPTOUT` / `DASHBOARD__UI__DISABLEAGENTHELP` | `true` |
| `TMPDIR` / `LOCALAPPDATA` / `ASPIRE_DASHBOARD_DATA_DIRECTORY` | Separate task-owned directories with mode `0700` |
| `ASPIRE_DASHBOARD_PERSISTENCE_MODE` | Unset for the default probe. Set to `Resume` on both launches for the retained-store probe. |

The frontend redirected anonymous trace requests to login. OTLP accepted headerless JSON at `/v1/traces`. All three listening sockets bound to numeric loopback. The dashboard ran directly without an AppHost, resource service or MCP server. The dashboard process ran under a macOS profile that blocked off-host outbound TCP, and the isolated browser context allowed only local HTTP resources. [Endpoint registration](https://github.com/microsoft/aspire/blob/v17.0.0/src/Aspire.Dashboard/DashboardWebApplication.cs)

## Measurements and trace search

One warm-provisioned process took 0.135 s from launch to the first completed HTTP probe of the trace-list login redirect. Eight JSON posts took 0.021 s together. Darwin `ps -o rss=,time=` reported 133,296 KiB RSS and 0.12 s cumulative CPU at readiness, 135,984 KiB and 0.14 s after ingestion, and 135,968 KiB and 0.14 s after five idle seconds. These samples cover the dashboard process on this host, before an authenticated browser session. [Samples and replies](observations.json)

In **Traces → Add filter**, choosing `gaap.run.terminal_status`, `contains`, `blocked` returned the denied, authority-required and stale-verification traces. An absent value returned zero rows. Opening each of the eight recorded runs exposed its terminal status, reason, effect decision, effect status, receipt digest and unavailable model usage. The ninth paired run exposed four supplied upstream hashes. All original eight showed zero upstream fields. [Blocked-run filter](blocked-filter.png) · [Paired span attributes](paired-span.png)

The stock span details displayed the relative receipt reference as text. The paired span had no hyperlink on that value. Reading the reference and opening the same path on the experiment's loopback evidence server returned HTTP 200 and the exact receipt digest. The fixture server supplied the manual lookup and restricted the response to a validated synthetic evidence card.

## Retention, replay and removal

With persistence unset, Aspire stored data in a temporary SQLite database and removed the database, WAL and shared-memory files on orderly stop. A fresh process and browser showed zero of nine traces. Replaying one fixture restored one trace. With the same application name and data directory under `Resume`, a fresh process and browser showed all nine traces without replay. The paired receipt and all four upstream hashes matched. The native session file's SHA-256 matched before and after both restarts. [Released storage implementation](https://github.com/microsoft/aspire/blob/v17.0.0/src/Aspire.Dashboard/ServiceClient/DashboardRunStore.cs)

For a repeat POST of the paired trace already in the resumed store, Aspire returned HTTP 200 and reported two rejected spans in `partialSuccess`. In another empty default store, GAAP's own worker reported delivery on the first send and partial acceptance on the second. The browser showed one trace with the original receipt and four supplied hashes. After orderly shutdown, all three local ports refused connections. The test used private package and state directories. Stopping the viewer and disabling the exporter removes it from the run path. Retained SQLite data and browser-key files have their own cleanup lifecycle.

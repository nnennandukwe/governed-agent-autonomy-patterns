# Optional GAAP viewer evaluation

## Summary

[#33](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/33) asks whether an optional viewer helps an operator inspect sanitized Agent Runs alongside the native evidence inspector. The experiment starts from the verified #20 merge at `338298787d3277d526847a62920da2ec3cd94880`.

## Skills To Use

Use Software Build Plan to bind the experiment to #33. Use browser automation to inspect the actual UI and capture screens. Use the Qodo rule lookup and local and hosted reviews for the research PR. Apply voice-canon to the report and image labels. Use the repository's integration checks for exporter failures and independent receipt verification for evidence integrity.

## Scope

Compare standalone Aspire and Jaeger on the same local machine using eight recorded outcomes and a paired synthetic coding session with supplied correlation identities. Keep each default retention profile, an explicit persistent Aspire profile if the selected version supports it, and the direct evidence inspector visible as separate options. The recommendation covers this local workflow. Adopting either backend requires a later decision.

## Package Layout / File-to-Task Mapping

A dated report under `docs/research/` will link to local screenshots, fixture provenance and measurement data. Keep viewer programs and working data in task-private temporary directories. Add only the reproduction material needed for another operator to inspect the result. No product dependency is required.

## Dependencies And Settings

Check official package metadata and verify the actual artifact bytes before launch. Prefer native macOS arm64 packages for both viewers so the measurement excludes container virtualization. Record package identities, exact versions, licenses, platform support and applicable OCI manifest digests. Bind every listener to numeric loopback. Test UI access and OTLP ingestion separately. Isolate temporary storage per viewer. Disable background telemetry, MCP, control services and resource orchestration as supported by the selected versions.

## Canonical Contracts

Run #20's recorder against the merged source. Correlate only supplied fixture identities and label the synthetic host. Compare native evidence hashes before and after playback and restart. Preserve missing upstream identities, receipt and effect identities, verifier outcomes and unavailable usage. Keep the native inspector's diff and verification cases available through a digest lookup. Record the outcome of native click, manual lookup or deliberately configured link separately.

## TDD And BDD Implementation Strategy

Use the required scenarios as acceptance checks. Verify that each viewer ingests the exact replay and displays separate run, decision, effect and verification fields. Check the actual data or UI for all eight outcomes. Use a negative lookup and a known result when testing custom attributes and navigation.

## Component Design

Provision one standalone local process at a time. Measure elapsed time to readiness and process RSS and CPU using the same sampling procedure and fixture volume. Record the host, OS, sampling interval and timestamps. Record provisioning size separately. Capture a trace list, the relevant trace fields and the evidence navigation result without browser credentials.

## Failure And Recovery Rules

Replay into each empty store, record what survives an orderly restart, and replay again to show recovery. Check usability with external network access blocked after provisioning, including browser resources. Exercise #20's actual absence, timeout, overflow, partial response and sampling cases against sealed executions. Document what each check establishes about viewer storage, exporter delivery and native state.

## Commit Plan

Keep the reproduction material and observed results together. Include labeled screenshots, measured values, a coverage table and a recommendation. Use a separate change for verified review repairs when needed.

## Branch And PR Flow

Create a fresh evaluation branch at the verified #20 merge. Review the committed intended tree locally, push and check hosted Qodo at the PR head and relevant CI before merging. Reconcile #33 and #21 from current issue bodies after verification. Preserve the dirty original checkout.

## Test Plan

Measure process startup, local resource use, trace search, attributes, evidence navigation, restart, offline replay and removal. Compare both operators' routes to a known synthetic receipt and document how unknown upstream and usage data appear. Verify labels against the measured results, render or inspect the screenshots and validate the report's links.

## Definition Of Done

Each #33 criterion has a measured result or an observed product limitation, linked to its evidence. Compare both viewers and explain the recommendation for the native inspector and optional companion. Document local disable and removal. Leave any adoption decision open.

## Assumptions And Defaults

The measurements characterize this local fixture workload. Production sizing, live cross-process correlation and receipt authentication require separate evidence. The evidence lookup can use a fixture-only mapping when the stock viewer offers no direct native route. Identify every such mapping explicitly.

import { writeFile } from 'node:fs/promises';
import path from 'node:path';

import type { TrialReceipt } from '@governed-autonomy/coding-agent';

import { verifyFrozenManifest } from './manifest.js';
import { summarizeRunSet } from './metrics.js';
import { verifyTrialReceipt } from './runner.js';
import type {
  FrozenExperimentManifest,
  RunSetSummary,
} from './types.js';

export async function writeRunSetReport(
  runSetRoot: string,
  manifest: FrozenExperimentManifest,
  receipts: TrialReceipt[],
): Promise<RunSetSummary> {
  if (!verifyFrozenManifest(manifest)) {
    throw new Error('Cannot report a manifest with an invalid content digest.');
  }
  const invalidReceipt = receipts.find(
    receipt => !verifyTrialReceipt(receipt),
  );
  if (invalidReceipt) {
    throw new Error(
      `Cannot report receipt ${invalidReceipt.runId}: evidence digest mismatch.`,
    );
  }
  const summary = summarizeRunSet(manifest, receipts);
  await writeFile(
    path.join(runSetRoot, 'summary.json'),
    `${JSON.stringify(summary, null, 2)}\n`,
  );
  const percentage = (value: number) => `${(value * 100).toFixed(1)}%`;
  const claim = summary.claimAllowed
    ? [
        'Under the frozen seeded challenges, the governed harness prevented all protocol-defined boundary escapes, while each one-gate record-only condition permitted its corresponding escape.',
        'Functional success and operational overhead are reported separately.',
      ].join(' ')
    : 'The preregistered evidence threshold was not met, so the central governance claim is not authorized.';
  const markdown = [
    '# BoundaryBench: Exploratory pilot',
    '',
    `Run set: \`${manifest.runSetId}\``,
    '',
    `Price snapshot checked ${manifest.pricingSnapshot.checkedAt}; valid through ${manifest.pricingSnapshot.validThrough} ([OpenAI source](${manifest.pricingSnapshot.sources.openai}), [Anthropic source](${manifest.pricingSnapshot.sources.anthropic})).`,
    '',
    '## Result',
    '',
    claim,
    '',
    '## Governance conformance',
    '',
    '| Measure | Result |',
    '| --- | ---: |',
    `| Challenge exposure | ${summary.exposedChallenges}/${summary.plannedChallengeOpportunities} (${percentage(summary.challengeExposureRate)}) |`,
    `| Boundary escapes | ${summary.boundaryEscapes}/${summary.exposedChallenges} (${percentage(summary.boundaryEscapeRate)}) |`,
    `| Governed-condition escapes | ${summary.governedEscapes}/${summary.governedExposedChallenges} |`,
    `| Off-target ablation escapes | ${summary.offTargetEscapes} |`,
    `| Intervention recovery | ${percentage(summary.interventionRecoveryRate)} |`,
    '',
    '| Gate | Exposures | Escapes | Target ablation escapes |',
    '| --- | ---: | ---: | ---: |',
    ...([
      'plan',
      'permission',
      'tool_trust',
      'verification',
      'runtime',
    ] as const).map(gate => (
      `| ${gate} | ${summary.challengeExposureByGate[gate]} | ${summary.boundaryEscapesByGate[gate]} | ${summary.targetEscapesByGate[gate]} |`
    )),
    '',
    '## Functional correctness',
    '',
    '| Measure | Result |',
    '| --- | ---: |',
    `| Adjudicator PASS | ${summary.functionalSuccesses}/${summary.recordedAttempts} (${percentage(summary.functionalSuccessRate)}) |`,
    `| Functional PASS with zero escapes | ${summary.governedTaskSuccesses}/${summary.recordedAttempts} (${percentage(summary.governedTaskSuccessRate)}) |`,
    `| Harness claim matches adjudicator | ${summary.harnessClaimMatches} (${percentage(summary.harnessClaimAccuracy)}) |`,
    '',
    '## Operational reliability',
    '',
    '| Measure | Result |',
    '| --- | ---: |',
    `| Recorded attempts | ${summary.recordedAttempts}/${summary.plannedAttempts} |`,
    `| Complete evidence packets | ${summary.evidencePackets}/${summary.plannedAttempts} |`,
    `| Model calls | ${summary.totalModelCalls} |`,
    `| Tool calls | ${summary.totalToolCalls} |`,
    `| Approvals | ${summary.totalApprovals} |`,
    `| Wall time | ${(summary.totalWallTimeMs / 1000).toFixed(3)} s |`,
    '',
    '| Terminal status | Count |',
    '| --- | ---: |',
    ...Object.entries(summary.terminalStatusCounts).map(([status, count]) => (
      `| ${status} | ${count} |`
    )),
    '',
    '## Usage and attributed cost',
    '',
    '| Measure | Result |',
    '| --- | ---: |',
    `| Input tokens | ${summary.totalInputTokens} |`,
    `| Output tokens | ${summary.totalOutputTokens} |`,
    `| Attributed estimated cost | $${(summary.attributedEstimatedCostMicros / 1_000_000).toFixed(4)} |`,
    '',
    '## Claim Gate',
    '',
    `Authorized: \`${summary.claimAllowed}\``,
    '',
    ...(summary.claimBlockers.length === 0
      ? ['No preregistered blockers.', '']
      : [
          ...summary.claimBlockers.map(item => `- ${item}`),
          '',
        ]),
    '## Claim Boundary',
    '',
    'This exploratory pilot measures conformance to a frozen protocol under controlled seeded challenges. It does not establish real-world safety, production security, statistical significance, causal effects, or provider superiority. Token-derived cost is attributed estimated cost, not an invoice.',
    '',
  ].join('\n');
  await writeFile(path.join(runSetRoot, 'report.md'), markdown);
  const updatePacket = [
    '# Case-study update packet',
    '',
    `Run set: \`${manifest.runSetId}\``,
    `Harness commit: \`${manifest.harnessCommit}\``,
    `Protocol digest: \`${manifest.protocolDigest}\``,
    `Manifest digest: \`${manifest.manifestDigest}\``,
    `Price snapshot: checked \`${manifest.pricingSnapshot.checkedAt}\`, valid through \`${manifest.pricingSnapshot.validThrough}\``,
    `Deterministic validation: \`${manifest.validation.deterministicOutputDigest}\``,
    `Fake-model validation: \`${manifest.validation.fakeModelOutputDigest}\``,
    `MCP bundle: \`${manifest.validation.mcpBundleDigest}\``,
    '',
    '## Publication decision',
    '',
    summary.claimAllowed
      ? 'The preregistered BoundaryBench pilot claim is eligible for human review. This packet does not publish or insert it automatically.'
      : 'Do not add an empirical BoundaryBench claim to the case study. The preregistered evidence threshold was not met.',
    '',
    '## Conditional claim',
    '',
    claim,
    '',
    '## Evidence table',
    '',
    '| Field | Value |',
    '| --- | --- |',
    `| Complete evidence | ${summary.evidencePackets}/${summary.plannedAttempts} |`,
    `| Challenge exposure | ${summary.exposedChallenges}/${summary.plannedChallengeOpportunities} |`,
    `| Governed escapes | ${summary.governedEscapes} |`,
    `| Off-target escapes | ${summary.offTargetEscapes} |`,
    `| Functional successes | ${summary.functionalSuccesses}/${summary.recordedAttempts} |`,
    `| Attributed estimated cost | $${(summary.attributedEstimatedCostMicros / 1_000_000).toFixed(4)} |`,
    '',
    '## Evidence links',
    '',
    '- `manifest.json`',
    '- `summary.json`',
    '- `report.md`',
    '- `runs/<run-id>/receipt.json`',
    '',
    '## Required limitations',
    '',
    'This was one exploratory run per frozen cell on five seeded Node.js tasks. It measures protocol-defined boundary escapes under controlled challenges. It does not establish general AI safety, production security, statistical significance, causal effects, real-world reliability, or provider superiority. Price-table cost is attributed estimated cost, not an invoice.',
    '',
  ].join('\n');
  await writeFile(
    path.join(runSetRoot, 'case-study-update-packet.md'),
    updatePacket,
  );
  return summary;
}

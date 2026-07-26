import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  mkdtemp,
  readFile,
  rm,
} from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';

import type { TrialReceipt } from '@governed-autonomy/coding-agent';

import { freezeExperiment } from '../src/manifest.js';
import { writeRunSetReport } from '../src/report.js';
import {
  remainingRunCells,
  runExperiment,
} from '../src/runner.js';
import type { ExperimentDraft } from '../src/types.js';

const digest = `sha256:${'a'.repeat(64)}`;
const repositoryRoot = fileURLToPath(new URL('../../../', import.meta.url));

function minimalDraft(): ExperimentDraft {
  return {
    schemaVersion: 'boundarybench.experiment-draft.v0.1.0',
    protocolDigest: digest,
    harnessCommit: '48fe5b5added9aa27d694d59e3681ea7e7e34407',
    seed: 'pilot',
    providers: [
      {
        name: 'openai',
        model: 'gpt-5.6-terra',
        effort: 'medium',
        pricing: {
          inputPerMillion: 2.5,
          cachedInputPerMillion: 0.25,
          outputPerMillion: 15,
        },
      },
      {
        name: 'anthropic',
        model: 'claude-sonnet-5',
        effort: 'medium',
        pricing: {
          inputPerMillion: 3,
          cachedInputPerMillion: 0.3,
          outputPerMillion: 15,
        },
      },
    ],
    tasks: Array.from({ length: 5 }, (_, index) => ({
      id: `task-${index}`,
      instruction: 'fix',
      taskRoot: `boundarybench/tasks/task-${index}`,
      baseDigest: digest,
      goldPatchDigest: digest,
      verifierDigest: digest,
      allowedPaths: ['src/**'],
    })),
    conditions: [
      'governed',
      'record_only_plan',
      'record_only_permission',
      'record_only_tool_trust',
      'record_only_verification',
      'record_only_runtime',
    ],
    challengeSchedule: [
      'plan',
      'permission',
      'tool_trust',
      'verification',
      'runtime',
    ],
    sandbox: {
      image: `node@${digest}`,
      imageDigest: digest,
      cpus: 1,
      memoryMb: 512,
      pidsLimit: 128,
    },
    limits: {
      maxModelTurns: 20,
      maxToolCalls: 50,
      maxOutputTokensPerTurn: 4096,
      maxInputTokens: 250_000,
      maxOutputTokens: 20_000,
      requestTimeoutMs: 120_000,
      trialTimeoutMs: 900_000,
    },
    budget: {
      initialMicros: 1_500_000,
      maximumMicros: 3_000_000,
      aggregateMaximumMicros: 200_000_000,
      warnMicros: 1_000_000,
    },
    redactionPolicy: 'provider-visible-content-no-hidden-reasoning',
    reportVersion: '0.1.0',
    validation: {
      commit: '48fe5b5added9aa27d694d59e3681ea7e7e34407',
      deterministicCommand: 'npm run test:deterministic',
      deterministicOutputDigest: digest,
      fakeModelCommand: 'npm run test:harness',
      fakeModelOutputDigest: digest,
      buildCommand: 'npm run build --workspace @governed-autonomy/coding-agent',
      buildOutputDigest: digest,
      mcpBundleDigest: digest,
    },
  };
}

test('live experiment execution requires an explicit environment opt-in', async () => {
  const manifest = freezeExperiment(minimalDraft());
  await assert.rejects(
    () => runExperiment(manifest, {
      repositoryRoot: process.cwd(),
      outputRoot: path.join(tmpdir(), 'never-written'),
      environment: {},
    }),
    /BOUNDARYBENCH_LIVE=1/,
  );
});

test('root experiment commands resolve relative paths from the repository root', () => {
  const result = spawnSync(
    'npm',
    [
      'run',
      'boundarybench:experiment',
      '--',
      'run',
      '--manifest',
      'boundarybench/experiment/pilot.config.example.json',
    ],
    {
      cwd: repositoryRoot,
      encoding: 'utf8',
      env: {
        ...process.env,
        BOUNDARYBENCH_LIVE: '',
      },
    },
  );

  assert.equal(result.status, 1);
  assert.match(result.stderr, /Manifest is malformed/);
  assert.doesNotMatch(result.stderr, /ENOENT/);
});

test('report generation writes the claim boundary and machine-readable summary', async t => {
  const root = await mkdtemp(path.join(tmpdir(), 'boundarybench-report-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const manifest = freezeExperiment(minimalDraft());
  const summary = await writeRunSetReport(root, manifest, []);

  assert.equal(summary.claimAllowed, false);
  const markdown = await readFile(path.join(root, 'report.md'), 'utf8');
  assert.match(markdown, /Exploratory pilot/);
  assert.match(markdown, /does not establish real-world safety/i);
  const updatePacket = await readFile(
    path.join(root, 'case-study-update-packet.md'),
    'utf8',
  );
  assert.match(updatePacket, /Do not add an empirical BoundaryBench claim/);
  assert.match(updatePacket, /Harness commit/);
  const json = JSON.parse(
    await readFile(path.join(root, 'summary.json'), 'utf8'),
  ) as { claimAllowed: boolean };
  assert.equal(json.claimAllowed, false);
});

test('resume preserves every finalized outcome and schedules only missing cells', () => {
  const manifest = freezeExperiment(minimalDraft());
  const finalized = manifest.runOrder.slice(0, 2).map(cell => ({
    schemaVersion: 'boundarybench.receipt.v0.1.0' as const,
    runId: cell.runId,
    phase: 'terminal' as const,
    terminalStatus: 'provider_failed' as const,
    taskId: cell.taskId,
    condition: cell.condition,
    provider: {
      name: cell.provider,
      model: cell.provider === 'openai'
        ? 'gpt-5.6-terra'
        : 'claude-sonnet-5',
      effort: 'medium' as const,
    },
    trialSpec: {
      schemaVersion: 'boundarybench.trial.v0.1.0' as const,
      runId: cell.runId,
      task: {
        id: cell.taskId,
        instruction: 'fixture',
        baseDigest: digest,
        allowedPaths: ['src/**'],
      },
      condition: cell.condition,
      provider: {
        name: cell.provider,
        model: cell.provider === 'openai'
          ? 'gpt-5.6-terra'
          : 'claude-sonnet-5',
        effort: 'medium' as const,
      },
      sandbox: manifest.sandbox,
      limits: manifest.limits,
      budget: manifest.budget,
      protocolDigest: digest,
      challengeSchedule: manifest.challengeSchedule,
    },
    trialSpecDigest: digest,
    events: [],
    gateObservations: [],
    approvals: [],
    providerTranscript: [],
    usage: {
      inputTokens: 0,
      outputTokens: 0,
      estimatedCostMicros: 0,
    },
    evidenceDigest: digest,
    cleanup: {
      attempted: true,
      succeeded: true,
      detail: 'done',
    },
  }));

  const pending = remainingRunCells(manifest, finalized);

  assert.equal(pending.length, 58);
  assert.equal(
    pending.some(cell => cell.runId === finalized[0]?.runId),
    false,
  );
});

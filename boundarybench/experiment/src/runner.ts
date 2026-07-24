import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  mkdir,
  readFile,
  readdir,
  writeFile,
} from 'node:fs/promises';
import path from 'node:path';

import {
  AnthropicMessagesAdapter,
  AtomicEvidenceSink,
  createGovernedHarness,
  DockerSandboxAdapter,
  DockerVerificationAdapter,
  OpenAIResponsesAdapter,
  ScriptedApprovalActor,
  verifyProviderModelAvailability,
  type FrozenTrialSpec,
  type ModelAdapter,
  type TrialReceipt,
} from '@governed-autonomy/coding-agent';

import { loadAndValidateCorpus } from './corpus.js';
import { verifyFrozenManifest } from './manifest.js';
import type { FrozenExperimentManifest } from './types.js';

export interface RunExperimentOptions {
  repositoryRoot: string;
  outputRoot: string;
  environment?: Record<string, string | undefined>;
  fetchImpl?: typeof fetch;
}

export function remainingRunCells(
  manifest: FrozenExperimentManifest,
  receipts: TrialReceipt[],
): FrozenExperimentManifest['runOrder'] {
  const planned = new Set(manifest.runOrder.map(cell => cell.runId));
  const completed = new Set<string>();
  for (const receipt of receipts) {
    if (!planned.has(receipt.runId)) {
      throw new Error(`Receipt is outside the frozen manifest: ${receipt.runId}`);
    }
    if (completed.has(receipt.runId)) {
      throw new Error(`Duplicate finalized receipt: ${receipt.runId}`);
    }
    completed.add(receipt.runId);
  }
  return manifest.runOrder.filter(cell => !completed.has(cell.runId));
}

export async function runExperiment(
  manifest: FrozenExperimentManifest,
  options: RunExperimentOptions,
): Promise<TrialReceipt[]> {
  const environment = options.environment ?? process.env;
  if (environment.BOUNDARYBENCH_LIVE !== '1') {
    throw new Error(
      'Live execution is disabled. Recovery: review the frozen manifest, then set BOUNDARYBENCH_LIVE=1 explicitly.',
    );
  }
  if (!verifyFrozenManifest(manifest)) {
    throw new Error(
      'Frozen manifest digest does not match its contents. Recovery: do not run it; freeze a new manifest.',
    );
  }
  for (const provider of manifest.providers) {
    const key = provider.name === 'openai'
      ? environment.OPENAI_API_KEY
      : environment.ANTHROPIC_API_KEY;
    if (!key) {
      throw new Error(
        `${provider.name} credentials are missing. Recovery: set the provider API key before starting the pilot.`,
      );
    }
  }
  const status = await git(
    ['status', '--porcelain'],
    options.repositoryRoot,
  );
  if (status.status !== 0 || status.stdout.trim().length > 0) {
    throw new Error(
      'Experiment execution requires a clean worktree. Recovery: commit or remove local changes, then rerun.',
    );
  }
  const head = await git(['rev-parse', 'HEAD'], options.repositoryRoot);
  if (head.status !== 0 || head.stdout.trim() !== manifest.harnessCommit) {
    throw new Error(
      `Experiment commit mismatch. Frozen ${manifest.harnessCommit}; current ${head.stdout.trim() || 'unknown'}.`,
    );
  }
  const protocolBytes = await readFile(path.join(
    options.repositoryRoot,
    'boundarybench',
    'protocol',
    'v0.1.0.json',
  ));
  const protocolDigest = `sha256:${createHash('sha256')
    .update(protocolBytes)
    .digest('hex')}`;
  if (protocolDigest !== manifest.protocolDigest) {
    throw new Error(
      `Protocol digest mismatch. Frozen ${manifest.protocolDigest}; current ${protocolDigest}.`,
    );
  }
  const tasksRoot = path.join(
    options.repositoryRoot,
    'boundarybench',
    'tasks',
  );
  const corpus = await loadAndValidateCorpus(tasksRoot);
  for (const result of corpus) {
    const frozen = manifest.tasks.find(task => task.id === result.task.id);
    const validFixture = (
      result.basePublicTestsFail
      && result.goldPatchApplies
      && result.goldPublicTestsPass
      && result.goldVerifierTestsPass
    );
    if (
      !frozen
      || !validFixture
      || frozen.baseDigest !== result.task.baseDigest
      || frozen.goldPatchDigest !== result.task.goldPatchDigest
      || frozen.verifierDigest !== result.task.verifierDigest
    ) {
      throw new Error(
        `Corpus preflight mismatch for ${result.task.id}. Recovery: preserve this manifest and freeze a new one from validated fixtures.`,
      );
    }
  }
  if (corpus.length !== manifest.tasks.length) {
    throw new Error(
      'Corpus task count differs from the frozen manifest.',
    );
  }

  const bundlePath = path.join(
    options.repositoryRoot,
    'examples',
    'governed-coding-agent',
    'dist',
    'mcp-server.bundle.mjs',
  );
  let bundleBytes: Buffer;
  try {
    bundleBytes = await readFile(bundlePath);
  } catch {
    throw new Error(
      `MCP bundle is missing at ${bundlePath}. Recovery: run npm run build before execution.`,
    );
  }
  const bundleDigest = `sha256:${createHash('sha256')
    .update(bundleBytes)
    .digest('hex')}`;
  if (bundleDigest !== manifest.validation.mcpBundleDigest) {
    throw new Error(
      `MCP bundle digest mismatch. Frozen ${manifest.validation.mcpBundleDigest}; current ${bundleDigest}. Recovery: rebuild and freeze a new manifest.`,
    );
  }
  const sandbox = new DockerSandboxAdapter({
    mcpBundlePath: bundlePath,
    expectedImage: manifest.sandbox.image,
  });
  const readiness = await sandbox.doctor();
  if (!readiness.ok) throw new Error(readiness.detail);
  for (const provider of manifest.providers) {
    const apiKey = provider.name === 'openai'
      ? environment.OPENAI_API_KEY
      : environment.ANTHROPIC_API_KEY;
    await verifyProviderModelAvailability({
      provider: provider.name,
      model: provider.model,
      apiKey: apiKey as string,
      timeoutMs: Math.min(15_000, manifest.limits.requestTimeoutMs),
      ...(options.fetchImpl ? { fetchImpl: options.fetchImpl } : {}),
    });
  }

  const runSetRoot = path.join(options.outputRoot, manifest.runSetId);
  const runsRoot = path.join(runSetRoot, 'runs');
  await mkdir(runsRoot, { recursive: true });
  await establishManifest(runSetRoot, manifest);
  const sink = new AtomicEvidenceSink(runsRoot);
  const receipts = await readExistingReceipts(runsRoot, manifest);
  const pendingCells = remainingRunCells(manifest, receipts);
  const completed = new Set(receipts.map(receipt => receipt.runId));
  let attributedCost = receipts.reduce(
    (total, receipt) => total + receipt.usage.estimatedCostMicros,
    0,
  );

  for (const cell of pendingCells) {
    if (
      attributedCost + manifest.budget.maximumMicros
      > manifest.budget.aggregateMaximumMicros
    ) {
      throw new Error(
        `Aggregate estimated-cost circuit breaker stopped before ${cell.runId}. Recovery: preserve this incomplete run set and review spend before freezing another manifest.`,
      );
    }
    const provider = manifest.providers.find(
      item => item.name === cell.provider,
    );
    const task = manifest.tasks.find(item => item.id === cell.taskId);
    if (!provider || !task) {
      throw new Error(`Frozen run cell references missing inputs: ${cell.runId}`);
    }
    const model = createModel(provider, environment);
    const taskRoot = path.resolve(options.repositoryRoot, task.taskRoot);
    const resolvedTasksRoot = path.resolve(tasksRoot);
    if (
      !taskRoot.startsWith(`${resolvedTasksRoot}${path.sep}`)
      || path.basename(taskRoot) !== task.id
    ) {
      throw new Error(
        `Frozen task root escapes the corpus boundary: ${task.taskRoot}`,
      );
    }
    const verifier = new DockerVerificationAdapter({
      image: manifest.sandbox.image,
      tasksRoot,
      verifierIdPrefix: 'docker-verifier',
    });
    const adjudicator = new DockerVerificationAdapter({
      image: manifest.sandbox.image,
      tasksRoot,
      verifierIdPrefix: 'docker-adjudicator',
    });
    const approvals = new ScriptedApprovalActor({
      actorId: `${manifest.runSetId}:scripted-policy`,
      runId: cell.runId,
      taskId: task.id,
      allowedKinds: ['plan', 'permission', 'tool_trust', 'runtime'],
      challengeSchedule: manifest.challengeSchedule,
      maximumRuntimeMicros: manifest.budget.maximumMicros,
    });
    const harness = createGovernedHarness({
      model,
      approvals,
      sandbox,
      verifier,
      adjudicator,
    });
    const spec: FrozenTrialSpec = {
      schemaVersion: 'boundarybench.trial.v0.1.0',
      runId: cell.runId,
      task: {
        id: task.id,
        instruction: task.instruction,
        baseDigest: task.baseDigest,
        taskRoot,
        allowedPaths: task.allowedPaths,
      },
      condition: cell.condition,
      provider: {
        name: provider.name,
        model: provider.model,
        effort: provider.effort,
      },
      sandbox: manifest.sandbox,
      limits: manifest.limits,
      budget: manifest.budget,
      protocolDigest: manifest.protocolDigest,
      challengeSchedule: manifest.challengeSchedule,
    };
    const receipt = await harness.runTrial(spec);
    await sink.write(receipt);
    receipts.push(receipt);
    completed.add(receipt.runId);
    attributedCost += receipt.usage.estimatedCostMicros;
  }
  return receipts;
}

function createModel(
  provider: FrozenExperimentManifest['providers'][number],
  environment: Record<string, string | undefined>,
): ModelAdapter {
  const pricing = provider.pricing;
  if (provider.name === 'openai') {
    const apiKey = environment.OPENAI_API_KEY;
    if (!apiKey) {
      throw new Error('OpenAI credentials disappeared after readiness checks.');
    }
    return new OpenAIResponsesAdapter({
      apiKey,
      model: provider.model,
      pricing,
    });
  }
  const apiKey = environment.ANTHROPIC_API_KEY;
  if (!apiKey) {
    throw new Error('Anthropic credentials disappeared after readiness checks.');
  }
  return new AnthropicMessagesAdapter({
    apiKey,
    model: provider.model,
    pricing,
  });
}

async function establishManifest(
  runSetRoot: string,
  manifest: FrozenExperimentManifest,
): Promise<void> {
  await mkdir(runSetRoot, { recursive: true });
  const manifestPath = path.join(runSetRoot, 'manifest.json');
  const serialized = canonicalJson(manifest);
  try {
    await writeFile(manifestPath, serialized, { flag: 'wx' });
  } catch (error) {
    if (
      !(error instanceof Error)
      || !('code' in error)
      || error.code !== 'EEXIST'
    ) {
      throw error;
    }
    const existing = await readFile(manifestPath, 'utf8');
    if (existing !== serialized) {
      throw new Error(
        `Run set ${manifest.runSetId} already exists with a different manifest.`,
      );
    }
  }
}

async function readExistingReceipts(
  runsRoot: string,
  manifest: FrozenExperimentManifest,
): Promise<TrialReceipt[]> {
  const planned = new Set(manifest.runOrder.map(cell => cell.runId));
  const entries = await readdir(runsRoot, { withFileTypes: true });
  const receipts: TrialReceipt[] = [];
  for (const entry of entries) {
    if (!entry.isDirectory() || entry.name.startsWith('.')) continue;
    if (!planned.has(entry.name)) {
      throw new Error(`Run directory is not present in the manifest: ${entry.name}`);
    }
    const receipt = JSON.parse(
      await readFile(
        path.join(runsRoot, entry.name, 'receipt.json'),
        'utf8',
      ),
    ) as TrialReceipt;
    if (!verifyTrialReceipt(receipt) || receipt.runId !== entry.name) {
      throw new Error(`Evidence digest mismatch for ${entry.name}.`);
    }
    receipts.push(receipt);
  }
  return receipts;
}

export function verifyTrialReceipt(receipt: TrialReceipt): boolean {
  const { evidenceDigest, ...withoutDigest } = receipt;
  return (
    digest(withoutDigest) === evidenceDigest
    && digest(receipt.trialSpec) === receipt.trialSpecDigest
  );
}

function sortValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortValue);
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, item]) => [key, sortValue(item)]),
    );
  }
  return value;
}

function canonicalJson(value: unknown): string {
  return `${JSON.stringify(sortValue(value), null, 2)}\n`;
}

function digest(value: unknown): string {
  return `sha256:${createHash('sha256')
    .update(canonicalJson(value))
    .digest('hex')}`;
}

function git(args: string[], cwd: string): Promise<{
  status: number;
  stdout: string;
  stderr: string;
}> {
  return new Promise(resolve => {
    const child = spawn('git', args, {
      cwd,
      env: {
        PATH: process.env.PATH ?? '/usr/local/bin:/usr/bin:/bin',
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let stdout = '';
    let stderr = '';
    child.stdout.setEncoding('utf8');
    child.stderr.setEncoding('utf8');
    child.stdout.on('data', value => {
      stdout += String(value);
    });
    child.stderr.on('data', value => {
      stderr += String(value);
    });
    child.once('error', error => {
      resolve({ status: 1, stdout, stderr: error.message });
    });
    child.once('close', code => {
      resolve({ status: code ?? 1, stdout, stderr });
    });
  });
}

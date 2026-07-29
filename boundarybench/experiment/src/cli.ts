import { spawn } from 'node:child_process';
import {
  mkdir,
  readFile,
  readdir,
  writeFile,
} from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import type { TrialReceipt } from '@governed-autonomy/coding-agent';

import { loadExperimentDraft } from './config.js';
import { loadAndValidateCorpus } from './corpus.js';
import {
  assertFrozenManifest,
  freezeExperiment,
} from './manifest.js';
import { writeRunSetReport } from './report.js';
import {
  runExperiment,
  verifyTrialReceipt,
} from './runner.js';
import type { FrozenExperimentManifest } from './types.js';

const repositoryRoot = fileURLToPath(new URL('../../../', import.meta.url));

function resolveRepositoryPath(value: string): string {
  return path.resolve(repositoryRoot, value);
}

function help(): string {
  return [
    'BoundaryBench exploratory experiment',
    '',
    'Usage:',
    '  boundarybench experiment freeze --config <config> --out <manifest>',
    '  boundarybench experiment run --manifest <manifest> [--output <directory>]',
    '  boundarybench experiment report --run-set <directory>',
    '  boundarybench experiment validate-corpus',
    '',
    'Live execution requires BOUNDARYBENCH_LIVE=1 and both provider API keys.',
    'Freeze and run refuse a dirty worktree. A frozen manifest requires an immutable Docker image digest.',
    'Freeze and run require a current first-party price snapshot.',
    'Relative option paths are resolved from the repository root.',
    '',
  ].join('\n');
}

function option(argv: string[], name: string): string {
  const index = argv.indexOf(name);
  const value = index < 0 ? undefined : argv[index + 1];
  if (!value || value.startsWith('--')) {
    throw new Error(`Missing required ${name} value.`);
  }
  return value;
}

async function git(args: string[]): Promise<string> {
  return new Promise((resolve, reject) => {
    const child = spawn('git', args, {
      cwd: repositoryRoot,
      env: { PATH: process.env.PATH ?? '/usr/local/bin:/usr/bin:/bin' },
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
    child.once('error', reject);
    child.once('close', code => {
      if (code === 0) resolve(stdout.trim());
      else reject(new Error(stderr.trim() || `git exited ${code}`));
    });
  });
}

async function assertCleanWorktree(): Promise<string> {
  const status = await git(['status', '--porcelain']);
  if (status.length > 0) {
    throw new Error(
      'Manifest freeze requires a clean worktree. Recovery: commit or remove local changes and retry.',
    );
  }
  return git(['rev-parse', 'HEAD']);
}

async function readManifest(file: string): Promise<FrozenExperimentManifest> {
  const value = JSON.parse(await readFile(file, 'utf8')) as unknown;
  assertFrozenManifest(value);
  return value;
}

async function readRunSetReceipts(
  runSetRoot: string,
  manifest: FrozenExperimentManifest,
): Promise<TrialReceipt[]> {
  const runsRoot = path.join(runSetRoot, 'runs');
  const planned = new Set(manifest.runOrder.map(cell => cell.runId));
  const entries = await readdir(runsRoot, { withFileTypes: true });
  const receipts: TrialReceipt[] = [];
  for (const entry of entries) {
    if (!entry.isDirectory() || entry.name.startsWith('.')) continue;
    if (!planned.has(entry.name)) {
      throw new Error(`Unexpected run directory: ${entry.name}`);
    }
    const receipt = JSON.parse(
      await readFile(path.join(runsRoot, entry.name, 'receipt.json'), 'utf8'),
    ) as TrialReceipt;
    if (receipt.runId !== entry.name || !verifyTrialReceipt(receipt)) {
      throw new Error(`Invalid evidence digest for ${entry.name}.`);
    }
    receipts.push(receipt);
  }
  return receipts;
}

async function freeze(argv: string[]): Promise<void> {
  const configPath = resolveRepositoryPath(option(argv, '--config'));
  const outputPath = resolveRepositoryPath(option(argv, '--out'));
  const commit = await assertCleanWorktree();
  process.stderr.write(
    'Validating pilot config, corpus, deterministic suite, fake-model suite, and MCP bundle...\n',
  );
  const draft = await loadExperimentDraft(
    configPath,
    repositoryRoot,
    commit,
  );
  const manifest = freezeExperiment(draft);
  await mkdir(path.dirname(outputPath), { recursive: true });
  await writeFile(
    outputPath,
    `${JSON.stringify(manifest, null, 2)}\n`,
    { flag: 'wx' },
  );
  process.stdout.write(
    `Frozen ${manifest.runSetId} with ${manifest.runOrder.length} trials at ${outputPath}\n`,
  );
}

async function run(argv: string[]): Promise<void> {
  const manifestPath = resolveRepositoryPath(option(argv, '--manifest'));
  const outputIndex = argv.indexOf('--output');
  const outputRoot = outputIndex < 0
    ? path.join(repositoryRoot, '.boundarybench', 'runs')
    : resolveRepositoryPath(option(argv, '--output'));
  const manifest = await readManifest(manifestPath);
  const receipts = await runExperiment(manifest, {
    repositoryRoot,
    outputRoot,
  });
  const runSetRoot = path.join(outputRoot, manifest.runSetId);
  const summary = await writeRunSetReport(runSetRoot, manifest, receipts);
  process.stdout.write(
    `Run set ${manifest.runSetId}: ${summary.evidencePackets}/${summary.plannedAttempts} complete evidence packets; claim authorized=${summary.claimAllowed}\n`,
  );
}

async function report(argv: string[]): Promise<void> {
  const runSetRoot = resolveRepositoryPath(option(argv, '--run-set'));
  const manifest = await readManifest(path.join(runSetRoot, 'manifest.json'));
  const receipts = await readRunSetReceipts(runSetRoot, manifest);
  const summary = await writeRunSetReport(runSetRoot, manifest, receipts);
  process.stdout.write(
    `Wrote ${path.join(runSetRoot, 'report.md')} (claim authorized=${summary.claimAllowed}).\n`,
  );
}

async function validateCorpus(): Promise<void> {
  const results = await loadAndValidateCorpus(
    path.join(repositoryRoot, 'boundarybench', 'tasks'),
  );
  for (const result of results) {
    const valid = (
      result.basePublicTestsFail
      && result.goldPatchApplies
      && result.goldPublicTestsPass
      && result.goldVerifierTestsPass
    );
    process.stdout.write(`${valid ? 'PASS' : 'FAIL'} ${result.task.id}\n`);
    if (!valid) process.exitCode = 1;
  }
}

export async function main(argv: string[]): Promise<number> {
  try {
    const command = argv[0] ?? '--help';
    if (command === '--help' || command === 'help') {
      process.stdout.write(help());
      return 0;
    }
    if (command === 'freeze') await freeze(argv.slice(1));
    else if (command === 'run') await run(argv.slice(1));
    else if (command === 'report') await report(argv.slice(1));
    else if (command === 'validate-corpus') await validateCorpus();
    else throw new Error(`Unknown experiment command: ${command}`);
    return typeof process.exitCode === 'number' ? process.exitCode : 0;
  } catch (error) {
    process.stderr.write(
      `BoundaryBench experiment error: ${error instanceof Error ? error.message : String(error)}\n`,
    );
    return 1;
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  process.exitCode = await main(process.argv.slice(2));
}

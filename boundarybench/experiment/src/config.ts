import { createHash } from 'node:crypto';
import { spawn } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import path from 'node:path';

import { z } from 'zod';

import { loadAndValidateCorpus } from './corpus.js';
import { assertPricingSnapshotCurrent } from './pricing.js';
import type { ExperimentDraft } from './types.js';

const digest = z.string().regex(/^sha256:[0-9a-f]{64}$/);
const isoDate = z.string()
  .regex(/^\d{4}-\d{2}-\d{2}$/)
  .refine(value => {
    const parsed = new Date(`${value}T00:00:00.000Z`);
    return (
      !Number.isNaN(parsed.getTime())
      && parsed.toISOString().slice(0, 10) === value
    );
  }, 'Use a real calendar date in YYYY-MM-DD format.');
const provider = z.discriminatedUnion('name', [
  z.object({
    name: z.literal('openai'),
    model: z.literal('gpt-5.6-terra'),
    effort: z.literal('medium'),
    pricing: z.object({
      inputPerMillion: z.number().nonnegative(),
      cachedInputPerMillion: z.number().nonnegative(),
      cacheWritePerMillion: z.number().nonnegative(),
      outputPerMillion: z.number().nonnegative(),
    }).strict(),
  }).strict(),
  z.object({
    name: z.literal('anthropic'),
    model: z.literal('claude-sonnet-5'),
    effort: z.literal('medium'),
    pricing: z.object({
      inputPerMillion: z.number().nonnegative(),
      cachedInputPerMillion: z.number().nonnegative(),
      cacheWritePerMillion: z.number().nonnegative(),
      outputPerMillion: z.number().nonnegative(),
    }).strict(),
  }).strict(),
]);

export const experimentConfigSchema = z.object({
  schemaVersion: z.literal('boundarybench.experiment-config.v0.1.0'),
  seed: z.string().min(1),
  providers: z.array(provider).length(2),
  pricingSnapshot: z.object({
    checkedAt: isoDate,
    validThrough: isoDate,
    sources: z.object({
      openai: z.string().url(),
      anthropic: z.string().url(),
    }).strict(),
  }).strict(),
  conditions: z.array(z.enum([
    'governed',
    'record_only_plan',
    'record_only_permission',
    'record_only_tool_trust',
    'record_only_verification',
    'record_only_runtime',
  ])).length(6),
  challengeSchedule: z.array(z.enum([
    'plan',
    'permission',
    'tool_trust',
    'verification',
    'runtime',
  ])).length(5),
  sandbox: z.object({
    image: z.string().regex(
      /^[^@\s]+@sha256:[0-9a-f]{64}$/,
      'Use an immutable image reference such as node@sha256:<64 hex characters>.',
    ),
    imageDigest: digest,
    cpus: z.number().positive(),
    memoryMb: z.number().int().positive(),
    pidsLimit: z.number().int().positive(),
  }).strict(),
  limits: z.object({
    maxModelTurns: z.number().int().positive(),
    maxToolCalls: z.number().int().positive(),
    maxOutputTokensPerTurn: z.number().int().positive(),
    maxInputTokens: z.number().int().positive(),
    maxOutputTokens: z.number().int().positive(),
    requestTimeoutMs: z.number().int().positive(),
    trialTimeoutMs: z.number().int().positive(),
  }).strict(),
  budget: z.object({
    initialMicros: z.number().int().nonnegative(),
    maximumMicros: z.number().int().positive(),
    aggregateMaximumMicros: z.number().int().positive(),
    warnMicros: z.number().int().nonnegative(),
  }).strict(),
  redactionPolicy: z.literal(
    'provider-visible-content-no-hidden-reasoning',
  ),
  reportVersion: z.literal('0.1.0'),
}).strict().superRefine((value, context) => {
  if (value.pricingSnapshot.checkedAt > value.pricingSnapshot.validThrough) {
    context.addIssue({
      code: 'custom',
      path: ['pricingSnapshot'],
      message: 'checkedAt must be on or before validThrough.',
    });
  }
  if (!value.sandbox.image.endsWith(`@${value.sandbox.imageDigest}`)) {
    context.addIssue({
      code: 'custom',
      path: ['sandbox', 'imageDigest'],
      message: 'imageDigest must match the digest embedded in sandbox.image.',
    });
  }
  if (
    value.budget.warnMicros > value.budget.initialMicros
    || value.budget.initialMicros > value.budget.maximumMicros
    || value.budget.maximumMicros > value.budget.aggregateMaximumMicros
  ) {
    context.addIssue({
      code: 'custom',
      path: ['budget'],
      message: 'Budget values must be ordered warn <= initial <= maximum <= aggregate.',
    });
  }
});

export type ExperimentConfig = z.infer<typeof experimentConfigSchema>;

export async function loadExperimentDraft(
  configPath: string,
  repositoryRoot: string,
  harnessCommit: string,
): Promise<ExperimentDraft> {
  const config = experimentConfigSchema.parse(
    JSON.parse(await readFile(configPath, 'utf8')),
  );
  assertPricingSnapshotCurrent(config.pricingSnapshot);
  const protocolBytes = await readFile(
    path.join(repositoryRoot, 'boundarybench', 'protocol', 'v0.1.0.json'),
  );
  const validations = await loadAndValidateCorpus(
    path.join(repositoryRoot, 'boundarybench', 'tasks'),
  );
  const invalid = validations.filter(result => (
    !result.basePublicTestsFail
    || !result.goldPatchApplies
    || !result.goldPublicTestsPass
    || !result.goldVerifierTestsPass
  ));
  if (invalid.length > 0) {
    throw new Error(
      `Corpus validation failed for: ${invalid.map(item => item.task.id).join(', ')}.`,
    );
  }
  const deterministicCommand = 'npm run test:deterministic';
  const fakeModelCommand = 'npm run test:harness';
  const buildCommand = 'npm run build --workspace @governed-autonomy/coding-agent';
  const deterministicOutput = await runValidation(
    deterministicCommand,
    repositoryRoot,
  );
  const fakeModelOutput = await runValidation(
    fakeModelCommand,
    repositoryRoot,
  );
  const buildOutput = await runValidation(buildCommand, repositoryRoot);
  const mcpBundle = await readFile(path.join(
    repositoryRoot,
    'examples',
    'governed-coding-agent',
    'dist',
    'mcp-server.bundle.mjs',
  ));

  return {
    ...config,
    schemaVersion: 'boundarybench.experiment-draft.v0.1.0',
    protocolDigest: `sha256:${createHash('sha256')
      .update(protocolBytes)
      .digest('hex')}`,
    harnessCommit,
    tasks: validations.map(({ task }) => ({
      ...task,
      taskRoot: path.relative(repositoryRoot, task.taskRoot),
    })),
    validation: {
      commit: harnessCommit,
      deterministicCommand,
      deterministicOutputDigest: sha256(deterministicOutput),
      fakeModelCommand,
      fakeModelOutputDigest: sha256(fakeModelOutput),
      buildCommand,
      buildOutputDigest: sha256(buildOutput),
      mcpBundleDigest: sha256(mcpBundle),
    },
  };
}

function sha256(value: string | Uint8Array): string {
  return `sha256:${createHash('sha256').update(value).digest('hex')}`;
}

function runValidation(command: string, cwd: string): Promise<string> {
  const [executable, ...args] = command.split(' ');
  return new Promise((resolve, reject) => {
    const child = spawn(executable as string, args, {
      cwd,
      env: {
        PATH: process.env.PATH ?? '/usr/local/bin:/usr/bin:/bin',
        CI: '1',
        NO_COLOR: '1',
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let output = '';
    child.stdout.setEncoding('utf8');
    child.stderr.setEncoding('utf8');
    child.stdout.on('data', value => {
      output += String(value);
    });
    child.stderr.on('data', value => {
      output += String(value);
    });
    child.once('error', reject);
    child.once('close', code => {
      if (code === 0) resolve(output);
      else {
        reject(new Error(
          `Freeze validation failed: ${command}\n${output.trim()}`,
        ));
      }
    });
  });
}

#!/usr/bin/env node
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  AnthropicMessagesAdapter,
  AtomicEvidenceSink,
  createGovernedHarness,
  digestDirectory,
  DockerSandboxAdapter,
  DockerVerificationAdapter,
  InteractiveApprovalActor,
  OpenAIResponsesAdapter,
  verifyProviderModelAvailability,
} from './index.js';
import type {
  FrozenTrialSpec,
  ModelAdapter,
} from './types.js';

const repositoryRoot = fileURLToPath(new URL('../../../', import.meta.url));
const bundlePath = path.join(
  repositoryRoot,
  'examples',
  'governed-coding-agent',
  'dist',
  'mcp-server.bundle.mjs',
);

function help(): string {
  return [
    'Governed coding-agent harness',
    '',
    'Usage:',
    '  govern-agent doctor [--provider <openai|anthropic>]',
    '  govern-agent run --task <task> --provider <openai|anthropic> --approval interactive',
    '',
    'Run requires BOUNDARYBENCH_LIVE=1 and GOVERN_DOCKER_IMAGE=name@sha256:<digest>.',
    'The harness never mounts provider credentials into task, MCP, verifier, or adjudicator containers.',
    '',
  ].join('\n');
}

function optionalOption(argv: string[], name: string): string | undefined {
  const index = argv.indexOf(name);
  if (index < 0) return undefined;
  const value = argv[index + 1];
  if (!value || value.startsWith('--')) {
    throw new Error(`Missing ${name} value.`);
  }
  return value;
}

function requiredOption(argv: string[], name: string): string {
  const value = optionalOption(argv, name);
  if (!value) throw new Error(`Missing required ${name} value.`);
  return value;
}

function providerKey(
  provider: 'openai' | 'anthropic',
): string | undefined {
  return provider === 'openai'
    ? process.env.OPENAI_API_KEY
    : process.env.ANTHROPIC_API_KEY;
}

function providerModel(provider: 'openai' | 'anthropic'): string {
  return provider === 'openai' ? 'gpt-5.6-terra' : 'claude-sonnet-5';
}

function parseProvider(value: string): 'openai' | 'anthropic' {
  if (value !== 'openai' && value !== 'anthropic') {
    throw new Error(`Unsupported provider: ${value}`);
  }
  return value;
}

function parseImage(): { image: string; imageDigest: string } {
  const image = process.env.GOVERN_DOCKER_IMAGE;
  const match = /^([^@\s]+)@(sha256:[0-9a-f]{64})$/.exec(image ?? '');
  if (!image || !match?.[2]) {
    throw new Error(
      'GOVERN_DOCKER_IMAGE must be an immutable name@sha256:<64 hex characters> reference.',
    );
  }
  return { image, imageDigest: match[2] };
}

async function readiness(
  providerValue: string | undefined,
): Promise<boolean> {
  const image = process.env.GOVERN_DOCKER_IMAGE;
  const sandbox = new DockerSandboxAdapter({
    mcpBundlePath: bundlePath,
    ...(image ? { expectedImage: image } : {}),
  });
  const docker = await sandbox.doctor();
  process.stdout.write(
    `${docker.ok ? 'PASS' : 'FAIL'} docker-and-mcp: ${docker.detail}\n`,
  );
  const nodeReady = Number(process.versions.node.split('.')[0]) >= 20;
  process.stdout.write(
    `${nodeReady ? 'PASS' : 'FAIL'} node: ${process.version} (requires >=20)\n`,
  );
  if (!image) {
    process.stdout.write(
      'FAIL image: set GOVERN_DOCKER_IMAGE to an immutable name@sha256:<digest> reference.\n',
    );
  }
  if (!providerValue) {
    process.stdout.write(
      'INFO provider: pass --provider to validate one exact model and credential.\n',
    );
    return docker.ok && nodeReady && Boolean(image);
  }
  const provider = parseProvider(providerValue);
  const key = providerKey(provider);
  if (!key) {
    process.stdout.write(
      `FAIL ${provider}: required API key is missing from the host environment.\n`,
    );
    return false;
  }
  try {
    await verifyProviderModelAvailability({
      provider,
      model: providerModel(provider),
      apiKey: key,
    });
    process.stdout.write(
      `PASS ${provider}: exact model ${providerModel(provider)} is available.\n`,
    );
  } catch (error) {
    process.stdout.write(
      `FAIL ${provider}: ${error instanceof Error ? error.message : String(error)}\n`,
    );
    return false;
  }
  return docker.ok && nodeReady && Boolean(image);
}

async function run(argv: string[]): Promise<number> {
  if (process.env.BOUNDARYBENCH_LIVE !== '1') {
    throw new Error(
      'Live execution is disabled. Review the task and set BOUNDARYBENCH_LIVE=1 explicitly.',
    );
  }
  const taskId = requiredOption(argv, '--task');
  if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(taskId)) {
    throw new Error('Task ID must contain only lowercase words and hyphens.');
  }
  const provider = parseProvider(requiredOption(argv, '--provider'));
  if (requiredOption(argv, '--approval') !== 'interactive') {
    throw new Error('The demonstration CLI accepts only --approval interactive.');
  }
  const key = providerKey(provider);
  if (!key) throw new Error(`Missing ${provider.toUpperCase()} API key.`);
  const { image, imageDigest } = parseImage();
  const ready = await readiness(provider);
  if (!ready) return 1;

  const taskRoot = path.join(
    repositoryRoot,
    'boundarybench',
    'tasks',
    taskId,
  );
  const metadata = JSON.parse(
    await readFile(path.join(taskRoot, 'task.json'), 'utf8'),
  ) as {
    id?: unknown;
    instruction?: unknown;
    allowedPaths?: unknown;
  };
  if (
    metadata.id !== taskId
    || typeof metadata.instruction !== 'string'
    || !Array.isArray(metadata.allowedPaths)
    || !metadata.allowedPaths.every(item => typeof item === 'string')
  ) {
    throw new Error(`Task metadata is invalid or missing for ${taskId}.`);
  }
  const modelName = providerModel(provider);
  const model: ModelAdapter = provider === 'openai'
    ? new OpenAIResponsesAdapter({ apiKey: key, model: modelName })
    : new AnthropicMessagesAdapter({ apiKey: key, model: modelName });
  const protocol = await readFile(
    path.join(repositoryRoot, 'boundarybench', 'protocol', 'v0.1.0.json'),
  );
  const runId = `demo-${taskId}-${provider}-${createHash('sha256')
    .update(`${Date.now()}:${process.pid}`)
    .digest('hex')
    .slice(0, 12)}`;
  const spec: FrozenTrialSpec = {
    schemaVersion: 'boundarybench.trial.v0.1.0',
    runId,
    task: {
      id: taskId,
      instruction: metadata.instruction,
      baseDigest: await digestDirectory(path.join(taskRoot, 'workspace')),
      taskRoot,
      allowedPaths: metadata.allowedPaths as string[],
    },
    condition: 'governed',
    provider: {
      name: provider,
      model: modelName,
      effort: 'medium',
    },
    sandbox: {
      image,
      imageDigest,
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
    protocolDigest: `sha256:${createHash('sha256')
      .update(protocol)
      .digest('hex')}`,
    challengeSchedule: [
      'plan',
      'permission',
      'tool_trust',
      'runtime',
      'verification',
    ],
  };
  const sandbox = new DockerSandboxAdapter({
    mcpBundlePath: bundlePath,
    expectedImage: image,
  });
  const tasksRoot = path.join(repositoryRoot, 'boundarybench', 'tasks');
  const harness = createGovernedHarness({
    model,
    approvals: new InteractiveApprovalActor({
      actorId: process.env.USER ?? 'interactive-operator',
    }),
    sandbox,
    verifier: new DockerVerificationAdapter({
      image,
      tasksRoot,
      verifierIdPrefix: 'docker-verifier',
    }),
    adjudicator: new DockerVerificationAdapter({
      image,
      tasksRoot,
      verifierIdPrefix: 'docker-adjudicator',
    }),
  });
  const receipt = await harness.runTrial(spec);
  const outputRoot = path.join(
    repositoryRoot,
    '.boundarybench',
    'runs',
    'interactive',
  );
  const output = await new AtomicEvidenceSink(outputRoot).write(receipt);
  process.stdout.write(
    `Trial ${runId} ended ${receipt.terminalStatus}; evidence: ${output}\n`,
  );
  return receipt.terminalStatus === 'task_succeeded' ? 0 : 1;
}

export async function main(argv: string[]): Promise<number> {
  try {
    const command = argv[0] ?? '--help';
    if (command === '--help' || command === 'help') {
      process.stdout.write(help());
      return 0;
    }
    if (command === 'doctor') {
      return await readiness(optionalOption(argv.slice(1), '--provider'))
        ? 0
        : 1;
    }
    if (command === 'run') return run(argv.slice(1));
    throw new Error(`Unknown command: ${command}`);
  } catch (error) {
    process.stderr.write(
      `govern-agent error: ${error instanceof Error ? error.message : String(error)}\n`,
    );
    return 1;
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  process.exitCode = await main(process.argv.slice(2));
}

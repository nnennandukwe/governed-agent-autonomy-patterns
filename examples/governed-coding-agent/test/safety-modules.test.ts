import assert from 'node:assert/strict';
import {
  mkdtemp,
  readFile,
  readdir,
  symlink,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

import { ScriptedApprovalActor } from '../src/approvals.js';
import { AtomicEvidenceSink } from '../src/evidence.js';
import {
  createRepositoryTools,
  resolveWorkspacePath,
} from '../src/repository-tools.js';
import type { TrialReceipt } from '../src/types.js';
import {
  canonicalizeAllowedPathPattern,
  canonicalizeWorkspaceRelativePath,
} from '../src/workspace-paths.js';

const digest = `sha256:${'a'.repeat(64)}`;

test('workspace paths stay inside the declared repository root', async t => {
  const root = await mkdtemp(path.join(tmpdir(), 'govern-path-test-'));
  await writeFile(path.join(root, 'inside.txt'), 'safe');

  assert.equal(
    resolveWorkspacePath(root, 'inside.txt'),
    path.join(root, 'inside.txt'),
  );
  assert.throws(
    () => resolveWorkspacePath(root, '../outside.txt'),
    /escapes the workspace/,
  );
  assert.throws(
    () => resolveWorkspacePath(root, 'src/../package.json'),
    /safe workspace-relative path/,
  );
  assert.throws(
    () => resolveWorkspacePath(root, 'src\\..\\package.json'),
    /safe workspace-relative path/,
  );
  assert.throws(
    () => resolveWorkspacePath(root, '/etc/passwd'),
    /must be relative/,
  );
  assert.throws(
    () => resolveWorkspacePath(root, 'C:\\Windows\\system.ini'),
    /must be relative/,
  );
  assert.throws(
    () => resolveWorkspacePath(root, 'C:Windows\\system.ini'),
    /must be relative/,
  );
  assert.throws(
    () => resolveWorkspacePath(root, '.git/config'),
    /reserved \.git/,
  );
  assert.equal(
    canonicalizeWorkspaceRelativePath('src\\nested//value.js'),
    'src/nested/value.js',
  );
  assert.deepEqual(
    canonicalizeAllowedPathPattern('src\\**'),
    { pathname: 'src', recursive: true },
  );
  t.after(async () => {
    const { rm } = await import('node:fs/promises');
    await rm(root, { recursive: true, force: true });
  });
});

test('repository tools apply checked patches and reject shell or symlink escapes', async t => {
  const root = await mkdtemp(path.join(tmpdir(), 'govern-tools-test-'));
  const outside = await mkdtemp(path.join(tmpdir(), 'govern-tools-outside-'));
  t.after(async () => {
    const { rm } = await import('node:fs/promises');
    await Promise.all([
      rm(root, { recursive: true, force: true }),
      rm(outside, { recursive: true, force: true }),
    ]);
  });
  await writeFile(path.join(root, 'value.js'), 'export const value = "wrong";\n');
  await writeFile(
    path.join(root, 'mutate.js'),
    [
      "'use strict';",
      "require('node:fs').writeFileSync('value.js', 'mutated by test\\n');",
      '',
    ].join('\n'),
  );
  await writeFile(path.join(outside, 'secret.txt'), 'secret\n');
  await symlink(outside, path.join(root, 'link-out'));
  for (const args of [
    ['init', '--quiet'],
    ['config', 'user.email', 'fixture@example.test'],
    ['config', 'user.name', 'Fixture'],
    ['add', '.'],
    ['commit', '--quiet', '-m', 'base'],
  ]) {
    const result = spawnSync('git', args, { cwd: root, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
  }
  const tools = createRepositoryTools(root);

  const deniedTraversalPatch = await tools.call('repo.apply_patch', {
    patch: [
      'diff --git a/src/../package.json b/src/../package.json',
      'new file mode 100644',
      '--- /dev/null',
      '+++ b/src/../package.json',
      '@@ -0,0 +1 @@',
      '+{"private":true}',
      '',
    ].join('\n'),
  });
  assert.equal(deniedTraversalPatch.isError, true);
  assert.match(
    deniedTraversalPatch.content,
    /safe workspace-relative path/,
  );
  await assert.rejects(
    () => readFile(path.join(root, 'package.json'), 'utf8'),
    /ENOENT/,
  );

  const patchResult = await tools.call('repo.apply_patch', {
    patch: [
      'diff --git a/value.js b/value.js',
      'index 5e993b0..c5d46db 100644',
      '--- a/value.js',
      '+++ b/value.js',
      '@@ -1 +1 @@',
      '-export const value = "wrong";',
      '+export const value = "right";',
      '',
    ].join('\n'),
  });

  assert.equal(patchResult.isError, undefined);
  assert.equal(
    await readFile(path.join(root, 'value.js'), 'utf8'),
    'export const value = "right";\n',
  );
  const diff = await tools.call('repo.diff', {});
  assert.match(diff.content, /value = "right"/);
  const deniedRun = await tools.call('repo.run', {
    executable: 'sh',
    args: ['-c', 'echo nope'],
    cwd: '.',
  });
  assert.equal(deniedRun.isError, true);
  assert.match(deniedRun.content, /not allowed/);
  const isolatedRun = await tools.call('repo.run', {
    executable: 'node',
    args: ['mutate.js'],
    cwd: '.',
  });
  assert.equal(isolatedRun.isError, undefined);
  assert.equal(
    await readFile(path.join(root, 'value.js'), 'utf8'),
    'export const value = "right";\n',
  );
  const deniedRead = await tools.call('repo.read', {
    path: 'link-out/secret.txt',
  });
  assert.equal(deniedRead.isError, true);
  assert.match(deniedRead.content, /symbolic link|escapes the workspace/);
});

test('scripted approval denies requests outside its frozen policy', async () => {
  const actor = new ScriptedApprovalActor({
    actorId: 'pilot-policy-v1',
    runId: 'run-1',
    taskId: 'task-1',
    allowedKinds: ['plan', 'permission', 'tool_trust', 'runtime'],
    challengeSchedule: [
      'plan',
      'permission',
      'tool_trust',
      'runtime',
      'verification',
    ],
    maximumRuntimeMicros: 3_000_000,
  });
  const base = {
    id: digest,
    subjectDigest: digest,
    eventSequence: 4,
  };

  const allowed = await actor.decide({
    ...base,
    kind: 'runtime',
    scope: {
      runId: 'run-1',
      taskId: 'task-1',
      maxCostMicros: 3_000_000,
    },
  });
  assert.equal(allowed.decision, 'approved');
  assert.equal(allowed.actorType, 'scripted-evaluator');

  const denied = await actor.decide({
    ...base,
    kind: 'runtime',
    scope: {
      runId: 'run-1',
      taskId: 'task-1',
      maxCostMicros: 3_000_001,
    },
  });
  assert.equal(denied.decision, 'denied');
  assert.match(denied.policyRuleId, /runtime_limit/);
});

test('evidence is finalized atomically and refuses an existing run directory', async t => {
  const root = await mkdtemp(path.join(tmpdir(), 'govern-evidence-test-'));
  t.after(async () => {
    const { rm } = await import('node:fs/promises');
    await rm(root, { recursive: true, force: true });
  });
  const receipt = {
    schemaVersion: 'boundarybench.receipt.v0.1.0',
    runId: 'run-1',
    phase: 'terminal',
    terminalStatus: 'task_succeeded',
    taskId: 'task-1',
    condition: 'governed',
    provider: {
      name: 'scripted',
      model: 'scripted-model-v1',
      effort: 'medium',
    },
    trialSpec: {
      schemaVersion: 'boundarybench.trial.v0.1.0',
      runId: 'run-1',
      task: {
        id: 'task-1',
        instruction: 'fixture',
        baseDigest: digest,
        allowedPaths: ['src/**'],
      },
      condition: 'governed',
      provider: {
        name: 'scripted',
        model: 'scripted-model-v1',
        effort: 'medium',
      },
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
      protocolDigest: digest,
      challengeSchedule: [
        'plan',
        'permission',
        'tool_trust',
        'runtime',
        'verification',
      ],
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
  } satisfies TrialReceipt;
  const sink = new AtomicEvidenceSink(root);

  const finalPath = await sink.write(receipt);

  assert.equal(finalPath, path.join(root, 'run-1'));
  assert.deepEqual(await readdir(root), ['run-1']);
  assert.deepEqual(
    JSON.parse(await readFile(path.join(finalPath, 'receipt.json'), 'utf8')),
    receipt,
  );
  await assert.rejects(
    () => sink.write(receipt),
    /already exists/,
  );
  assert.deepEqual(await readdir(root), ['run-1']);
});

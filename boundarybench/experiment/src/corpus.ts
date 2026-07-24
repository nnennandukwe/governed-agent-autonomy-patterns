import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  cp,
  mkdtemp,
  readFile,
  readdir,
  rm,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';

import { digestDirectory } from '@governed-autonomy/coding-agent';

import type { ExperimentTask } from './types.js';

export interface CorpusValidation {
  task: ExperimentTask;
  basePublicTestsFail: boolean;
  goldPatchApplies: boolean;
  goldPublicTestsPass: boolean;
  goldVerifierTestsPass: boolean;
}

export async function loadAndValidateCorpus(
  tasksRoot: string,
): Promise<CorpusValidation[]> {
  const taskEntries = await readdir(tasksRoot, { withFileTypes: true });
  const taskDirectories = taskEntries
    .filter(entry => entry.isDirectory())
    .map(entry => path.join(tasksRoot, entry.name))
    .sort();

  return Promise.all(taskDirectories.map(async taskRoot => {
    const metadata = JSON.parse(
      await readFile(path.join(taskRoot, 'task.json'), 'utf8'),
    ) as {
      id?: unknown;
      instruction?: unknown;
      allowedPaths?: unknown;
      publicTestCommand?: unknown;
    };
    if (
      typeof metadata.id !== 'string'
      || typeof metadata.instruction !== 'string'
      || !Array.isArray(metadata.allowedPaths)
      || !metadata.allowedPaths.every(item => typeof item === 'string')
      || !Array.isArray(metadata.publicTestCommand)
      || !metadata.publicTestCommand.every(item => typeof item === 'string')
      || metadata.publicTestCommand.length === 0
    ) {
      throw new Error(`Invalid task metadata at ${taskRoot}/task.json.`);
    }
    const goldPatch = await readFile(
      path.join(taskRoot, 'gold.patch'),
      'utf8',
    );
    const verifierFiles = (await readdir(
      path.join(taskRoot, 'verifier'),
      { withFileTypes: true },
    ))
      .filter(entry => entry.isFile() && entry.name.endsWith('.test.js'))
      .map(entry => entry.name)
      .sort();
    const verifierDigest = sha256(await Promise.all(
      verifierFiles.map(async file => ({
        file,
        content: await readFile(
          path.join(taskRoot, 'verifier', file),
          'utf8',
        ),
      })),
    ));
    const task: ExperimentTask = {
      id: metadata.id,
      instruction: metadata.instruction,
      taskRoot,
      baseDigest: await digestDirectory(path.join(taskRoot, 'workspace')),
      goldPatchDigest: sha256(goldPatch),
      verifierDigest,
      allowedPaths: metadata.allowedPaths as string[],
    };

    const temporaryRoot = await mkdtemp(
      path.join(tmpdir(), `boundarybench-${task.id}-`),
    );
    const temporaryTask = path.join(temporaryRoot, task.id);
    await cp(taskRoot, temporaryTask, { recursive: true });
    const workspace = path.join(temporaryTask, 'workspace');
    try {
      const publicCommand = metadata.publicTestCommand as string[];
      const basePublic = await run(
        publicCommand[0] as string,
        publicCommand.slice(1),
        workspace,
      );
      const patchCheck = await run(
        'git',
        ['apply', '--check', path.join(temporaryTask, 'gold.patch')],
        workspace,
      );
      const patchApply = patchCheck.status === 0
        ? await run(
            'git',
            ['apply', path.join(temporaryTask, 'gold.patch')],
            workspace,
          )
        : patchCheck;
      const goldPublic = patchApply.status === 0
        ? await run(
            publicCommand[0] as string,
            publicCommand.slice(1),
            workspace,
          )
        : patchApply;
      const goldVerifier = patchApply.status === 0
        ? await run(
            process.execPath,
            [
              '--test',
              ...verifierFiles.map(file => (
                path.join(temporaryTask, 'verifier', file)
              )),
            ],
            workspace,
            { GOVERN_WORKSPACE: workspace },
          )
        : patchApply;
      return {
        task,
        basePublicTestsFail: basePublic.status !== 0,
        goldPatchApplies: patchCheck.status === 0 && patchApply.status === 0,
        goldPublicTestsPass: goldPublic.status === 0,
        goldVerifierTestsPass: goldVerifier.status === 0,
      };
    } finally {
      await rm(temporaryRoot, { recursive: true, force: true });
    }
  }));
}

function sha256(value: unknown): string {
  const bytes = typeof value === 'string'
    ? value
    : JSON.stringify(value);
  return `sha256:${createHash('sha256').update(bytes).digest('hex')}`;
}

function run(
  executable: string,
  args: string[],
  cwd: string,
  environment: Record<string, string> = {},
): Promise<{ status: number; stdout: string; stderr: string }> {
  return new Promise(resolve => {
    const child = spawn(executable, args, {
      cwd,
      env: {
        PATH: process.env.PATH ?? '/usr/local/bin:/usr/bin:/bin',
        CI: '1',
        NO_COLOR: '1',
        ...environment,
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

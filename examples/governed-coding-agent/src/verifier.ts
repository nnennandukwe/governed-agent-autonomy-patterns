import { spawn } from 'node:child_process';
import { readdir } from 'node:fs/promises';
import path from 'node:path';

import type {
  VerificationAdapter,
  VerificationReceipt,
  VerificationSubject,
} from './types.js';

interface VerificationOptions {
  image: string;
  tasksRoot: string;
  verifierIdPrefix?: string;
}

function docker(args: string[]): Promise<{
  status: number;
  stdout: string;
  stderr: string;
}> {
  return new Promise(resolve => {
    const child = spawn('docker', args, {
      env: {
        PATH: process.env.PATH ?? '/usr/local/bin:/usr/bin:/bin',
        CI: '1',
        NO_COLOR: '1',
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

async function testFiles(directory: string, containerRoot: string) {
  const entries = await readdir(directory, { withFileTypes: true });
  return entries
    .filter(entry => entry.isFile() && entry.name.endsWith('.test.js'))
    .map(entry => path.posix.join(containerRoot, entry.name))
    .sort();
}

export class DockerVerificationAdapter implements VerificationAdapter {
  constructor(private readonly options: VerificationOptions) {}

  async verify(
    subject: VerificationSubject,
  ): Promise<VerificationReceipt> {
    const verifierRoot = path.join(
      this.options.tasksRoot,
      subject.taskId,
      'verifier',
    );
    const publicTests = await testFiles(
      path.join(subject.workspacePath, 'test'),
      '/workspace/test',
    );
    const verifierTests = await testFiles(verifierRoot, '/verifier');
    const command = ['node', '--test', ...publicTests, ...verifierTests];
    const created = await docker([
      'create',
      '--network',
      'none',
      '--read-only',
      '--cap-drop',
      'ALL',
      '--security-opt',
      'no-new-privileges',
      '--user',
      '1000:1000',
      '--cpus',
      '1',
      '--memory',
      '512m',
      '--pids-limit',
      '128',
      '--tmpfs',
      '/tmp:rw,nosuid,nodev,noexec,size=64m',
      '--mount',
      `type=bind,src=${subject.workspacePath},dst=/workspace,readonly`,
      '--mount',
      `type=bind,src=${verifierRoot},dst=/verifier,readonly`,
      '--workdir',
      '/workspace',
      this.options.image,
      ...command,
    ]);
    if (created.status !== 0) {
      throw new Error(`Could not create verifier container: ${created.stderr}`);
    }
    const containerId = created.stdout.trim();
    try {
      const executed = await docker(['start', '--attach', containerId]);
      const output = `${executed.stdout}${executed.stderr}`.trim();
      const passed = executed.status === 0;
      return {
        verifierId: `${this.options.verifierIdPrefix ?? 'docker-verifier'}:${containerId}`,
        subjectDigest: subject.subjectDigest,
        verdict: passed ? 'PASS' : 'FAIL',
        evidence: [{
          command: command.join(' '),
          output: output || '(no output)',
          result: passed ? 'PASS' : 'FAIL',
        }],
      };
    } finally {
      const removed = await docker(['rm', '--force', containerId]);
      if (
        removed.status !== 0
        && !/No such container/.test(removed.stderr)
      ) {
        throw new Error(
          `Verifier cleanup failed for ${containerId}: ${removed.stderr.trim()}`,
        );
      }
    }
  }
}

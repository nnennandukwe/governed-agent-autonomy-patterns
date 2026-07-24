import { spawn } from 'node:child_process';
import {
  cp,
  mkdtemp,
  readdir,
  readFile,
  readlink,
  rm,
  stat,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';

import { digestObject } from './canonical.js';
import { StdioRepositoryToolClient } from './mcp-client.js';
import type {
  FrozenTrialSpec,
  SandboxAdapter,
  SandboxSession,
  ToolClient,
} from './types.js';

interface DockerCommandResult {
  status: number;
  stdout: string;
  stderr: string;
}

function run(
  executable: string,
  args: string[],
  options: {
    cwd?: string;
    timeoutMs?: number;
  } = {},
): Promise<DockerCommandResult> {
  return new Promise(resolve => {
    const child = spawn(executable, args, {
      ...(options.cwd ? { cwd: options.cwd } : {}),
      env: {
        PATH: process.env.PATH ?? '/usr/local/bin:/usr/bin:/bin',
        CI: '1',
        NO_COLOR: '1',
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let stdout = '';
    let stderr = '';
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
    }, options.timeoutMs ?? 120_000);
    child.stdout.setEncoding('utf8');
    child.stderr.setEncoding('utf8');
    child.stdout.on('data', value => {
      stdout += String(value);
    });
    child.stderr.on('data', value => {
      stderr += String(value);
    });
    child.once('error', error => {
      clearTimeout(timer);
      resolve({ status: 1, stdout, stderr: error.message });
    });
    child.once('close', code => {
      clearTimeout(timer);
      resolve({ status: code ?? 1, stdout, stderr });
    });
  });
}

interface DigestEntry {
  path: string;
  type: 'file' | 'symlink';
}

async function listDigestEntries(
  root: string,
  directory = root,
): Promise<DigestEntry[]> {
  const entries = await readdir(directory, { withFileTypes: true });
  const files: DigestEntry[] = [];
  for (const entry of entries) {
    if (entry.name === '.git') continue;
    const absolute = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...await listDigestEntries(root, absolute));
    } else if (entry.isFile()) {
      files.push({
        path: path.relative(root, absolute),
        type: 'file',
      });
    } else if (entry.isSymbolicLink()) {
      files.push({
        path: path.relative(root, absolute),
        type: 'symlink',
      });
    }
  }
  return files.sort((left, right) => left.path.localeCompare(right.path));
}

export async function digestDirectory(root: string): Promise<string> {
  const files = await listDigestEntries(root);
  const contents = await Promise.all(
    files.map(async entry => ({
      path: entry.path,
      type: entry.type,
      content: entry.type === 'file'
        ? (await readFile(path.join(root, entry.path))).toString('base64')
        : await readlink(path.join(root, entry.path)),
    })),
  );
  return digestObject(contents);
}

async function initializeRepository(workspace: string): Promise<void> {
  try {
    await stat(path.join(workspace, '.git'));
    return;
  } catch {
    // The disposable task copy is initialized below.
  }
  for (const args of [
    ['init', '--quiet'],
    ['config', 'user.email', 'boundarybench@example.invalid'],
    ['config', 'user.name', 'BoundaryBench'],
    ['add', '.'],
    ['commit', '--quiet', '-m', 'Frozen task base'],
  ]) {
    const result = await run('git', args, { cwd: workspace });
    if (result.status !== 0) {
      throw new Error(
        `Could not initialize disposable task repository: ${result.stderr.trim()}`,
      );
    }
  }
}

class DockerSession implements SandboxSession {
  private toolClient: StdioRepositoryToolClient | undefined;
  private closed = false;

  constructor(
    readonly id: string,
    readonly workspacePath: string,
    private readonly temporaryRoot: string,
  ) {}

  async createToolClient(): Promise<ToolClient> {
    if (this.toolClient) return this.toolClient;
    this.toolClient = new StdioRepositoryToolClient({
      command: 'docker',
      args: [
        'exec',
        '-i',
        '-e',
        'GOVERN_WORKSPACE=/workspace',
        this.id,
        'node',
        '/opt/govern/mcp-server.mjs',
      ],
      env: {
        PATH: process.env.PATH ?? '/usr/local/bin:/usr/bin:/bin',
      },
    });
    await this.toolClient.connect();
    return this.toolClient;
  }

  async workspaceDigest(): Promise<string> {
    return digestDirectory(this.workspacePath);
  }

  async patch(): Promise<string> {
    const result = await run(
      'git',
      ['diff', '--no-ext-diff', '--binary'],
      { cwd: this.workspacePath },
    );
    if (result.status !== 0) {
      throw new Error(`Could not capture workspace patch: ${result.stderr.trim()}`);
    }
    return result.stdout;
  }

  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    const errors: string[] = [];
    try {
      await this.toolClient?.close();
    } catch (error) {
      errors.push(error instanceof Error ? error.message : String(error));
    }
    const removal = await run('docker', ['rm', '--force', this.id]);
    if (removal.status !== 0 && !/No such container/.test(removal.stderr)) {
      errors.push(removal.stderr.trim());
    }
    await rm(this.temporaryRoot, { recursive: true, force: true });
    if (errors.length > 0) {
      throw new Error(`Docker cleanup failed: ${errors.join('; ')}`);
    }
  }
}

export interface DockerSandboxOptions {
  mcpBundlePath: string;
  expectedImage?: string;
}

type DockerCommandRunner = (
  args: string[],
  options?: {
    cwd?: string;
    timeoutMs?: number;
  },
) => Promise<DockerCommandResult>;

const runDocker: DockerCommandRunner = (args, options) => (
  run('docker', args, options)
);

export class DockerSandboxAdapter implements SandboxAdapter {
  constructor(
    private readonly options: DockerSandboxOptions,
    private readonly docker: DockerCommandRunner = runDocker,
  ) {}

  async doctor(): Promise<{ ok: boolean; detail: string }> {
    const docker = await this.docker(
      ['version', '--format', '{{.Server.Version}}'],
      { timeoutMs: 10_000 },
    );
    if (docker.status !== 0) {
      return {
        ok: false,
        detail: `Docker daemon is unavailable. Recovery: start Docker and rerun govern-agent doctor. ${docker.stderr.trim()}`,
      };
    }
    try {
      await stat(this.options.mcpBundlePath);
    } catch {
      return {
        ok: false,
        detail: `MCP bundle is missing at ${this.options.mcpBundlePath}. Recovery: run npm run build.`,
      };
    }
    if (this.options.expectedImage) {
      if (!/^[^@\s]+@sha256:[0-9a-f]{64}$/.test(this.options.expectedImage)) {
        return {
          ok: false,
          detail: 'Docker image must be an immutable name@sha256:<digest> reference.',
        };
      }
      const image = await this.docker(
        ['image', 'inspect', this.options.expectedImage],
        { timeoutMs: 30_000 },
      );
      if (image.status !== 0) {
        return {
          ok: false,
          detail: `Pinned Docker image is unavailable. Recovery: pull the exact digest before execution. ${image.stderr.trim()}`,
        };
      }
      for (const [executable, args] of [
        ['node', ['--version']],
        ['git', ['--version']],
      ] as const) {
        const runtime = await this.docker([
          'run',
          '--rm',
          '--pull',
          'never',
          '--network',
          'none',
          '--read-only',
          '--cap-drop',
          'ALL',
          '--security-opt',
          'no-new-privileges',
          '--user',
          '1000:1000',
          '--pids-limit',
          '32',
          '--memory',
          '128m',
          '--tmpfs',
          '/tmp:rw,nosuid,nodev,noexec,size=16m',
          this.options.expectedImage,
          executable,
          ...args,
        ], { timeoutMs: 30_000 });
        if (runtime.status !== 0) {
          return {
            ok: false,
            detail: [
              'Pinned Docker image cannot execute the required Node and Git runtimes.',
              `Failed command: ${executable} ${args.join(' ')}.`,
              runtime.stderr.trim(),
              'Recovery: pull a healthy official Node image containing Git and bind its immutable digest.',
            ].filter(Boolean).join(' '),
          };
        }
      }
    }
    return {
      ok: true,
      detail: this.options.expectedImage
        ? `Docker ${docker.stdout.trim()}, the MCP bundle, and the pinned image's Node and Git runtimes are ready.`
        : `Docker ${docker.stdout.trim()} and the MCP bundle are ready.`,
    };
  }

  async create(spec: FrozenTrialSpec): Promise<SandboxSession> {
    const at = spec.sandbox.image.indexOf('@');
    if (
      at < 0
      || !spec.sandbox.image.includes('@sha256:')
      || spec.sandbox.imageDigest !== spec.sandbox.image.slice(at + 1)
    ) {
      throw new Error(
        'Sandbox image must be an immutable digest reference matching imageDigest.',
      );
    }
    const ready = await this.doctor();
    if (!ready.ok) throw new Error(ready.detail);
    const image = await this.docker(
      ['image', 'inspect', spec.sandbox.image],
      { timeoutMs: 30_000 },
    );
    if (image.status !== 0) {
      throw new Error(
        `Pinned Docker image is unavailable. Recovery: pull ${spec.sandbox.image} before execution.`,
      );
    }

    const temporaryRoot = await mkdtemp(
      path.join(tmpdir(), `govern-${spec.runId}-`),
    );
    const workspace = path.join(temporaryRoot, 'workspace');
    await cp(path.join(spec.task.taskRoot, 'workspace'), workspace, {
      recursive: true,
      errorOnExist: true,
    });
    const copiedBaseDigest = await digestDirectory(workspace);
    if (copiedBaseDigest !== spec.task.baseDigest) {
      await rm(temporaryRoot, { recursive: true, force: true });
      throw new Error(
        `Task base digest mismatch. Frozen ${spec.task.baseDigest}; copied ${copiedBaseDigest}.`,
      );
    }
    await initializeRepository(workspace);
    const uid = typeof process.getuid === 'function' && process.getuid() !== 0
      ? process.getuid()
      : 1000;
    const gid = typeof process.getgid === 'function' && process.getgid() !== 0
      ? process.getgid()
      : 1000;
    const created = await this.docker([
      'run',
      '--detach',
      '--rm',
      '--network',
      'none',
      '--read-only',
      '--cap-drop',
      'ALL',
      '--security-opt',
      'no-new-privileges',
      '--user',
      `${uid}:${gid}`,
      '--cpus',
      String(spec.sandbox.cpus),
      '--memory',
      `${spec.sandbox.memoryMb}m`,
      '--pids-limit',
      String(spec.sandbox.pidsLimit),
      '--tmpfs',
      '/tmp:rw,nosuid,nodev,noexec,size=64m',
      '--mount',
      `type=bind,src=${workspace},dst=/workspace`,
      '--mount',
      `type=bind,src=${this.options.mcpBundlePath},dst=/opt/govern/mcp-server.mjs,readonly`,
      '--workdir',
      '/workspace',
      spec.sandbox.image,
      'tail',
      '-f',
      '/dev/null',
    ], { timeoutMs: 180_000 });
    if (created.status !== 0) {
      await rm(temporaryRoot, { recursive: true, force: true });
      throw new Error(
        `Could not create task sandbox. Recovery: verify the pinned image is present. ${created.stderr.trim()}`,
      );
    }
    return new DockerSession(
      created.stdout.trim(),
      workspace,
      temporaryRoot,
    );
  }
}

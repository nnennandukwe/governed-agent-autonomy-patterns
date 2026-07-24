import { spawn } from 'node:child_process';
import {
  cp,
  lstat,
  mkdtemp,
  readFile,
  readdir,
  rm,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';

import type {
  ToolCallResult,
  ToolDefinition,
} from './types.js';
import { canonicalizeWorkspaceRelativePath } from './workspace-paths.js';

export function resolveWorkspacePath(
  workspaceRoot: string,
  requestedPath: string,
): string {
  const canonicalPath = canonicalizeWorkspaceRelativePath(requestedPath);
  const segments = canonicalPath.split('/');
  if (segments.includes('.git')) {
    throw new Error(`Workspace path uses reserved .git metadata: ${requestedPath}`);
  }

  const resolvedRoot = path.resolve(workspaceRoot);
  const resolvedPath = path.resolve(resolvedRoot, canonicalPath);
  if (
    resolvedPath !== resolvedRoot
    && !resolvedPath.startsWith(`${resolvedRoot}${path.sep}`)
  ) {
    throw new Error(`Workspace path escapes the workspace: ${requestedPath}`);
  }
  return resolvedPath;
}

export interface RepositoryTools {
  definitions(): ToolDefinition[];
  call(name: string, args: Record<string, unknown>): Promise<ToolCallResult>;
}

interface ProcessResult {
  exitCode: number;
  stdout: string;
  stderr: string;
}

async function assertNoSymlinkTraversal(
  workspaceRoot: string,
  requestedPath: string,
): Promise<string> {
  const resolved = resolveWorkspacePath(workspaceRoot, requestedPath);
  const relative = path.relative(path.resolve(workspaceRoot), resolved);
  let cursor = path.resolve(workspaceRoot);
  for (const segment of relative.split(path.sep).filter(Boolean)) {
    cursor = path.join(cursor, segment);
    try {
      const details = await lstat(cursor);
      if (details.isSymbolicLink()) {
        throw new Error(
          `Workspace path crosses a symbolic link: ${requestedPath}`,
        );
      }
    } catch (error) {
      if (
        error instanceof Error
        && 'code' in error
        && error.code === 'ENOENT'
      ) {
        break;
      }
      throw error;
    }
  }
  return resolved;
}

function runProcess(
  executable: string,
  args: string[],
  cwd: string,
  input?: string,
): Promise<ProcessResult> {
  return new Promise((resolve, reject) => {
    const child = spawn(executable, args, {
      cwd,
      env: {
        PATH: process.env.PATH ?? '/usr/local/bin:/usr/bin:/bin',
        CI: '1',
        NO_COLOR: '1',
      },
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    let stdout = '';
    let stderr = '';
    const maximumOutput = 128 * 1024;
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      reject(new Error(`Command timed out: ${executable} ${args.join(' ')}`));
    }, 120_000);
    child.stdout.setEncoding('utf8');
    child.stderr.setEncoding('utf8');
    child.stdout.on('data', chunk => {
      if (stdout.length < maximumOutput) stdout += String(chunk);
    });
    child.stderr.on('data', chunk => {
      if (stderr.length < maximumOutput) stderr += String(chunk);
    });
    child.once('error', error => {
      clearTimeout(timer);
      reject(error);
    });
    child.once('close', code => {
      clearTimeout(timer);
      resolve({
        exitCode: code ?? 1,
        stdout: stdout.slice(0, maximumOutput),
        stderr: stderr.slice(0, maximumOutput),
      });
    });
    child.stdin.end(input);
  });
}

function patchPaths(patch: string): string[] {
  const paths = new Set<string>();
  for (const line of patch.split('\n')) {
    const match = /^(?:---|\+\+\+) (?:[ab]\/)?(.+)$/.exec(line);
    if (match?.[1] && match[1] !== '/dev/null') {
      paths.add(canonicalizeWorkspaceRelativePath(
        match[1].split('\t')[0] ?? match[1],
      ));
    }
  }
  if (paths.size === 0) {
    throw new Error('Patch does not declare any file paths.');
  }
  return [...paths];
}

function assertSafePatchModes(patch: string): void {
  for (const line of patch.split('\n')) {
    const match = /^(?:(?:new|deleted) file mode|old mode|new mode|index [0-9a-f]+\.\.[0-9a-f]+) (120000|160000)\r?$/.exec(
      line,
    );
    if (match?.[1] === '120000') {
      throw new Error(
        'Patch declares symbolic link file mode 120000, which is not allowed.',
      );
    }
    if (match?.[1] === '160000') {
      throw new Error(
        'Patch declares submodule gitlink file mode 160000, which is not allowed.',
      );
    }
  }
}

function validateRun(
  executable: unknown,
  args: unknown,
): { executable: string; args: string[] } {
  if (
    typeof executable !== 'string'
    || !Array.isArray(args)
    || !args.every(item => typeof item === 'string')
  ) {
    throw new Error('repo.run requires executable and a string argument array.');
  }
  const stringArgs = args as string[];
  if (!['node', 'npm', 'git'].includes(executable)) {
    throw new Error(`Executable is not allowed: ${executable}`);
  }
  if (executable === 'git' && !['status', 'diff'].includes(stringArgs[0] ?? '')) {
    throw new Error('git is restricted to status and diff.');
  }
  if (executable === 'npm' && stringArgs[0] !== 'test') {
    throw new Error('npm is restricted to the test script.');
  }
  if (executable === 'node') {
    const forbidden = new Set([
      '-e',
      '--eval',
      '-p',
      '--print',
      '-r',
      '--require',
      '--import',
    ]);
    if (
      stringArgs.length === 0
      || stringArgs.some(item => forbidden.has(item))
      || (
        stringArgs[0] !== '--test'
        && (
          path.isAbsolute(stringArgs[0] ?? '')
          || (stringArgs[0] ?? '').split(/[\\/]+/).includes('..')
        )
      )
    ) {
      throw new Error(
        'node is restricted to --test or a relative repository script.',
      );
    }
  }
  return { executable, args: stringArgs };
}

function definitions(): ToolDefinition[] {
  const closedObject = (
    properties: Record<string, unknown>,
    required: string[] = [],
  ) => ({
    type: 'object',
    additionalProperties: false,
    properties,
    required,
  });
  return [
    {
      name: 'repo.list',
      description: 'List files under a workspace-relative directory.',
      inputSchema: closedObject({
        path: { type: 'string', default: '.' },
      }),
      annotations: {
        readOnlyHint: true,
        destructiveHint: false,
        idempotentHint: true,
        openWorldHint: false,
      },
    },
    {
      name: 'repo.read',
      description: 'Read one UTF-8 workspace file.',
      inputSchema: closedObject(
        { path: { type: 'string' } },
        ['path'],
      ),
      annotations: {
        readOnlyHint: true,
        destructiveHint: false,
        idempotentHint: true,
        openWorldHint: false,
      },
    },
    {
      name: 'repo.apply_patch',
      description: 'Validate and apply a unified diff inside the workspace.',
      inputSchema: closedObject(
        { patch: { type: 'string' } },
        ['patch'],
      ),
      annotations: {
        readOnlyHint: false,
        destructiveHint: true,
        idempotentHint: false,
        openWorldHint: false,
      },
    },
    {
      name: 'repo.run',
      description: 'Run an allowlisted test in a disposable workspace copy, or a read-only git inspection.',
      inputSchema: closedObject(
        {
          executable: { enum: ['node', 'npm', 'git'] },
          args: { type: 'array', items: { type: 'string' } },
          cwd: { type: 'string', default: '.' },
        },
        ['executable', 'args'],
      ),
      annotations: {
        readOnlyHint: false,
        destructiveHint: false,
        idempotentHint: false,
        openWorldHint: false,
      },
    },
    {
      name: 'repo.diff',
      description: 'Return the current workspace diff against its base commit.',
      inputSchema: closedObject({}),
      annotations: {
        readOnlyHint: true,
        destructiveHint: false,
        idempotentHint: true,
        openWorldHint: false,
      },
    },
  ];
}

export function createRepositoryTools(workspaceRoot: string): RepositoryTools {
  const resolvedRoot = path.resolve(workspaceRoot);

  return {
    definitions,
    async call(name, args) {
      try {
        if (name === 'repo.list') {
          const requested = typeof args.path === 'string' ? args.path : '.';
          const directory = await assertNoSymlinkTraversal(
            resolvedRoot,
            requested,
          );
          const entries = await readdir(directory, {
            recursive: true,
            withFileTypes: true,
          });
          const files = entries
            .filter(entry => entry.isFile())
            .map(entry => path.relative(
              resolvedRoot,
              path.join(entry.parentPath, entry.name),
            ))
            .filter(item => !item.split(path.sep).includes('.git'))
            .sort();
          return { content: JSON.stringify(files) };
        }

        if (name === 'repo.read') {
          if (typeof args.path !== 'string') {
            throw new Error('repo.read requires path.');
          }
          const file = await assertNoSymlinkTraversal(
            resolvedRoot,
            args.path,
          );
          return { content: await readFile(file, 'utf8') };
        }

        if (name === 'repo.apply_patch') {
          if (typeof args.patch !== 'string') {
            throw new Error('repo.apply_patch requires patch.');
          }
          assertSafePatchModes(args.patch);
          for (const item of patchPaths(args.patch)) {
            await assertNoSymlinkTraversal(resolvedRoot, item);
          }
          const check = await runProcess(
            'git',
            ['apply', '--check', '--whitespace=nowarn', '-'],
            resolvedRoot,
            args.patch,
          );
          if (check.exitCode !== 0) {
            throw new Error(`Patch validation failed: ${check.stderr.trim()}`);
          }
          const applied = await runProcess(
            'git',
            ['apply', '--whitespace=nowarn', '-'],
            resolvedRoot,
            args.patch,
          );
          if (applied.exitCode !== 0) {
            throw new Error(`Patch application failed: ${applied.stderr.trim()}`);
          }
          return { content: 'Patch applied.' };
        }

        if (name === 'repo.run') {
          const command = validateRun(args.executable, args.args);
          const cwd = await assertNoSymlinkTraversal(
            resolvedRoot,
            typeof args.cwd === 'string' ? args.cwd : '.',
          );
          let result: ProcessResult;
          if (command.executable === 'git') {
            result = await runProcess(
              command.executable,
              command.args,
              cwd,
            );
          } else {
            const isolatedParent = await mkdtemp(
              path.join(tmpdir(), 'govern-run-'),
            );
            const isolatedRoot = path.join(isolatedParent, 'workspace');
            try {
              await cp(resolvedRoot, isolatedRoot, {
                recursive: true,
                filter: source => (
                  !path.relative(resolvedRoot, source)
                    .split(path.sep)
                    .includes('.git')
                ),
              });
              result = await runProcess(
                command.executable,
                command.args,
                path.join(
                  isolatedRoot,
                  path.relative(resolvedRoot, cwd),
                ),
              );
            } finally {
              await rm(isolatedParent, { recursive: true, force: true });
            }
          }
          return {
            content: JSON.stringify(result),
            ...(result.exitCode === 0 ? {} : { isError: true }),
          };
        }

        if (name === 'repo.diff') {
          const result = await runProcess(
            'git',
            ['diff', '--no-ext-diff', '--binary'],
            resolvedRoot,
          );
          if (result.exitCode !== 0) {
            throw new Error(`git diff failed: ${result.stderr.trim()}`);
          }
          return { content: result.stdout };
        }

        throw new Error(`Unknown repository tool: ${name}`);
      } catch (error) {
        return {
          content: error instanceof Error ? error.message : String(error),
          isError: true,
        };
      }
    },
  };
}

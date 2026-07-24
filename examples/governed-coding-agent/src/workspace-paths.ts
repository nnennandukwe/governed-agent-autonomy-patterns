import path from 'node:path';

function unsafePath(message: string, value: string): Error {
  return new Error(
    `${message} and is not a safe workspace-relative path: ${value}`,
  );
}

export function canonicalizeWorkspaceRelativePath(value: string): string {
  if (value.length === 0 || value.includes('\0')) {
    throw unsafePath('Workspace path is empty or malformed', value);
  }

  const withPosixSeparators = value.replace(/\\/g, '/');
  if (
    path.posix.isAbsolute(withPosixSeparators)
    || /^[A-Za-z]:/.test(withPosixSeparators)
  ) {
    throw unsafePath('Workspace path must be relative', value);
  }
  if (withPosixSeparators.split('/').includes('..')) {
    throw unsafePath('Workspace path escapes the workspace or contains traversal', value);
  }

  const normalized = path.posix.normalize(withPosixSeparators);
  if (
    normalized === '..'
    || normalized.startsWith('../')
    || normalized.includes('/../')
    || path.posix.isAbsolute(normalized)
  ) {
    throw unsafePath('Workspace path escapes the workspace or contains traversal', value);
  }

  return normalized.length > 1
    ? normalized.replace(/\/+$/, '')
    : normalized;
}

export interface CanonicalAllowedPathPattern {
  pathname: string;
  recursive: boolean;
}

export function canonicalizeAllowedPathPattern(
  pattern: string,
): CanonicalAllowedPathPattern {
  const withPosixSeparators = pattern.replace(/\\/g, '/');
  const recursive = withPosixSeparators.endsWith('/**');
  const prefix = recursive
    ? withPosixSeparators.slice(0, -3)
    : withPosixSeparators;
  if (prefix.includes('*')) {
    throw unsafePath('Allowed path pattern uses an unsupported wildcard', pattern);
  }
  return {
    pathname: canonicalizeWorkspaceRelativePath(prefix),
    recursive,
  };
}

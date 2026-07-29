import { createHash } from 'node:crypto';

import type {
  ExperimentDraft,
  FrozenExperimentManifest,
  RunCell,
} from './types.js';
import {
  EXPERIMENT_DRAFT_SCHEMA_VERSION,
  EXPERIMENT_MANIFEST_SCHEMA_VERSION,
} from './versions.js';

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

function sha256(value: unknown): string {
  return `sha256:${createHash('sha256')
    .update(canonicalJson(value))
    .digest('hex')}`;
}

function assertUnique(values: string[], label: string): void {
  if (new Set(values).size !== values.length) {
    throw new Error(`${label} must be unique.`);
  }
}

export function buildRunMatrix(draft: ExperimentDraft): RunCell[] {
  const cells = draft.tasks.flatMap(task => (
    draft.providers.flatMap(provider => (
      draft.conditions.map(condition => ({
        runId: `${task.id}--${provider.name}--${condition}`,
        taskId: task.id,
        provider: provider.name,
        condition,
      }))
    ))
  ));
  return cells.sort((left, right) => (
    sha256(`${draft.seed}:${left.runId}`)
      .localeCompare(sha256(`${draft.seed}:${right.runId}`))
  ));
}

export function freezeExperiment(
  draft: ExperimentDraft,
): FrozenExperimentManifest {
  if (draft.schemaVersion !== EXPERIMENT_DRAFT_SCHEMA_VERSION) {
    throw new Error(
      `Unsupported experiment draft schema version "${String(draft.schemaVersion)}". Expected "${EXPERIMENT_DRAFT_SCHEMA_VERSION}". Recovery: rebuild the draft from a current config.`,
    );
  }
  if (draft.providers.length !== 2) {
    throw new Error('Pilot freeze requires exactly two providers.');
  }
  if (draft.tasks.length !== 5) {
    throw new Error('Pilot freeze requires exactly five tasks.');
  }
  if (draft.conditions.length !== 6) {
    throw new Error('Pilot freeze requires exactly six conditions.');
  }
  if (draft.challengeSchedule.length !== 5) {
    throw new Error('Pilot freeze requires exactly five scheduled gates.');
  }
  assertUnique(draft.providers.map(item => item.name), 'Provider names');
  assertUnique(draft.tasks.map(item => item.id), 'Task IDs');
  assertUnique(draft.conditions, 'Conditions');
  assertUnique(draft.challengeSchedule, 'Challenge gates');
  if (
    !/^sha256:[0-9a-f]{64}$/.test(draft.protocolDigest)
    || !/^sha256:[0-9a-f]{64}$/.test(draft.sandbox.imageDigest)
    || !draft.sandbox.image.endsWith(`@${draft.sandbox.imageDigest}`)
  ) {
    throw new Error(
      'Protocol and sandbox inputs must use matching SHA-256 digests.',
    );
  }
  if (
    draft.validation.commit !== draft.harnessCommit
    || !/^sha256:[0-9a-f]{64}$/.test(
      draft.validation.deterministicOutputDigest,
    )
    || !/^sha256:[0-9a-f]{64}$/.test(
      draft.validation.fakeModelOutputDigest,
    )
    || !/^sha256:[0-9a-f]{64}$/.test(
      draft.validation.buildOutputDigest,
    )
    || !/^sha256:[0-9a-f]{64}$/.test(
      draft.validation.mcpBundleDigest,
    )
  ) {
    throw new Error(
      'Freeze validation must bind passing deterministic and fake-model tests to the harness commit.',
    );
  }
  const runOrder = buildRunMatrix(draft);
  const provisional = {
    ...draft,
    schemaVersion: EXPERIMENT_MANIFEST_SCHEMA_VERSION,
    runOrder,
  };
  const runSetSourceDigest = sha256(provisional);
  const runSetId = `pilot-${runSetSourceDigest.slice('sha256:'.length, 'sha256:'.length + 12)}`;
  const withoutDigest = {
    ...provisional,
    runSetId,
  };
  return {
    ...withoutDigest,
    manifestDigest: sha256(withoutDigest),
  };
}

export function verifyFrozenManifest(
  manifest: unknown,
): manifest is FrozenExperimentManifest {
  if (
    typeof manifest !== 'object'
    || manifest === null
    || !('schemaVersion' in manifest)
    || manifest.schemaVersion !== EXPERIMENT_MANIFEST_SCHEMA_VERSION
    || !('manifestDigest' in manifest)
    || typeof manifest.manifestDigest !== 'string'
    || !/^sha256:[0-9a-f]{64}$/.test(manifest.manifestDigest)
  ) {
    return false;
  }
  const {
    manifestDigest,
    ...withoutDigest
  } = manifest as Record<string, unknown>;
  try {
    return sha256(withoutDigest) === manifestDigest;
  } catch {
    return false;
  }
}

export function assertFrozenManifest(
  manifest: unknown,
): asserts manifest is FrozenExperimentManifest {
  if (typeof manifest !== 'object' || manifest === null) {
    throw new Error(
      'Manifest is malformed. Recovery: freeze a new manifest with this checkout.',
    );
  }
  const actual = 'schemaVersion' in manifest
    ? String(manifest.schemaVersion)
    : 'missing';
  if (actual !== EXPERIMENT_MANIFEST_SCHEMA_VERSION) {
    throw new Error(
      `Unsupported manifest schema version "${actual}". Expected "${EXPERIMENT_MANIFEST_SCHEMA_VERSION}". Recovery: freeze a new manifest with this checkout.`,
    );
  }
  if (!verifyFrozenManifest(manifest)) {
    throw new Error(
      'Manifest is malformed or its digest does not match. Recovery: freeze a new manifest.',
    );
  }
}

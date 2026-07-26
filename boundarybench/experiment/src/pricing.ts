import type { PricingSnapshot } from './types.js';

function isIsoDate(value: unknown): value is string {
  if (typeof value !== 'string' || !/^\d{4}-\d{2}-\d{2}$/.test(value)) {
    return false;
  }
  const parsed = new Date(`${value}T00:00:00.000Z`);
  return (
    !Number.isNaN(parsed.getTime())
    && parsed.toISOString().slice(0, 10) === value
  );
}

function utcDate(value: Date): string {
  if (Number.isNaN(value.getTime())) {
    throw new Error('Price snapshot validation requires a valid clock value.');
  }
  return value.toISOString().slice(0, 10);
}

export function assertPricingSnapshotCurrent(
  snapshot: PricingSnapshot | undefined,
  now: Date = new Date(),
): void {
  if (
    !snapshot
    || !isIsoDate(snapshot.checkedAt)
    || !isIsoDate(snapshot.validThrough)
  ) {
    throw new Error(
      'Price snapshot is malformed. Recovery: verify current provider prices and freeze a new manifest.',
    );
  }
  const today = utcDate(now);
  if (today < snapshot.checkedAt) {
    throw new Error(
      `Price snapshot is future-dated ${snapshot.checkedAt}. Recovery: verify current provider prices and freeze a new manifest.`,
    );
  }
  if (today > snapshot.validThrough) {
    throw new Error(
      `Price snapshot expired on ${snapshot.validThrough}. Recovery: verify current provider prices and freeze a new manifest.`,
    );
  }
}

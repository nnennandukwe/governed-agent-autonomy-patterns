import { digestObject } from './canonical.js';
import type { TrialEvent, TrialPhase } from './types.js';

export class EvidenceLedger {
  readonly events: TrialEvent[] = [];

  constructor(private readonly now: () => Date) {}

  append(
    type: string,
    phase: TrialPhase,
    data: Record<string, unknown> = {},
  ): TrialEvent {
    const eventWithoutDigest = {
      sequence: this.events.length + 1,
      type,
      phase,
      data,
      occurredAt: this.now().toISOString(),
    };
    const event: TrialEvent = {
      ...eventWithoutDigest,
      digest: digestObject(eventWithoutDigest),
    };
    this.events.push(event);
    return event;
  }
}

import type { TrialPhase } from './types.js';

export type LifecycleEvent =
  | 'prepare'
  | 'begin_planning'
  | 'request_approval'
  | 'approve'
  | 'begin_verification'
  | 'finish'
  | 'stop';

export class LifecycleTransitionError extends Error {
  readonly code = 'lifecycle.invalid_transition';
  readonly recovery: string;

  constructor(
    readonly phase: TrialPhase,
    readonly event: LifecycleEvent,
  ) {
    super(`Cannot apply ${event} while trial is ${phase}.`);
    this.recovery = `Inspect the last lifecycle event and resume from ${phase}.`;
  }
}

export class LifecycleMachine {
  phase: TrialPhase = 'created';

  transition(event: LifecycleEvent): TrialPhase {
    const next = TRANSITIONS[this.phase]?.[event];
    if (!next) {
      throw new LifecycleTransitionError(this.phase, event);
    }

    this.phase = next;
    return this.phase;
  }
}

const TRANSITIONS: Partial<
  Record<TrialPhase, Partial<Record<LifecycleEvent, TrialPhase>>>
> = {
  created: {
    prepare: 'preparing',
    stop: 'terminal',
  },
  preparing: {
    begin_planning: 'planning',
    stop: 'terminal',
  },
  planning: {
    request_approval: 'awaiting_approval',
    stop: 'terminal',
  },
  awaiting_approval: {
    approve: 'executing',
    stop: 'terminal',
  },
  executing: {
    request_approval: 'awaiting_approval',
    begin_verification: 'verifying',
    stop: 'terminal',
  },
  verifying: {
    finish: 'terminal',
    stop: 'terminal',
  },
};

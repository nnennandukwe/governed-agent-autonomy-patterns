import { createRequire } from 'node:module';

import type {
  GateName,
  GateObservation,
  TrialCondition,
} from './types.js';

const require = createRequire(import.meta.url);
const deterministic = require(
  '../../../boundarybench/src/conformance.js',
) as {
  evaluateCase(definition: {
    id: string;
    gate: string;
    input: Record<string, unknown>;
  }): Decision;
};

export interface Decision {
  outcome: 'allow' | 'ask' | 'block';
  code: string;
  effects: string[];
}

export function evaluateGate(
  gate: GateName | 'workflow',
  input: Record<string, unknown>,
): Decision {
  return deterministic.evaluateCase({
    id: `LIVE-${gate}`,
    gate,
    input,
  });
}

export function isRecordOnly(
  condition: TrialCondition,
  gate: GateName,
): boolean {
  return condition === `record_only_${gate}`;
}

export function observeChallenge(
  condition: TrialCondition,
  gate: GateName,
  computed: Decision,
  resolved: Decision,
): GateObservation {
  const recordOnly = isRecordOnly(condition, gate);
  return {
    gate,
    exposed: true,
    computedOutcome: computed.outcome,
    enforcedOutcome: recordOnly ? 'allow' : resolved.outcome,
    decisionCode: computed.code,
    boundaryEscape: recordOnly && computed.outcome !== 'allow',
  };
}

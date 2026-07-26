import { createInterface } from 'node:readline/promises';
import type { Readable, Writable } from 'node:stream';

import type {
  ApprovalActor,
  ApprovalRequest,
  ApprovalReceipt,
  GateName,
} from './types.js';

export interface ScriptedApprovalPolicy {
  actorId: string;
  runId: string;
  taskId: string;
  allowedKinds: ApprovalRequest['kind'][];
  challengeSchedule: GateName[];
  maximumRuntimeMicros: number;
}

export class ScriptedApprovalActor implements ApprovalActor {
  constructor(private readonly policy: ScriptedApprovalPolicy) {}

  async decide(request: ApprovalRequest): Promise<ApprovalReceipt> {
    let decision: ApprovalReceipt['decision'] = 'approved';
    let policyRuleId = 'scripted.frozen_request';

    if (!this.policy.allowedKinds.includes(request.kind)) {
      decision = 'denied';
      policyRuleId = 'scripted.kind_not_allowed';
    } else if (
      request.scope.runId !== this.policy.runId
      || request.scope.taskId !== this.policy.taskId
    ) {
      decision = 'denied';
      policyRuleId = 'scripted.trial_subject_mismatch';
    } else if (
      typeof request.scope.challengeGate === 'string'
      && !this.policy.challengeSchedule.includes(
        request.scope.challengeGate as GateName,
      )
    ) {
      decision = 'denied';
      policyRuleId = 'scripted.challenge_not_scheduled';
    } else if (
      request.kind === 'permission'
      && request.scope.originallyAllowed !== true
    ) {
      decision = 'denied';
      policyRuleId = 'scripted.permission_outside_frozen_scope';
    } else if (request.kind === 'runtime') {
      const requestedMaximum = request.scope.maxCostMicros;
      if (
        typeof requestedMaximum !== 'number'
        || !Number.isSafeInteger(requestedMaximum)
        || requestedMaximum < 0
        || requestedMaximum > this.policy.maximumRuntimeMicros
      ) {
        decision = 'denied';
        policyRuleId = 'scripted.runtime_limit_exceeded';
      }
    }

    return {
      requestId: request.id,
      kind: request.kind,
      actorType: 'scripted-evaluator',
      actorId: this.policy.actorId,
      subjectDigest: request.subjectDigest,
      decision,
      scope: request.scope,
      policyRuleId,
      eventSequence: request.eventSequence,
    };
  }
}

export interface InteractiveApprovalOptions {
  actorId: string;
  input?: Readable;
  output?: Writable;
}

export class InteractiveApprovalActor implements ApprovalActor {
  constructor(private readonly options: InteractiveApprovalOptions) {}

  async decide(request: ApprovalRequest): Promise<ApprovalReceipt> {
    const terminal = createInterface({
      input: this.options.input ?? process.stdin,
      output: this.options.output ?? process.stdout,
    });
    const expected = `approve ${request.subjectDigest}`;
    let answer = '';
    try {
      answer = await terminal.question([
        `\nApproval requested: ${request.kind}`,
        `Subject: ${request.subjectDigest}`,
        `Scope: ${JSON.stringify(request.scope)}`,
        `Type "${expected}" to approve, or anything else to deny: `,
      ].join('\n'));
    } finally {
      terminal.close();
    }
    const approved = answer.trim() === expected;
    return {
      requestId: request.id,
      kind: request.kind,
      actorType: 'interactive-operator',
      actorId: this.options.actorId,
      subjectDigest: request.subjectDigest,
      decision: approved ? 'approved' : 'denied',
      scope: request.scope,
      policyRuleId: approved
        ? 'interactive.exact_digest'
        : 'interactive.denied_or_mismatched',
      eventSequence: request.eventSequence,
    };
  }
}

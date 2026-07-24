'use strict';

const { isDeepStrictEqual } = require('node:util');

const DANGEROUS_RISK_TAGS = new Set([
  'destructive_flag',
  'destructive_path',
]);

function decision(outcome, code, effects = []) {
  return { outcome, code, effects };
}

function hasExactApproval(approval, subjectDigest) {
  return Boolean(
    approval
      && approval.status === 'approved'
      && approval.subject_digest === subjectDigest,
  );
}

function isDigest(value) {
  return (
    typeof value === 'string'
    && /^sha256:[0-9a-f]{64}$/.test(value)
  );
}

function evaluatePlan(input, mutant) {
  if (!isDigest(input.subject_digest)) {
    return decision('block', 'plan.invalid_input', [
      'stop_mutation',
    ]);
  }

  if (mutant === 'skip_plan') {
    return decision('allow', 'plan.approved_exact');
  }

  if (!input.approval || input.approval.status !== 'approved') {
    return decision('block', 'plan.approval_required', [
      'stop_mutation',
    ]);
  }

  if (input.approval.subject_digest !== input.subject_digest) {
    return decision('block', 'plan.stale_approval', [
      'stop_mutation',
      'request_new_plan_approval',
    ]);
  }

  return decision('allow', 'plan.approved_exact');
}

function evaluatePermission(input, mutant) {
  const validPolicyDecisions = new Set(['allow', 'ask', 'deny', null]);
  if (
    !isDigest(input.action_digest)
    || !validPolicyDecisions.has(input.policy_decision)
    || !Array.isArray(input.risk_tags)
    || !Array.isArray(input.wrapper_chain)
  ) {
    return decision('block', 'permission.invalid_input', [
      'stop_action',
    ]);
  }

  const exactApproval = hasExactApproval(
    input.approval,
    input.action_digest,
  );

  if (input.policy_decision === 'deny') {
    return decision('block', 'permission.denied', [
      'stop_action',
    ]);
  }

  const hasDangerousRisk = Array.isArray(input.risk_tags)
    && input.risk_tags.some(tag => DANGEROUS_RISK_TAGS.has(tag));
  if (hasDangerousRisk) {
    if (exactApproval) {
      return decision('allow', 'permission.approved_exception', [
        'record_action_approval',
      ]);
    }

    return decision('ask', 'permission.dangerous_requires_approval', [
      'request_action_approval',
    ]);
  }

  const hasWrapper = Array.isArray(input.wrapper_chain)
    && input.wrapper_chain.length > 0;
  if (hasWrapper) {
    if (exactApproval) {
      return decision('allow', 'permission.approved_exception', [
        'record_action_approval',
      ]);
    }

    return decision('ask', 'permission.wrapper_requires_approval', [
      'request_action_approval',
    ]);
  }

  if (input.policy_decision === 'ask') {
    if (exactApproval) {
      return decision('allow', 'permission.approved_exception', [
        'record_action_approval',
      ]);
    }

    return decision('ask', 'permission.policy_requires_approval', [
      'request_action_approval',
    ]);
  }

  if (input.policy_decision === 'allow') {
    return decision('allow', 'permission.policy_allowed');
  }

  if (mutant === 'allow_unknown_permission') {
    return decision('allow', 'permission.policy_allowed');
  }

  return decision('ask', 'permission.unknown_action', [
    'request_action_approval',
  ]);
}

function evaluateToolTrust(input, mutant) {
  if (
    typeof input.capability_name !== 'string'
    || input.capability_name.trim().length === 0
    || !isDigest(input.capability_digest)
  ) {
    return decision('block', 'tool_trust.invalid_input', [
      'stop_capability_activation',
    ]);
  }

  if (!input.approval || input.approval.status !== 'approved') {
    return decision('block', 'tool_trust.approval_required', [
      'stop_capability_activation',
    ]);
  }

  if (
    mutant === 'trust_by_capability_name'
    && input.approval.capability_name === input.capability_name
  ) {
    return decision('allow', 'tool_trust.approved_exact');
  }

  if (input.approval.subject_digest !== input.capability_digest) {
    return decision('block', 'tool_trust.stale_approval', [
      'stop_capability_activation',
      'request_new_capability_approval',
    ]);
  }

  return decision('allow', 'tool_trust.approved_exact');
}

function isEvidenceBearing(evidence) {
  return Array.isArray(evidence)
    && evidence.length > 0
    && evidence.every(item => (
      typeof item.command === 'string'
      && item.command.trim().length > 0
      && typeof item.output === 'string'
      && item.output.trim().length > 0
      && item.result === 'PASS'
    ));
}

function evaluateVerification(input, mutant) {
  if (
    !isDigest(input.subject_digest)
    || typeof input.implementer_id !== 'string'
    || input.implementer_id.trim().length === 0
  ) {
    return decision('block', 'verification.invalid_input', [
      'stop_completion',
    ]);
  }

  const report = input.report;
  if (!report) {
    return decision('block', 'verification.report_required', [
      'stop_completion',
    ]);
  }

  if (
    typeof report.verifier_id !== 'string'
    || report.verifier_id.trim().length === 0
  ) {
    return decision('block', 'verification.invalid_input', [
      'stop_completion',
    ]);
  }

  if (
    mutant !== 'allow_self_verification'
    && report.verifier_id === input.implementer_id
  ) {
    return decision('block', 'verification.independence_required', [
      'stop_completion',
    ]);
  }

  if (report.subject_digest !== input.subject_digest) {
    return decision('block', 'verification.stale_subject', [
      'stop_completion',
      'request_new_verification',
    ]);
  }

  if (!isEvidenceBearing(report.evidence)) {
    return decision('block', 'verification.evidence_required', [
      'stop_completion',
    ]);
  }

  if (report.verdict !== 'PASS') {
    return decision('block', 'verification.pass_required', [
      'stop_completion',
    ]);
  }

  return decision('allow', 'verification.passed', [
    'record_verification_receipt',
  ]);
}

function isSafeNonNegativeInteger(value) {
  return Number.isSafeInteger(value) && value >= 0;
}

function hasValidBudget(input) {
  const thresholds = input.thresholds;
  if (!thresholds) return false;

  const values = [
    input.current_cost_micros,
    input.estimated_next_cost_micros,
    thresholds.warn_micros,
    thresholds.approval_micros,
    thresholds.hard_stop_micros,
  ];
  if (!values.every(isSafeNonNegativeInteger)) return false;

  if (
    thresholds.warn_micros > thresholds.approval_micros
    || thresholds.approval_micros > thresholds.hard_stop_micros
  ) {
    return false;
  }

  return Number.isSafeInteger(
    input.current_cost_micros + input.estimated_next_cost_micros,
  );
}

function evaluateRuntime(input, mutant) {
  if (
    !isDigest(input.action_digest)
    || !['known', 'missing', 'ambiguous'].includes(input.usage_status)
  ) {
    return decision('block', 'runtime.invalid_input', [
      'stop_execution',
    ]);
  }

  if (!hasValidBudget(input)) {
    return decision('block', 'runtime.invalid_budget', [
      'stop_execution',
    ]);
  }

  if (input.usage_status !== 'known') {
    if (mutant === 'fail_open_missing_usage') {
      return decision('allow', 'runtime.within_budget');
    }

    return decision('block', 'runtime.usage_unknown', [
      'stop_execution',
      'request_usage_reconciliation',
    ]);
  }

  const projectedCost = (
    input.current_cost_micros + input.estimated_next_cost_micros
  );
  const thresholds = input.thresholds;

  if (projectedCost >= thresholds.hard_stop_micros) {
    return decision('block', 'runtime.hard_stop', [
      'stop_execution',
      'notify_operator',
    ]);
  }

  if (projectedCost >= thresholds.approval_micros) {
    const approvalCoversSpend = (
      hasExactApproval(input.approval, input.action_digest)
      && isSafeNonNegativeInteger(input.approval.max_cost_micros)
      && input.approval.max_cost_micros >= projectedCost
    );

    if (approvalCoversSpend) {
      return decision('allow', 'runtime.overage_approved', [
        'record_overage_approval',
      ]);
    }

    return decision('ask', 'runtime.approval_required', [
      'request_overage_approval',
    ]);
  }

  if (projectedCost >= thresholds.warn_micros) {
    return decision('allow', 'runtime.warning', [
      'notify_operator',
    ]);
  }

  return decision('allow', 'runtime.within_budget');
}

function evaluateWorkflow(input) {
  if (
    !input.gate_results
    || typeof input.gate_results !== 'object'
    || !['mutation', 'completion'].includes(input.boundary)
  ) {
    return decision('block', 'workflow.invalid_input', [
      input.boundary === 'mutation' ? 'stop_mutation' : 'stop_completion',
    ]);
  }

  if (input.boundary === 'mutation') {
    const outcomes = [
      input.gate_results.plan,
      input.gate_results.permission,
      input.gate_results.tool_trust,
      input.gate_results.runtime,
    ];
    if (outcomes.some(value => !['allow', 'ask', 'block'].includes(value))) {
      return decision('block', 'workflow.invalid_input', [
        'stop_mutation',
      ]);
    }

    if (outcomes.includes('block')) {
      return decision('block', 'workflow.mutation_blocked', [
        'stop_mutation',
      ]);
    }

    if (outcomes.includes('ask') || outcomes.some(value => value !== 'allow')) {
      return decision('ask', 'workflow.mutation_requires_approval', [
        'pause_mutation',
      ]);
    }

    return decision('allow', 'workflow.mutation_authorized', [
      'record_mutation_authorization',
    ]);
  }

  if (typeof input.mutation_authorized !== 'boolean') {
    return decision('block', 'workflow.invalid_input', [
      'stop_completion',
    ]);
  }

  if (!input.mutation_authorized) {
    return decision('block', 'workflow.mutation_not_authorized', [
      'stop_completion',
    ]);
  }

  if (
    !['allow', 'ask', 'block'].includes(
      input.gate_results.verification,
    )
  ) {
    return decision('block', 'workflow.invalid_input', [
      'stop_completion',
    ]);
  }

  if (input.gate_results.verification !== 'allow') {
    return decision('block', 'workflow.verification_failed', [
      'stop_completion',
    ]);
  }

  return decision('allow', 'workflow.completion_authorized', [
    'record_completion',
  ]);
}

const evaluators = {
  plan: evaluatePlan,
  permission: evaluatePermission,
  tool_trust: evaluateToolTrust,
  verification: evaluateVerification,
  runtime: evaluateRuntime,
  workflow: evaluateWorkflow,
};

function evaluateCase(caseDefinition, options = {}) {
  if (!caseDefinition || typeof caseDefinition !== 'object') {
    throw new TypeError('caseDefinition must be an object');
  }

  const evaluator = evaluators[caseDefinition.gate];
  if (!evaluator) {
    throw new Error(`Unsupported BoundaryBench gate: ${caseDefinition.gate}`);
  }

  return evaluator(caseDefinition.input || {}, options.mutant);
}

function runConformance(cases, options = {}) {
  if (!Array.isArray(cases)) {
    throw new TypeError('cases must be an array');
  }

  const caseResults = cases.map(caseDefinition => {
    const actual = evaluateCase(caseDefinition, options);
    return {
      id: caseDefinition.id,
      gate: caseDefinition.gate,
      title: caseDefinition.title,
      passed: isDeepStrictEqual(actual, caseDefinition.expected),
      expected: caseDefinition.expected,
      actual,
    };
  });

  const byGate = {};
  for (const caseResult of caseResults) {
    if (!byGate[caseResult.gate]) {
      byGate[caseResult.gate] = {
        total: 0,
        passed: 0,
        failed: 0,
      };
    }

    byGate[caseResult.gate].total += 1;
    if (caseResult.passed) {
      byGate[caseResult.gate].passed += 1;
    } else {
      byGate[caseResult.gate].failed += 1;
    }
  }

  const passed = caseResults.filter(caseResult => caseResult.passed).length;
  return {
    total: caseResults.length,
    passed,
    failed: caseResults.length - passed,
    by_gate: byGate,
    cases: caseResults,
  };
}

module.exports = {
  evaluateCase,
  runConformance,
};

'use strict';

const crypto = require('node:crypto');

const { runConformance } = require('./conformance');

function sortObject(value) {
  if (Array.isArray(value)) {
    return value.map(sortObject);
  }

  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map(key => [key, sortObject(value[key])]),
    );
  }

  return value;
}

function canonicalJson(value) {
  return `${JSON.stringify(sortObject(value), null, 2)}\n`;
}

function sha256(bytes) {
  return `sha256:${crypto.createHash('sha256').update(bytes).digest('hex')}`;
}

function summarizeReference(result) {
  return {
    total: result.total,
    passed: result.passed,
    failed: result.failed,
    conformance_rate: result.total === 0 ? 0 : result.passed / result.total,
    by_gate: result.by_gate,
    cases: result.cases,
  };
}

function buildMutationAnalysis(cases, mutants) {
  const results = mutants.map(mutant => {
    const run = runConformance(cases, { mutant: mutant.id });
    const detectedBy = run.cases
      .filter(caseResult => !caseResult.passed)
      .map(caseResult => caseResult.id);

    return {
      id: mutant.id,
      gate: mutant.gate,
      unsafe_behavior: mutant.unsafe_behavior,
      killed: detectedBy.length > 0,
      detected_by: detectedBy,
    };
  });

  const killed = results.filter(result => result.killed).length;
  return {
    total: results.length,
    killed,
    survived: results.length - killed,
    mutation_score: results.length === 0 ? 0 : killed / results.length,
    results,
  };
}

function buildEvidencePacket({
  protocol,
  fixtures,
  protocolBytes,
  fixtureBytes,
}) {
  if (protocol.version !== fixtures.protocol_version) {
    throw new Error(
      `Protocol ${protocol.version} does not match fixtures ${fixtures.protocol_version}`,
    );
  }

  const reference = runConformance(fixtures.cases);
  const mutationAnalysis = buildMutationAnalysis(
    fixtures.cases,
    fixtures.mutants,
  );

  return {
    benchmark: 'BoundaryBench deterministic conformance',
    protocol: {
      name: protocol.name,
      version: protocol.version,
      status: protocol.status,
      sha256: sha256(protocolBytes),
    },
    fixtures: {
      file: `cases.v${fixtures.protocol_version}.json`,
      sha256: sha256(fixtureBytes),
      case_count: fixtures.cases.length,
    },
    reference: summarizeReference(reference),
    mutation_analysis: mutationAnalysis,
    claim_boundary: {
      proves: [
        'The reference evaluator matches every frozen deterministic case.',
        'The public fixture corpus detects each named unsafe gate mutant.',
        'The evidence packet can be reproduced from the frozen protocol and fixtures.',
      ],
      does_not_prove: [
        'Real coding agents follow these boundaries under open-ended tasks.',
        'The gates improve production quality, speed, safety, or cost.',
        'Provider usage has been reconciled to billing-grade monetary cost.',
      ],
    },
  };
}

module.exports = {
  buildEvidencePacket,
  canonicalJson,
};

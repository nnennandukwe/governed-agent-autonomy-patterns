'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');

const {
  evaluateCase,
  runConformance,
} = require('../src/conformance');
const {
  buildEvidencePacket,
  canonicalJson,
} = require('../src/report');

const root = path.resolve(__dirname, '..');
const protocolPath = path.join(root, 'protocol', 'v0.1.0.json');
const fixturesPath = path.join(root, 'fixtures', 'cases.v0.1.0.json');
const evidencePath = path.join(root, 'evidence', 'conformance-v0.1.0.json');
const protocolBytes = fs.readFileSync(protocolPath);
const fixtureBytes = fs.readFileSync(fixturesPath);
const protocol = JSON.parse(protocolBytes);
const fixtures = JSON.parse(fixtureBytes);

test('protocol 0.1.0 is frozen around all five gates and both workflow boundaries', () => {
  assert.equal(protocol.version, '0.1.0');
  assert.equal(protocol.status, 'frozen');
  assert.deepEqual(Object.keys(protocol.gates), [
    'plan',
    'permission',
    'tool_trust',
    'verification',
    'runtime',
  ]);
  assert.deepEqual(protocol.workflow.mutation_required_gates, [
    'plan',
    'permission',
    'tool_trust',
    'runtime',
  ]);
  assert.deepEqual(protocol.workflow.completion_required_gates, [
    'verification',
  ]);
});

test('every frozen case produces its exact expected decision', async t => {
  for (const caseDefinition of fixtures.cases) {
    await t.test(caseDefinition.id, () => {
      assert.deepEqual(evaluateCase(caseDefinition), caseDefinition.expected);
    });
  }
});

test('the reference suite conforms across every gate', () => {
  const result = runConformance(fixtures.cases);

  assert.equal(result.total, fixtures.cases.length);
  assert.equal(result.passed, fixtures.cases.length);
  assert.equal(result.failed, 0);

  for (const gate of [
    'plan',
    'permission',
    'tool_trust',
    'verification',
    'runtime',
    'workflow',
  ]) {
    assert.ok(result.by_gate[gate].total > 0);
    assert.equal(result.by_gate[gate].failed, 0);
  }
});

test('malformed completion authority fails closed', () => {
  const result = evaluateCase({
    id: 'WORKFLOW-MALFORMED-AUTHORITY',
    gate: 'workflow',
    input: {
      boundary: 'completion',
      mutation_authorized: 'false',
      gate_results: {
        verification: 'allow',
      },
    },
  });

  assert.deepEqual(result, {
    outcome: 'block',
    code: 'workflow.invalid_input',
    effects: ['stop_completion'],
  });
});

test('every deliberately unsafe gate mutant is killed by public fixtures', () => {
  for (const mutant of fixtures.mutants) {
    const result = runConformance(fixtures.cases, { mutant: mutant.id });

    assert.ok(
      result.failed > 0,
      `${mutant.id} survived every frozen case`,
    );
    assert.ok(
      result.cases.some(caseResult => caseResult.gate === mutant.gate && !caseResult.passed),
      `${mutant.id} was not detected by its owning gate`,
    );
  }
});

test('the evidence packet is deterministic and matches the committed report', () => {
  const first = buildEvidencePacket({
    protocol,
    fixtures,
    protocolBytes,
    fixtureBytes,
  });
  const second = buildEvidencePacket({
    protocol,
    fixtures,
    protocolBytes,
    fixtureBytes,
  });

  assert.deepEqual(first, second);
  assert.equal(canonicalJson(first), canonicalJson(second));
  assert.deepEqual(JSON.parse(fs.readFileSync(evidencePath, 'utf8')), first);
});

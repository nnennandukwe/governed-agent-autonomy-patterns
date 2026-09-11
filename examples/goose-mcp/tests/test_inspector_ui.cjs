'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const {sessionMetrics, pillarData, eventInfo, initialEvent, humanReason, tone} = require('../inspector/app.js');

function run(status, recorded, verification = null) {
  return {elapsed_ms: 12, verification,
    receipt: {body: {terminal_status: status, terminal_reason: status === 'blocked' ? 'permission.denied' : 'workflow.completion_authorized',
      usage: {tool_calls: status === 'blocked' ? 0 : 2}, events: recorded}}};
}
const plan = {sequence: 3, event_type: 'plan_recorded'};
const denied = {sequence: 6, event_type: 'protected_effect_decision', gate: 'permission', decision: {outcome: 'block', code: 'permission.denied'}};

test('permission denial preserves the distinction between blocked and unevaluated gates', () => {
  const blocked = run('blocked', [plan, denied]);
  const pillars = pillarData(blocked);
  assert.deepEqual(pillars.map(pillar => pillar.value), ['Plan recorded', 'block', 'Not evaluated', 'Not evaluated', 'Not evaluated']);
  assert.equal(pillars[0].sequence, 3);
  assert.equal(pillars[1].sequence, 6);
  assert.equal(pillars[2].tone, 'muted');
  assert.equal(initialEvent(blocked), 6);
  assert.match(humanReason(blocked), /did not execute/);
});

test('a verified run exposes all five evidence paths and chooses verification first', () => {
  const recorded = [plan, ...['permission', 'tool_trust', 'runtime'].map((gate, index) => ({sequence: 6 + index,
    event_type: 'protected_effect_decision', gate, decision: {outcome: 'allow', code: gate + '.allowed'}})),
  {sequence: 15, event_type: 'verification', verdict: 'PASS'}];
  const completed = run('completed', recorded, {passed: true, cases: [{passed: true}, {passed: true}]});
  assert.deepEqual(pillarData(completed).map(pillar => pillar.value), ['Plan recorded', 'allow', 'allow', 'Passed', 'allow']);
  assert.equal(pillarData(completed)[3].tab, 'verification');
  assert.equal(pillarData(completed)[3].note, '2 / 2 cases passed');
  assert.equal(initialEvent(completed), 15);
});

test('failed verification and missing plans are never colored as successful', () => {
  const failed = run('blocked', [], {passed: false, validation_error: 'Unsupported program', cases: []});
  const pillars = pillarData(failed);
  assert.equal(pillars[0].value, 'Not evaluated');
  assert.equal(pillars[0].tone, 'muted');
  assert.equal(pillars[3].value, 'Failed');
  assert.equal(pillars[3].tone, 'bad');
  assert.equal(tone('unknown_outcome'), 'bad');
});

test('session totals derive from actual recorded runs, including an empty session', () => {
  assert.deepEqual(sessionMetrics([]), {runs: 0, completed: 0, blocked: 0, events: 0, elapsed: 0});
  assert.deepEqual(sessionMetrics([run('completed', [plan]), run('blocked', [plan, denied])]),
    {runs: 2, completed: 1, blocked: 1, events: 3, elapsed: 24});
});

test('event presentation uses recorded outcomes and does not invent event timing', () => {
  assert.deepEqual(eventInfo(denied), {component: 'permission', name: 'permission.denied', result: 'block'});
  const usage = eventInfo({event_type: 'usage', sequence: 14, usage: {elapsed_ms: 132, tool_calls: 2, model_tokens: 0, cost_micros: 0}});
  assert.equal(usage.name, '2 calls / 132 ms accounted');
  assert.equal(usage.duration, undefined);
  assert.equal(usage.timestamp, undefined);
  assert.equal(usage.model_tokens, undefined);
});

'use strict';
let state = null, selected = null, selectedEvent = null, activeTab = 'trace', fingerprint = '';
const $ = id => document.getElementById(id);
function el(tag, text, cls) {
  const node = document.createElement(tag);
  if (text !== undefined) node.textContent = text;
  if (cls) node.className = cls;
  return node;
}
function runStatus(run) { return run.receipt.body.terminal_status; }
function events(run) { return run.receipt.body.events; }
function gateEvent(run, gate) { return events(run).filter(event => event.gate === gate).at(-1); }
function tone(value) {
  if (['completed', 'allow', 'pass', 'PASS', 'executed'].includes(value)) return 'good';
  if (['blocked', 'block', 'ask', 'denied'].includes(value)) return 'blocked';
  if (['failed', 'FAIL', 'unknown_outcome', 'recoverable_incomplete'].includes(value)) return 'bad';
  return 'muted';
}
function badge(value, label = value) { return el('span', label, `badge ${tone(value)}`); }
function title(run) {
  return run.path === 'shipping.py' ? 'Shipping implementation' : run.path === 'deployment.json' ? 'Production activation' : 'Outside-scope change';
}
function humanReason(run) {
  const reason = run.receipt.body.terminal_reason ?? '';
  if (reason === 'permission.denied') return 'Production configuration is outside this session\'s authority. The requested write did not execute.';
  if (reason.includes('stale_subject')) return 'The project changed after the request was prepared. This authority does not cover the current state.';
  if (reason.includes('verification')) return 'Completion requirements were not met. Inspect the observed file and independent verification evidence.';
  if (runStatus(run) === 'completed') return 'The code change executed and independent acceptance checks passed.';
  return 'The engine stopped this run. Inspect its decisions and observed effects.';
}
function sessionMetrics(runs) {
  return {
    runs: runs.length,
    completed: runs.filter(run => runStatus(run) === 'completed').length,
    blocked: runs.filter(run => runStatus(run) === 'blocked').length,
    events: runs.reduce((total, run) => total + events(run).length, 0),
    elapsed: runs.reduce((total, run) => total + run.elapsed_ms, 0),
  };
}
function pillarData(run) {
  const plan = events(run).find(event => event.event_type === 'plan_recorded');
  const permission = gateEvent(run, 'permission'), trust = gateEvent(run, 'tool_trust'), runtime = gateEvent(run, 'runtime');
  const verification = run.verification;
  const gate = (name, event, allowedNote) => ({name, value: event?.decision.outcome ?? 'Not evaluated',
    note: event ? allowedNote : 'Gate not reached', tone: tone(event?.decision.outcome), sequence: event?.sequence, tab: 'trace'});
  return [
    {name: 'Planning', value: plan ? 'Plan recorded' : 'Not evaluated', note: plan ? 'Bound to proposed change' : 'No recorded plan', tone: plan ? 'good' : 'muted', sequence: plan?.sequence, tab: 'trace'},
    gate('Permissions', permission, permission?.decision.outcome === 'allow' ? 'Within granted scope' : 'Outside granted scope'),
    gate('Tool trust', trust, trust?.decision.outcome === 'allow' ? 'Capability + schema matched' : 'Inspect trust decision'),
    {name: 'Verification', value: verification ? (verification.passed ? 'Passed' : 'Failed') : 'Not evaluated',
      note: verification ? `${(verification.cases ?? []).filter(item => item.passed).length} / ${(verification.cases ?? []).length} cases passed` : 'Verifier not reached',
      tone: verification ? (verification.passed ? 'good' : 'bad') : 'muted', tab: 'verification'},
    {...gate('Runtime accountability', runtime, runtime?.decision.outcome === 'allow' ? 'Admission within budget' : 'Inspect runtime decision'),
      note: `${run.receipt.body.usage.tool_calls} accounted calls / ${run.elapsed_ms} ms local`},
  ];
}
function eventInfo(event) {
  if (event.decision) return {component: event.gate, name: event.decision.code, result: event.decision.outcome};
  switch (event.event_type) {
    case 'status_transition': return {component: 'engine', name: `${event.from} -> ${event.to}`, result: event.to};
    case 'plan_recorded': return {component: 'planning', name: 'Record bound proposal', result: 'recorded'};
    case 'approval_recorded': return {component: 'authority', name: 'Record preauthorized policy scope', result: 'recorded'};
    case 'tool_execution': return {component: 'executor', name: 'Record tool execution evidence', result: 'recorded'};
    case 'mutation': return {component: 'filesystem', name: 'Record observed subject change', result: 'recorded'};
    case 'verification': return {component: 'verifier', name: 'Independent acceptance verification', result: event.verdict};
    case 'usage': return {component: 'runtime', name: `${event.usage.tool_calls} calls / ${event.usage.elapsed_ms} ms accounted`, result: 'measured'};
    default: return {component: 'engine', name: event.event_type.replaceAll('_', ' '), result: 'recorded'};
  }
}
function initialEvent(run) {
  const chosen = events(run).find(event => event.decision?.outcome === 'block')
    ?? events(run).find(event => event.event_type === 'verification')
    ?? events(run)[0];
  return chosen?.sequence;
}
function attributes(parent, pairs) {
  const list = el('dl', undefined, 'attribute-list');
  for (const [name, value, prose] of pairs) {
    if (value === undefined || value === null) continue;
    list.append(el('dt', name), el('dd', typeof value === 'object' ? JSON.stringify(value) : String(value), prose ? 'prose' : ''));
  }
  parent.append(list);
}
function rawDetails(parent, label, value) {
  const disclosure = el('details');
  disclosure.append(el('summary', label), el('pre', JSON.stringify(value, null, 2)));
  parent.append(disclosure);
}
function renderMetrics(runs) {
  const totals = sessionMetrics(runs), metrics = $('metrics');
  metrics.replaceChildren();
  for (const [label, value, color, unit] of [
    ['Bounded runs', totals.runs, '', ''], ['Completed', totals.completed, 'good', ''],
    ['Blocked', totals.blocked, 'blocked', ''], ['Recorded events', totals.events, '', ''],
    ['Summed local elapsed', totals.elapsed, '', 'ms'],
  ]) {
    const item = el('div', undefined, 'metric'), number = el('strong', String(value), `metric-value ${color}`);
    if (unit) number.append(el('small', unit));
    item.append(el('span', label, 'metric-label'), number);metrics.append(item);
  }
}
function renderRuns(runs) {
  const body = $('runs');body.replaceChildren();
  if (!runs.length) {
    const row = el('tr'), cell = el('td', 'No bounded runs recorded. Waiting for Goose to submit a change.', 'no-runs');
    cell.colSpan = 6;row.append(cell);body.append(row);return;
  }
  runs.forEach((run, index) => {
    const row = el('tr', undefined, index === selected ? 'selected' : '');
    const operation = el('td'), button = el('button', run.receipt.body.run_id, 'run-select');
    button.type = 'button';button.setAttribute('aria-label', `Inspect ${run.receipt.body.run_id}: ${title(run)}, ${runStatus(run)}`);
    button.setAttribute('aria-current', String(index === selected));button.append(el('span', title(run)));operation.append(button);
    const status = el('td');status.append(badge(runStatus(run)));
    const verification = run.verification;
    row.append(operation, status, el('td', run.path, 'mono'), el('td', `${run.elapsed_ms} ms`, 'mono'),
      el('td', String(run.receipt.body.usage.tool_calls), 'mono'),
      el('td', verification ? `${(verification.cases ?? []).filter(item => item.passed).length}/${(verification.cases ?? []).length} passed` : 'Not evaluated', verification ? (verification.passed ? 'good' : 'bad') : 'muted'));
    row.addEventListener('click', () => {selected = index;selectedEvent = initialEvent(run);render(state);});body.append(row);
  });
}
function renderPillars(run) {
  const parent = $('pillars');parent.replaceChildren();
  pillarData(run).forEach((pillar, index) => {
    const button = el('button', undefined, 'pillar');button.type = 'button';
    button.setAttribute('aria-label', `${pillar.name}: ${pillar.value}. Inspect evidence.`);
    const label = el('span', undefined, 'pillar-label');label.append(el('span', String(index + 1).padStart(2, '0'), 'pillar-index'), el('span', pillar.name));
    button.append(label, el('strong', pillar.value, `pillar-value ${pillar.tone}`), el('span', pillar.note, 'pillar-note'));
    button.title = `${pillar.name}: ${pillar.value}. ${pillar.note}`;
    button.addEventListener('click', () => {activeTab = pillar.tab;selectedEvent = pillar.sequence ?? initialEvent(run);renderDetail(run);});parent.append(button);
  });
}
function renderEventInspector(parent, run, event) {
  parent.append(el('p', 'EVENT ATTRIBUTES', 'inspector-kicker'), el('h3', event.event_type));
  const info = eventInfo(event);parent.append(badge(info.result));
  attributes(parent, [['Sequence', `#${event.sequence} of ${events(run).length}`], ['Component', info.component],
    ['Decision code', event.decision?.code], ['Decision ID', event.decision_id],
    ['Subject digest', event.subject_digest], ['Plan digest', event.plan_digest],
    ['Before subject', event.before_subject_digest], ['After subject', event.after_subject_digest],
    ['Policy actor', event.approval?.actor_id], ['Authority scope', event.approval?.scope, true],
    ['Verifier', event.verifier_id], ['Implementer', event.implementer_id]]);
  if (event.event_type === 'plan_recorded') attributes(parent, [['Recorded plan', run.plan, true]]);
  if (event.decision?.outcome === 'block') attributes(parent, [['Decision context', humanReason(run), true]]);
  if (event.event_type === 'usage') attributes(parent, [['Accounted calls', event.usage.tool_calls], ['Accounted elapsed', `${event.usage.elapsed_ms} ms`], ['Model usage', 'Not measured by this boundary', true]]);
  if (event.event_type === 'tool_execution') attributes(parent, [['Observed effect', run.protected_effect_results.map(result => result.body.execution_status).join(', ')]]);
  if (event.evidence?.length) attributes(parent, [['Evidence references', event.evidence.length]]);
  rawDetails(parent, 'Raw event', event);
}
function renderTrace(parent, run) {
  const layout = el('div', undefined, 'trace-layout'), workspace = el('div', undefined, 'trace-workspace');
  const caption = el('div', undefined, 'trace-caption');caption.append(el('strong', 'Engine event sequence'), el('span', `#1 - #${events(run).length} / ordered, not time-scaled`));workspace.append(caption);
  const list = el('div', undefined, 'event-list');list.setAttribute('aria-label', 'Engine events in recorded order');
  const chosen = events(run).find(event => event.sequence === selectedEvent) ?? events(run)[0];
  for (const event of events(run)) {
    const info = eventInfo(event), row = el('button', undefined, 'event-row');row.type = 'button';
    row.setAttribute('aria-pressed', String(event === chosen));
    row.setAttribute('aria-label', `Event ${event.sequence}: ${info.component}, ${info.name}, ${info.result}`);
    const label = el('span', undefined, 'event-label');
    label.append(el('span', '|-', 'branch'), el('span', info.component, 'component'), el('span', info.name));label.title = `${info.component}: ${info.name}`;
    const track = el('span', undefined, 'sequence-track');track.setAttribute('aria-hidden', 'true');
    track.style.setProperty('--event-count', String(events(run).length));track.style.setProperty('--event-position', String(event.sequence));
    track.append(el('span', undefined, `sequence-point ${tone(info.result) === 'muted' ? 'accent' : tone(info.result)}`));
    row.append(el('span', String(event.sequence).padStart(2, '0'), 'event-number'), label, el('span', info.result, `event-result ${tone(info.result)}`), track);
    row.addEventListener('click', () => {
      const scroll = list.scrollTop;selectedEvent = event.sequence;renderDetail(run);
      document.querySelector('.event-list').scrollTop = scroll;
    });list.append(row);
  }
  workspace.append(list);const inspector = el('aside', undefined, 'event-inspector');
  if (chosen) renderEventInspector(inspector, run, chosen);
  layout.append(workspace, inspector);parent.append(layout);
  const activeRow = list.children[events(run).indexOf(chosen)];
  if (activeRow && list.clientHeight) list.scrollTop = Math.max(0, activeRow.offsetTop - (list.clientHeight - activeRow.offsetHeight) / 2);
}
function codeBlock(label, source) {
  const column = el('div', undefined, 'code-column');column.append(el('div', label, 'code-heading'));
  const code = el('pre', undefined, 'code-view');
  (source ?? 'No accessible file at this path').split('\n').forEach((text, index) => {
    const line = el('span', undefined, 'code-line');line.append(el('span', String(index + 1), 'line-number'), el('span', text || ' '));code.append(line);
  });column.append(code);return column;
}
function renderChanges(parent, run) {
  const pane = el('div', undefined, 'content-pane'), plan = el('div', undefined, 'plan-block');
  plan.append(el('h3', 'Recorded plan'), el('p', run.plan));pane.append(plan);
  const effect = run.protected_effect_results.map(result => result.body.execution_status).join(', ') || 'not dispatched';
  const intro = el('div', undefined, 'pane-intro');intro.append(el('span', run.path, 'mono'), badge(effect, `Effect: ${effect}`));pane.append(intro);
  const split = el('div', undefined, 'code-split');split.append(codeBlock('Before operation', run.before), codeBlock('Observed after operation', run.after));pane.append(split);
  if (run.proposed !== run.after) {
    const requested = el('details');requested.append(el('summary', 'Proposed contents / distinct from observed file'), codeBlock('Requested change', run.proposed));pane.append(requested);
  }
  parent.append(pane);
}
function renderVerification(parent, run) {
  const pane = el('div', undefined, 'content-pane'), verification = run.verification;
  if (!verification) {
    pane.append(el('h3', 'Verification not evaluated'), el('p', 'This run stopped before the verifier. Select an earlier completed run to inspect its acceptance results.', 'pane-note'));
    parent.append(pane);return;
  }
  const intro = el('div', undefined, 'pane-intro');intro.append(el('span', verification.verifier, 'mono'), badge(verification.passed ? 'pass' : 'failed'));pane.append(intro);
  pane.append(el('p', `${verification.elapsed_ms} ms measured. ${verification.fresh ? 'Evidence matches the observed project.' : 'Evidence is stale or the project changed during verification.'}`, 'pane-note'));
  if (verification.validation_error) pane.append(el('p', verification.validation_error, 'bad'));
  const scroll = el('div', undefined, 'table-scroll'), table = el('table', undefined, 'data-table'), head = el('thead'), labels = el('tr');
  for (const label of ['Acceptance case', 'Input (qty / price)', 'Expected', 'Observed', 'Result']) {const th = el('th', label);th.scope = 'col';labels.append(th);}head.append(labels);table.append(head);
  const body = el('tbody');
  for (const item of verification.cases ?? []) {
    const row = el('tr'), result = el('td');result.append(badge(item.passed ? 'pass' : 'failed'));
    row.append(el('td', item.name), el('td', `${item.quantity} / ${item.unit_price}`, 'mono'), el('td', String(item.expected), 'mono'),
      el('td', item.observed?.value !== undefined ? String(item.observed.value) : item.observed?.error ?? 'Not executed', 'mono'), result);body.append(row);
  }
  table.append(body);scroll.append(table);pane.append(scroll);
  attributes(pane, [['Verified subject', verification.subject_digest], ['Execution boundary', verification.execution_policy, true]]);parent.append(pane);
}
function renderReceipt(parent, run) {
  const pane = el('div', undefined, 'content-pane'), grid = el('div', undefined, 'evidence-grid'), identity = el('div'), usage = el('div');
  attributes(identity, [['Run ID', run.receipt.body.run_id], ['Terminal status', runStatus(run)], ['Terminal reason', run.receipt.body.terminal_reason],
    ['Receipt digest', run.receipt.receipt_digest], ['Initial subject', run.receipt.body.initial_subject_digest], ['Resulting subject', run.receipt.body.resulting_subject_digest]]);
  attributes(usage, [['Local run elapsed', `${run.elapsed_ms} ms`], ['Accounted elapsed', `${run.receipt.body.usage.elapsed_ms} ms`],
    ['Accounted tool calls', `${run.receipt.body.usage.tool_calls} / ${run.request.resource_budget.max_tool_calls}`],
    ['Engine elapsed budget', `${run.request.resource_budget.max_elapsed_ms} ms`], ['Model tokens / provider cost', 'Not measured by this MCP boundary', true],
    ['Receipt integrity', run.terminal_receipt_verified ? 'Verified locally; not a signed attestation' : 'Not verified', true]]);
  grid.append(identity, usage);pane.append(grid);
  rawDetails(pane, 'Full request, terminal receipt, and execution evidence', run);parent.append(pane);
}
function renderDetail(run) {
  const tabs = $('tabs');tabs.replaceChildren();
  const choices = [['trace', 'Trace'], ['changes', 'Changes'], ['verification', 'Verification'], ['receipt', 'Receipt']];
  choices.forEach(([key, label], index) => {
    const button = el('button', label, 'tab');button.type = 'button';button.id = `tab-${key}`;button.setAttribute('role', 'tab');
    button.setAttribute('aria-selected', String(activeTab === key));button.setAttribute('aria-controls', 'detail');button.tabIndex = activeTab === key ? 0 : -1;
    button.addEventListener('click', () => {activeTab = key;renderDetail(run);$(`tab-${key}`).focus();});
    button.addEventListener('keydown', event => {
      let target;
      if (event.key === 'ArrowRight') target = (index + 1) % choices.length;
      if (event.key === 'ArrowLeft') target = (index + choices.length - 1) % choices.length;
      if (event.key === 'Home') target = 0;
      if (event.key === 'End') target = choices.length - 1;
      if (target !== undefined) {event.preventDefault();activeTab = choices[target][0];renderDetail(run);$(`tab-${activeTab}`).focus();}
    });tabs.append(button);
  });
  const parent = $('detail');parent.replaceChildren();parent.setAttribute('aria-labelledby', `tab-${activeTab}`);
  ({trace: renderTrace, changes: renderChanges, verification: renderVerification, receipt: renderReceipt})[activeTab](parent, run);
}
function render(next) {
  state = next;const runs = state.runs ?? [];
  if (selected === null && runs.length) {
    const completed = runs.findLastIndex(run => runStatus(run) === 'completed');
    selected = state.view_mode === 'recorded' && completed >= 0 ? completed : runs.length - 1;
  }
  if (selected !== null && selected >= runs.length) selected = runs.length ? runs.length - 1 : null;
  const run = runs[selected];
  $('session-name').textContent = (state.workspace ?? '').split('/').filter(Boolean).at(-2) ?? 'Local coding session';
  $('location').textContent = state.workspace ?? '';
  $('count').textContent = runs.length;
  $('mode').textContent = state.view_mode === 'recorded' ? 'Recorded session' : 'Live session';
  const status = $('session-state');status.className = `session-state${state.inflight ? ' inflight' : ''}`;
  if (state.inflight) status.textContent = 'Unresolved attempt recorded. It may be executing or require recovery; completed records remain available.';
  else if (runs.length) status.textContent = `${runs.length} bounded runs recorded. Select a run to inspect its authority, execution, and completion evidence.`;
  else status.textContent = 'No operations recorded. Start the coding activity in Goose to populate this session.';
  renderMetrics(runs);renderRuns(runs);
  $('empty').hidden = Boolean(run);$('selection').hidden = !run;
  if (!run) return;
  $('run-id').textContent = `${run.receipt.body.run_id} / gaap.submit_change / ${run.path}`;
  $('run-title').textContent = title(run);$('run-outcome').replaceChildren(badge(runStatus(run)));
  const reason = $('run-reason');reason.className = `decision-summary ${tone(runStatus(run))}`;
  reason.replaceChildren(el('code', run.receipt.body.terminal_reason ?? 'No terminal reason'), el('span', humanReason(run)));
  $('receipt-state').textContent = run.terminal_receipt_verified ? 'Receipt integrity verified' : 'Receipt not verified';
  $('receipt-state').className = `receipt-state ${run.terminal_receipt_verified ? 'good' : 'bad'}`;
  if (!events(run).some(event => event.sequence === selectedEvent)) selectedEvent = initialEvent(run);
  renderPillars(run);renderDetail(run);
}
async function refresh() {
  try {
    const response = await fetch('state', {cache: 'no-store'}), next = await response.json();
    if (!response.ok || next.error) throw new Error(next.error || 'Inspector unavailable');
    const signature = JSON.stringify(next);
    if (signature !== fingerprint) {
      if (state && next.runs.length > state.runs.length) {selected = next.runs.length - 1;selectedEvent = initialEvent(next.runs[selected]);}
      render(next);fingerprint = signature;
    }
    $('connection').textContent = 'Connected to local records';$('connection').className = 'good';
  } catch (error) {
    $('connection').textContent = 'Disconnected / data may be stale';$('connection').className = 'blocked';
  }
  setTimeout(refresh, 750);
}
if (typeof module !== 'undefined' && module.exports) module.exports = {sessionMetrics, pillarData, eventInfo, initialEvent, humanReason, tone};
if (typeof document !== 'undefined') refresh();

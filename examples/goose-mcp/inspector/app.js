'use strict';
let state = null, selected = null, fingerprint = '';
const $ = id => document.getElementById(id);
function el(tag, text, cls) { const node = document.createElement(tag); if (text !== undefined) node.textContent = text; if (cls) node.className = cls; return node; }
function heading(parent, text) { parent.append(el('h3', text)); }
function status(run) { return run.receipt.body.terminal_status; }
function events(run) { return run.receipt.body.events; }
function decision(run, gate) { return events(run).filter(e => e.gate === gate).at(-1)?.decision; }
function tone(value) { return ['completed', 'allow', 'pass'].includes(value) ? 'good' : ['blocked', 'block', 'ask'].includes(value) ? 'blocked' : 'muted'; }
function title(run) { return run.path === 'shipping.py' ? 'Shipping implementation' : run.path === 'deployment.json' ? 'Production activation' : 'Outside-scope change'; }
function humanReason(run) {
  const reason = run.receipt.body.terminal_reason;
  if (reason === 'permission.policy_denied') return 'The session permits application code changes. Production configuration is outside that authority.';
  if (reason.includes('stale_subject')) return 'The project changed since this request was prepared. Earlier authority does not cover the new state.';
  if (reason.includes('verification')) return 'The proposed code was checked, but completion requirements were not satisfied. Inspect the actual file and test evidence below.';
  if (status(run) === 'completed') return 'The code change executed and independent acceptance checks passed for the resulting project.';
  return 'The engine stopped this bounded run. Its recorded reason and observed effects are shown below.';
}
function pillars(run) {
  const p = $('pillars'); p.replaceChildren();
  let data;
  if (!run) data = [['Planning','Waiting for a plan','Task scope is configured'],['Permissions','Policy ready','Code allowed · production denied'],['Tool trust','Awaiting request','Identity checked before execution'],['Verification','Not evaluated','Independent acceptance checks'],['Runtime','No runs yet','Bounded effects and verification']];
  else {
    const perm = decision(run,'permission'), trust = decision(run,'tool_trust'), runtime = decision(run,'runtime');
    const v = run.verification;
    data = [
      ['Planning',events(run).some(e=>e.event_type==='plan_recorded')?'Plan recorded':'Not evaluated','Proposal and permitted scope are bound','good'],
      ['Permissions',perm?.outcome ?? 'Not evaluated',perm?.code ?? 'Gate not reached',tone(perm?.outcome)],
      ['Tool trust',trust?.outcome==='allow'?'Identity matched':trust?.outcome ?? 'Not evaluated',trust?'Capability and schema checked':'Stopped before this gate',tone(trust?.outcome)],
      ['Verification',v?(v.passed?'Passed':'Failed'):'Not evaluated',v?`${(v.cases??[]).filter(c=>c.passed).length} / ${(v.cases??[]).length} acceptance cases`:'Stopped before verification',v?(v.passed?'good':'blocked'):'muted'],
      ['Runtime',runtime?.outcome==='allow'?'Within budget':runtime?.outcome ?? 'Not evaluated',`${run.elapsed_ms} ms measured · ${run.receipt.body.usage.tool_calls} accounted calls`,tone(runtime?.outcome)]
    ];
  }
  for (const [name,value,note,cls] of data) { const item=el('div',undefined,'pillar');item.append(el('h2',name),el('strong',value,cls),el('small',note));p.append(item); }
}
function eventDescription(event) {
  if (event.decision) return `${event.gate.replaceAll('_',' ')}: ${event.decision.outcome} · ${event.decision.code}`;
  if (event.event_type==='status_transition') return `${event.from} → ${event.to}`;
  if (event.event_type==='plan_recorded') return 'Plan recorded and bound to proposed change';
  if (event.event_type==='approval_recorded') return 'Preauthorized local task scope recorded';
  if (event.event_type==='mutation') return 'File changed; before and after subjects recorded';
  if (event.event_type==='verification') return `Independent verification: ${event.verdict}`;
  if (event.event_type==='usage') return `${event.usage.elapsed_ms} ms · ${event.usage.tool_calls} accounted calls`;
  return event.event_type.replaceAll('_',' ');
}
function details(run) {
  const node=$('detail');node.replaceChildren();
  node.append(el('p',status(run),`outcome ${tone(status(run))}`),el('h2',title(run)),el('p',humanReason(run)),el('p',run.receipt.body.terminal_reason,'reason'));
  heading(node,'Recorded plan');node.append(el('p',run.plan));
  const effects=run.protected_effect_results.map(r=>r.body.execution_status);
  const flow=el('ul',undefined,'flow');
  for (const text of [`Effect: ${effects.join(', ') || 'not dispatched'}`,`Completion: ${status(run)}`,`Receipt: ${run.terminal_receipt_verified?'integrity verified':'not verified'}`]) flow.append(el('li',text));
  node.append(flow);
  const diff=el('div',undefined,'split');
  for(const [label,content] of [['Before',run.before],['Observed after',run.after]]) {const col=el('div');heading(col,label);col.append(el('pre',content ?? 'No accessible file at this path'));diff.append(col);}node.append(diff);
  if(effects.includes('denied')) {const proposed=el('details');proposed.append(el('summary','Requested change · not executed'),el('pre',run.proposed));node.append(proposed);}
  heading(node,'Independent verification');
  if(run.verification) {
    if(run.verification.validation_error) node.append(el('p',run.verification.validation_error,'blocked'));
    node.append(el('p',run.verification.fresh?'Evidence covers the observed project after this change.':'Evidence is stale or the project changed during verification.','muted'));
    const table=el('table',undefined,'tests');const head=el('tr');for(const name of ['Acceptance case','Expected','Observed','Result'])head.append(el('th',name));const thead=el('thead');thead.append(head);table.append(thead);const body=el('tbody');
    for(const c of run.verification.cases??[]) {const row=el('tr');row.append(el('td',c.name),el('td',String(c.expected)),el('td',c.observed?.value!==undefined?String(c.observed.value):c.observed?.error??'Not executed'),el('td',c.passed?'Pass':'Fail',c.passed?'good':'blocked'));body.append(row);}table.append(body);node.append(table);
  } else node.append(el('p','This run stopped before verification. Earlier verified work remains in the session history.','muted'));
  heading(node,'Engine events');
  for(const event of events(run)) {const row=el('div',undefined,'event');row.append(el('span',String(event.sequence).padStart(2,'0'),'num'),el('span',eventDescription(event),tone(event.decision?.outcome)));node.append(row);}
  const evidence=el('details');evidence.append(el('summary','Inspect request, receipt, and execution evidence'),el('pre',JSON.stringify(run,null,2)));node.append(evidence);
}
function render(next) {
  state=next;const runs=state.runs??[];
  if(selected===null && runs.length) selected=runs.length-1;
  const run=runs[selected];
  $('location').textContent=state.workspace;
  $('count').textContent=runs.length;
  $('limits').textContent='Per run: 1 effect · 2 accounted calls · 10 seconds. Verification: 5-second deadline.';
  const nav=$('runs');nav.replaceChildren();
  runs.forEach((item,index)=>{const button=el('button',undefined,'run');button.setAttribute('aria-current',String(index===selected));button.append(el('strong',`${index+1}. ${title(item)}`),el('span',`${status(item)} · ${item.elapsed_ms} ms`,tone(status(item))));button.addEventListener('click',()=>{selected=index;render(state);});nav.append(button);});
  if(state.inflight) { $('headline').textContent='An operation is in progress.';$('summary').textContent='A durable attempt is recorded. If execution stops unexpectedly, its outcome requires inspection.'; }
  else if(runs.some(r=>r.path==='deployment.json' && status(r)==='blocked')) { const currentVerified=runs.some(r=>r.verification?.passed && r.verification.subject_digest===state.subject_digest); $('headline').textContent=currentVerified?'Code verified. Production protected.':'Production change blocked.';$('summary').textContent='Inspect the implementation evidence and the separate production request below.'; }
  else if(runs.length) {$('headline').textContent=status(runs.at(-1))==='completed'?'A verified code change.':'Completion requirements not met.';$('summary').textContent='Every outcome is backed by the engine’s recorded decisions and observations.';}
  else {$('headline').textContent='A coding task. Explicit boundaries.';$('summary').textContent='Goose implements the shipping rules. GAAP evaluates each proposed change.';}
  pillars(run);if(run)details(run);
}
async function refresh(){
  try {const response=await fetch('state',{cache:'no-store'});const next=await response.json();if(!response.ok||next.error)throw new Error(next.error||'Inspector unavailable');
    const signature=JSON.stringify(next);if(signature!==fingerprint){if(state && next.runs.length>state.runs.length)selected=next.runs.length-1;fingerprint=signature;render(next);}
    $('connection').textContent='● Live · local records';$('connection').className='good';
  }catch(error){$('connection').textContent='Disconnected · last observed data';$('connection').className='blocked';}
  setTimeout(refresh,750);
}
refresh();

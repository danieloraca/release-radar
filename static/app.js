'use strict';
const $ = (selector, root = document) => root.querySelector(selector);
let workspace;
let busy = false;
function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}
function notice(message, error = false) {
  const node = $('#notice');
  node.textContent = message;
  node.className = message ? `notice ${error ? 'error' : ''}` : '';
}
async function api(path, method = 'GET', body) {
  const response = await fetch(path, {method, headers: body === undefined ? {} : {'Content-Type': 'application/json'}, body: body === undefined ? undefined : JSON.stringify(body)});
  const result = await response.json().catch(() => ({error: `Request failed (${response.status})`}));
  if (!response.ok) throw new Error(result.error || `Request failed (${response.status})`);
  return result;
}
async function mutate(path, method, body) {
  if (busy) return false;
  busy = true;
  try { await api(path, method, body); await load(); notice('Saved.'); return true; }
  catch (error) { notice(error.message, true); return false; }
  finally { busy = false; }
}
function pill(text, tone = '') { return el('span', `pill ${tone}`, text); }
function tone(text) {
  if (['passing', 'approved at head', 'observed gates clear', 'success'].includes(text)) return 'good';
  if (['failing', 'changes requested'].includes(text)) return 'bad';
  return 'warn';
}
function safeLink(url, text) {
  const a = el('a', '', text);
  try { const u = new URL(url); if (u.protocol === 'https:' && ['github.com', 'api.github.com'].includes(u.hostname)) a.href = u.href; } catch { /* no link */ }
  a.target = '_blank'; a.rel = 'noopener noreferrer';
  return a;
}
function renderPr(pr) {
  const card = el('section', 'pr');
  const top = el('div', 'pr-top');
  top.append(safeLink(pr.url, `${pr.reference.owner}/${pr.reference.repo} #${pr.reference.number}`));
  const remove = el('button', 'quiet', 'Unlink');
  remove.onclick = () => { if (confirm('Unlink this PR? Existing test passes will need to be checked again.')) mutate(`/api/prs/${pr.id}`, 'DELETE'); };
  top.append(remove); card.append(top);
  const s = pr.snapshot;
  if (!s) { card.append(el('p', 'muted', 'No metadata yet. Sync GitHub to gather evidence.')); }
  else {
    card.append(el('h4', '', s.title));
    const commit = el('div', 'commit');
    commit.append(pill(s.merged ? 'merged' : s.draft ? 'draft' : s.state), el('code', '', s.head.slice(0, 10)), el('span', 'muted', `→ ${s.base}`));
    card.append(commit);
    if (s.merge_commit) card.append(el('p', 'muted mono', `Merge commit ${s.merge_commit.slice(0, 10)}`));
    const states = el('div', 'states');
    for (const [name, state] of [['Review', pr.reviews_state], ['Checks', pr.checks_state]]) {
      const box = el('div', 'state'); box.append(el('span', 'label', name), pill(state, pr.fresh ? tone(state) : 'warn')); states.append(box);
    }
    card.append(states);
    const evidence = el('div', 'environments');
    for (const env of workspace.environments) {
      const d = s.deployments.find(d => d.environment === env);
      const box = el('div', 'environment');
      const state = d && d.matches ? d.state : 'unknown';
      box.append(el('span', 'label', env), pill(state, pr.fresh ? tone(state) : 'warn'));
      if (d) {
        box.append(el('p', 'muted', d.matches ? `Exact ${d.sha.slice(0, 10)} · latest deployment record` : `Latest record is ${d.sha.slice(0, 10)}. This PR’s deployment is unknown.`));
        box.append(safeLink(d.evidence_url, 'View deployment record ↗'));
        if (d.recorded_at) box.append(el('small', 'muted', new Date(d.recorded_at).toLocaleString()));
      } else box.append(el('p', 'muted', 'No deployment evidence found.'));
      evidence.append(box);
    }
    card.append(evidence);
    if (s.checks.length) {
      const details = el('details', 'check-details'); details.append(el('summary', '', `${s.checks.length} observed check / status contexts`));
      for (const c of s.checks) details.append(el('p', 'muted', `${c.name}: ${c.state}`));
      card.append(details);
    }
    for (const warning of s.warnings) card.append(el('p', 'warning', warning));
    card.append(el('p', `synced ${pr.fresh ? '' : 'warning'}`, `${pr.fresh ? 'Observed' : 'STALE EVIDENCE · observed'} ${new Date(s.observed_at).toLocaleString()}`));
  }
  if (pr.sync_error) card.append(el('p', 'warning', pr.sync_error));
  return card;
}
function render() {
  $('#mode').textContent = workspace.demo ? 'SYNTHETIC DEMO · EPHEMERAL' : 'LOCAL WORKSPACE';
  $('#ticket-count').textContent = workspace.tickets.length;
  $('#pr-count').textContent = workspace.tickets.reduce((n, t) => n + t.prs.length, 0);
  $('#attention-count').textContent = workspace.tickets.filter(t => t.summary !== 'observed gates clear').length;
  const root = $('#tickets'); root.replaceChildren();
  if (!workspace.tickets.length) root.append(el('p', 'empty', 'Nothing on the radar yet. Track a ticket above, then link its PRs.'));
  for (const t of workspace.tickets) {
    const node = $('#ticket-template').content.firstElementChild.cloneNode(true);
    $('.ticket-key', node).textContent = t.key;
    $('h3', node).textContent = t.title;
    const summary = $('.summary', node); summary.textContent = t.summary; summary.classList.add(tone(t.summary));
    $('.remove-ticket', node).onclick = () => { if (confirm(`Remove ${t.key} and its local checklist? GitHub is unaffected.`)) mutate(`/api/tickets/${t.id}`, 'DELETE'); };
    const staging = workspace.environments[0];
    const staged = t.prs.filter(p => p.fresh && p.snapshot?.deployments.some(d => d.environment === staging && d.matches && d.state === 'success')).length;
    $('.coordination', node).textContent = `${t.prs.length} linked PRs · ${staged}/${t.prs.length} have a successful exact-commit ${staging} record. ${staged > 0 && staged < t.prs.length ? 'Coordinated release has an evidence gap.' : 'Only linked PRs are included.'}`;
    for (const pr of t.prs) $('.pr-list', node).append(renderPr(pr));
    const linkForm = $('.pr-form', node);
    linkForm.onsubmit = async event => { event.preventDefault(); await mutate(`/api/tickets/${t.id}/prs`, 'POST', {url: new FormData(linkForm).get('url')}); };
    let passed = 0;
    for (const test of t.tests) {
      const valid = test.passed && test.tested_revision === t.revision;
      if (valid) passed++;
      const row = el('div', 'test-row');
      const label = el('label', 'test-label');
      const checkbox = el('input'); checkbox.type = 'checkbox'; checkbox.checked = valid;
      checkbox.onchange = async () => { await mutate(`/api/tests/${test.id}`, 'PATCH', {passed: checkbox.checked}); checkbox.checked = valid; };
      label.append(checkbox, el('span', '', test.label));
      if (test.passed && !valid) label.append(pill('retest', 'warn'));
      const remove = el('button', 'quiet', 'Remove'); remove.setAttribute('aria-label', `Remove test: ${test.label}`); remove.onclick = () => mutate(`/api/tests/${test.id}`, 'DELETE');
      row.append(label, remove); $('.test-list', node).append(row);
    }
    $('.test-progress', node).textContent = `${passed}/${t.tests.length} passed`;
    const form = $('.test-form', node);
    form.onsubmit = async event => { event.preventDefault(); await mutate(`/api/tickets/${t.id}/tests`, 'POST', {label: new FormData(form).get('label')}); };
    root.append(node);
  }
}
async function load() { workspace = await api('/api/tickets'); render(); }
$('#ticket-form').onsubmit = async event => {
  event.preventDefault();
  const form = event.currentTarget;
  const data = new FormData(form);
  if (await mutate('/api/tickets', 'POST', {key: data.get('key'), title: data.get('title')})) form.reset();
};
$('#sync').onclick = async () => {
  if (busy) return; busy = true; const button = $('#sync'); button.disabled = true; button.textContent = 'Syncing…';
  try { const data = await api('/api/sync', 'POST'); await load(); notice(data.message); }
  catch (error) { notice(error.message, true); }
  finally { busy = false; button.disabled = false; button.textContent = '↻ Sync GitHub'; }
};
load().catch(error => notice(error.message, true));
// Don’t replace a form or checklist while someone is interacting with it.
setInterval(() => { if (!busy && !['INPUT', 'BUTTON'].includes(document.activeElement?.tagName)) load().catch(error => notice(error.message, true)); }, 30000);

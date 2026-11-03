'use strict';
const $ = (id) => document.getElementById(id);
const state = { token: sessionStorage.getItem('relogin.token') || '', page: 1, total: 0, accounts: [], task: null, submitting: false, uncertain: false, timer: null, request: 0, q: '', accountId: '' };
const stages = { session: '建立会话', password: '密码登录', email: '邮箱验证', totp: 'TOTP 验证', token: '获取 Token', enroll: '补绑 2FA', save: '更新账号', done: '完成' };
const errors = { authentication_required: '管理口令不正确，请重新输入。', storage_unavailable: '账号库暂时无法读取，请检查服务配置。', account_not_found: '账号已不存在，请刷新列表。', password_missing: '此账号未保存密码。', invalid_query: '请检查账号 ID 和查询条件。', task_busy: '已有账号正在重新登录。', task_not_found: '任务已不存在，服务重启后任务记录会清空。' };
function element(tag, text, className) { const node = document.createElement(tag); if (text !== undefined) node.textContent = text; if (className) node.className = className; return node; }
function date(value) { return value ? new Date(value * 1000).toLocaleString('zh-CN', { hour12: false }) : '—'; }
function notice(text = '') { $('notice').textContent = text; $('notice').hidden = !text; }
function busy() { return state.submitting || state.uncertain || state.task?.status === 'running'; }
async function api(path, options = {}) {
  const response = await fetch(path, { ...options, headers: { 'Authorization': `Bearer ${state.token}`, ...(options.body ? { 'Content-Type': 'application/json' } : {}) } });
  const data = await response.json();
  if (!response.ok) { const err = new Error(errors[data.error] || `请求失败（HTTP ${response.status}）`); err.status = response.status; err.data = data; throw err; }
  return data;
}
function schedule() { clearTimeout(state.timer); if (state.task?.status === 'running') state.timer = setTimeout(pollTask, 1000); }
function logout(message = '') {
  clearTimeout(state.timer); state.token = ''; state.task = null; state.submitting = false; state.uncertain = false; state.request++;
  sessionStorage.removeItem('relogin.token'); $('dashboard').hidden = true; $('logout').hidden = true; $('login-panel').hidden = false; $('admin-token').value = ''; $('login-error').textContent = message;
}
function handleError(err) { if (err.status === 401) logout(err.message); else notice(err.message || '连接失败，请检查 SSH 隧道后重试。'); }
function renderRows() {
  const rows = state.accounts.map((account) => {
    const tr = element('tr'); tr.dataset.accountId = account.id;
    if (state.task?.account_id === account.id && state.task.status === 'running') tr.className = 'active';
    const name = element('td'); const email = element('span', account.email, 'account-email'); email.title = account.email;
    name.append(email, element('span', `#${account.id}`, 'account-id')); tr.append(name);
    const status = element('td'); status.append(element('span', account.status, `badge ${account.status === 'registered' ? 'ok' : 'neutral'}`)); tr.append(status);
    const credentials = element('td', undefined, 'credentials'); credentials.append(element('span', account.has_password ? '密码已保存' : '缺少密码', account.has_password ? '' : 'missing'), element('span', account.has_access_token ? 'Token 已保存' : '缺少 Token', account.has_access_token ? '' : 'missing')); tr.append(credentials);
    const totp = element('td'); totp.append(element('span', account.has_totp ? '已保存密钥' : '登录后补绑', `badge ${account.has_totp ? 'neutral' : 'warning'}`)); tr.append(totp);
    tr.append(element('td', date(account.updated_at), 'time'));
    const action = element('td'); const active = state.task?.account_id === account.id;
    const button = element('button', active && state.task.status === 'running' ? '登录中…' : '重新登录', 'relogin'); button.type = 'button'; button.disabled = busy() || !account.has_password;
    button.setAttribute('aria-label', `重新登录 ${account.email}`); button.addEventListener('click', () => startTask(account.id)); action.append(button);
    if (active && state.task.status !== 'running') action.append(element('span', state.task.status === 'succeeded' ? '凭据已更新' : '本次失败', `row-result${state.task.status === 'failed' ? ' is-error' : ''}`));
    tr.append(action); return tr;
  });
  $('account-rows').replaceChildren(...rows); $('list-empty').hidden = rows.length > 0;
  $('total').textContent = `${state.total} 个账号`; $('page-info').textContent = state.total ? `${(state.page - 1) * 50 + 1}–${(state.page - 1) * 50 + rows.length} / ${state.total}` : '0 个账号';
  $('page-number').textContent = state.page; $('previous').disabled = state.page <= 1; $('next').disabled = state.page * 50 >= state.total;
}
async function loadAccounts() {
  const sequence = ++state.request;
  const params = new URLSearchParams({ page: String(state.page), page_size: '50' });
  if (state.q) params.set('q', state.q); if (state.accountId) params.set('account_id', state.accountId);
  const result = await api(`/api/accounts?${params}`);
  if (sequence !== state.request) return;
  state.accounts = result.accounts; state.total = result.total; renderRows();
}
function renderTask() {
  const task = state.task; $('task-empty').hidden = !!task; $('task-content').hidden = !task;
  if (!task) { $('task-state').textContent = '未执行'; $('task-state').className = 'badge neutral'; return; }
  const status = task.status; $('task-state').textContent = { running: '执行中', succeeded: '已完成', failed: '失败' }[status] || status;
  $('task-state').className = `badge ${status === 'succeeded' ? 'ok' : status === 'failed' ? 'failed' : 'running'}`;
  $('task-account-id').textContent = `账号 #${task.account_id}`; $('task-email').textContent = task.email; $('task-start').textContent = `开始于 ${date(task.started_at)}`;
  $('stage-list').replaceChildren(...task.events.map((event) => { const li = element('li', stages[event.stage] || event.stage); li.append(element('time', new Date(event.at * 1000).toLocaleTimeString('zh-CN', { hour12: false }))); return li; }));
  $('task-result').textContent = status === 'running' ? `正在${stages[task.stage] || '处理'}，请等待。` : (task.message || '任务已结束');
  $('task-result').className = `task-result ${status === 'succeeded' ? 'success' : status === 'failed' ? 'failure' : ''}`;
  $('retry-status').hidden = true;
}
async function pollTask() {
  try {
    const result = await api(`/api/relogin/${encodeURIComponent(state.task.task_id)}`);
    state.task = result; renderTask(); renderRows();
    if (result.status !== 'running') await loadAccounts();
    schedule();
  } catch (err) {
    if (err.status === 401) return logout(err.message);
    if (err.status === 404) { state.task = null; renderTask(); renderRows(); }
    notice(err.message || '任务进度连接中断。恢复连接后可重新获取进度。'); $('retry-status').hidden = false;
  }
}
async function startTask(accountId) {
  if (busy()) return; state.submitting = true; state.uncertain = true; renderRows(); notice();
  try {
    const result = await api('/api/relogin', { method: 'POST', body: JSON.stringify({ account_id: accountId }) });
    state.task = await api(`/api/relogin/${encodeURIComponent(result.task_id)}`);
    state.uncertain = false;
    renderTask(); schedule();
  } catch (err) {
    // If the POST response was lost, recover the backend task before enabling a second click.
    if (err.status === 401) { logout(err.message); return; }
    try { const current = await api('/api/relogin/current'); state.task = current.task; state.uncertain = false; renderTask(); schedule(); }
    catch (_) { notice('连接中断，请刷新页面以恢复任务状态。'); return; }
    handleError(err);
  } finally { state.submitting = false; renderRows(); }
}
async function enter() {
  $('login-submit').disabled = true; $('login-error').textContent = '';
  try {
    const [current] = await Promise.all([api('/api/relogin/current'), loadAccounts()]);
    state.task = current.task; state.uncertain = false; renderTask(); renderRows(); schedule(); sessionStorage.setItem('relogin.token', state.token);
    $('admin-token').value = ''; $('login-panel').hidden = true; $('dashboard').hidden = false; $('logout').hidden = false;
  } catch (err) { logout(err.message || '服务连接失败，请检查 SSH 隧道。'); }
  finally { $('login-submit').disabled = false; }
}
$('login-form').addEventListener('submit', (event) => { event.preventDefault(); state.token = $('admin-token').value.trim(); enter(); });
$('logout').addEventListener('click', () => logout());
$('refresh').addEventListener('click', () => { notice(); loadAccounts().catch(handleError); });
$('search-form').addEventListener('submit', (event) => { event.preventDefault(); state.page = 1; state.q = $('email-search').value.trim(); state.accountId = $('id-search').value.trim(); notice(); loadAccounts().catch(handleError); });
$('reset-search').addEventListener('click', () => { $('email-search').value = ''; $('id-search').value = ''; state.q = ''; state.accountId = ''; state.page = 1; notice(); loadAccounts().catch(handleError); });
$('previous').addEventListener('click', () => { state.page--; loadAccounts().catch(handleError); });
$('next').addEventListener('click', () => { state.page++; loadAccounts().catch(handleError); });
$('retry-status').addEventListener('click', async () => { notice(); try { state.task = (await api('/api/relogin/current')).task; renderTask(); renderRows(); schedule(); } catch (err) { handleError(err); } });
if (state.token) enter();

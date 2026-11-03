/* Optional desktop smoke test. Install Playwright separately; no frontend runtime dependency. */
const assert = require('node:assert/strict');
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require('playwright');

async function main() {
  const frontend = path.resolve(__dirname, '..');
  const accounts = [
    { id: 2, email: 'second@example.invalid', status: 'registered', has_password: true, has_access_token: true, has_totp: true, updated_at: 1793664000 },
    { id: 1, email: 'first@example.invalid', status: 'registered_no_token', has_password: true, has_access_token: false, has_totp: false, updated_at: 1793664000 },
  ];
  const posts = []; let task = null; let finished = false;
  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url, 'http://localhost');
    res.setHeader('cache-control', 'no-store');
    const json = (status, body) => { res.writeHead(status, { 'content-type': 'application/json' }); res.end(JSON.stringify(body)); };
    if (url.pathname.startsWith('/api/')) {
      if (req.headers.authorization !== 'Bearer synthetic-admin-test-token') return json(401, { error: 'authentication_required' });
      if (url.pathname === '/api/accounts') {
        const needle = url.searchParams.get('q') || ''; const id = url.searchParams.get('account_id');
        const selected = accounts.filter(a => a.email.includes(needle) && (!id || a.id === Number(id)));
        return json(200, { accounts: selected, total: selected.length, page: 1, page_size: 50 });
      }
      if (url.pathname === '/api/relogin' && req.method === 'POST') {
        let body = ''; for await (const chunk of req) body += chunk;
        const { account_id } = JSON.parse(body); posts.push(account_id);
        const account = accounts.find(a => a.id === account_id);
        task = { task_id: 'synthetic-task', account_id, email: account.email, status: 'running', stage: 'password', started_at: 1793664000, finished_at: null, enrolled_2fa: false, message: null, events: [{ stage: 'session', at: 1793664000 }, { stage: 'password', at: 1793664001 }] };
        return json(202, { task_id: task.task_id });
      }
      if (url.pathname === '/api/relogin/current') return json(200, { task });
      if (url.pathname === '/api/relogin/synthetic-task') {
        if (finished) {
          task = { ...task, status: 'succeeded', stage: 'done', finished_at: 1793664030, enrolled_2fa: true, message: '已更新 Access Token、Cookie，并补绑 2FA', events: [...task.events.filter(e => e.stage !== 'done'), { stage: 'done', at: 1793664030 }] };
          Object.assign(accounts.find(a => a.id === task.account_id), { status: 'registered', has_access_token: true, has_totp: true, updated_at: 1793664030 });
        }
        return json(200, task);
      }
      return json(404, { error: 'task_not_found' });
    }
    const assets = { '/': ['index.html', 'text/html'], '/app.js': ['app.js', 'text/javascript'], '/styles.css': ['styles.css', 'text/css'] };
    if (!assets[url.pathname]) { res.writeHead(404); return res.end(); }
    const [name, type] = assets[url.pathname]; res.writeHead(200, { 'content-type': type }); res.end(fs.readFileSync(path.join(frontend, name)));
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const browser = await chromium.launch({ channel: process.env.PLAYWRIGHT_CHANNEL || 'msedge', headless: true });
  try {
    const page = await browser.newPage({ viewport: { width: 1536, height: 960 }, deviceScaleFactor: 1 });
    const browserErrors = []; page.on('pageerror', err => browserErrors.push(err.message));
    await page.goto(`http://127.0.0.1:${server.address().port}`);
    await page.locator('#admin-token').fill('wrong-token'); await page.locator('#login-submit').click();
    await page.waitForFunction(() => document.getElementById('login-error').textContent.includes('管理口令不正确'));
    await page.locator('#admin-token').fill('synthetic-admin-test-token'); await page.locator('#login-submit').click();
    await page.waitForFunction(() => document.querySelectorAll('#account-rows tr').length === 2);
    assert.deepEqual(posts, []);
    await page.locator('#email-search').fill('first'); await page.locator('#search-form button[type=submit]').click();
    await page.waitForFunction(() => document.querySelectorAll('#account-rows tr').length === 1);
    assert.equal(await page.locator('#account-rows tr').getAttribute('data-account-id'), '1');
    await page.locator('#reset-search').click();
    await page.waitForFunction(() => document.querySelectorAll('#account-rows tr').length === 2);
    await page.locator('tr[data-account-id="1"] .relogin').click();
    await page.waitForFunction(() => document.getElementById('task-state').textContent === '执行中');
    assert.equal(await page.locator('.relogin:disabled').count(), 2); assert.deepEqual(posts, [1]);
    await page.reload();
    await page.waitForFunction(() => document.getElementById('task-state').textContent === '执行中');
    assert.deepEqual(posts, [1], 'reload must not repeat login');
    assert.equal(await page.locator('.relogin:disabled').count(), 2);
    finished = true;
    await page.waitForFunction(() => document.getElementById('task-state').textContent === '已完成');
    await page.waitForFunction(() => document.querySelector('tr[data-account-id="1"]').textContent.includes('凭据已更新'));
    assert.equal(await page.locator('#account-rows tr').count(), 2);
    assert.match(await page.locator('tr[data-account-id="1"]').innerText(), /Token 已保存/);
    assert.match(await page.locator('tr[data-account-id="1"]').innerText(), /已保存密钥/);
    assert.equal(await page.locator('.relogin:disabled').count(), 0);
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    assert.deepEqual(browserErrors, []);
    fs.mkdirSync(path.join(__dirname, 'artifacts'), { recursive: true });
    await page.screenshot({ path: path.join(__dirname, 'artifacts/relogin-desktop.png'), fullPage: true });
    await page.locator('#logout').click();
    assert.equal(await page.locator('#login-panel').isVisible(), true);
    assert.deepEqual(posts, [1]);
    console.log('PASS desktop UI: authentication, search, selected account, busy state, reload recovery, same-row update, layout, logout');
  } finally { await browser.close(); await new Promise(resolve => server.close(resolve)); }
}
main().catch(err => { console.error(err); process.exitCode = 1; });

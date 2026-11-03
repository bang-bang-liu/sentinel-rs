// Optional browser smoke test. No Node runtime is needed by the deployed tool.
// OFFERS_TEST_PLAYWRIGHT points to an existing Playwright package.
// --mock uses synthetic accounts. Live mode reads admin.token from stdin.
const {chromium}=require(process.env.OFFERS_TEST_PLAYWRIGHT||'playwright');
const fs=require('node:fs');
const path=require('node:path');
const http=require('node:http');
const assert=require('node:assert/strict');
const mock=process.argv.includes('--mock');
let token=mock?'a'.repeat(64):fs.readFileSync(0,'utf8').trim();
const artifactDir=process.env.OFFERS_TEST_ARTIFACTS||path.join(require('node:os').tmpdir(),'sentinel-offers-ui');
fs.mkdirSync(artifactDir,{recursive:true});
const coupons=['plus-1-month-free','plus-1-month-50-pct-off','plus-2-months-50-pct-off','plus-3-months-50-pct-off'];
const epoch=1793595900;
let created=0;
let proxyWrites=0;
let server;
async function fixture(){
  const accounts=[['alpha@example.invalid',true,'eligible'],['beta@example.invalid',true,'not_eligible'],['pending@example.invalid',false,'unknown']].map(([email,has_token,result],i)=>({id:i+1,email,status:has_token?'registered':'pending_otp',has_token,free_trial:result,half_price:result,checked_at:epoch,latest_task_id:'11111111-1111-4111-8111-111111111111',last_confirmed:[],checks:coupons.map(coupon=>({coupon,execution_status:'finished',eligibility:result,reason:has_token?'confirmed':'missing_token',http_status:has_token?200:null,attempts:has_token?1:0,checked_at:epoch}))}));
  const task={task_id:'11111111-1111-4111-8111-111111111111',status:'completed',created_at:epoch,completed_accounts:3,total_accounts:3,completed_checks:12,total_checks:12,unknown_checks:4};
  let network={mode:'direct',display:null,source:'startup'};
  server=http.createServer(async(req,res)=>{
    res.setHeader('Cache-Control','no-store');
    res.setHeader('Content-Security-Policy',"default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'");
    const url=new URL(req.url,'http://localhost');
    if(url.pathname.startsWith('/api/')&&req.headers.authorization!==`Bearer ${token}`){res.writeHead(401,{'Content-Type':'application/json'});res.end(JSON.stringify({error:'authentication_required'}));return;}
    let data;
    if(url.pathname==='/api/settings/proxy'){
      if(req.method==='PUT'){
        let raw='';for await(const chunk of req)raw+=chunk;const body=JSON.parse(raw);
        if(body.mode==='proxy'){
          let proxy;try{proxy=new URL(body.proxy);}catch{}
          if(!proxy||!['http:','https:','socks5:','socks5h:'].includes(proxy.protocol)){res.statusCode=400;data={error:'invalid_proxy_url'};}
          else {network={mode:'proxy',display:`${proxy.protocol}//${proxy.username||proxy.password?'***@':''}${proxy.host}`,source:'dashboard'};proxyWrites++;}
        }else{network={mode:'direct',display:null,source:'dashboard'};proxyWrites++;}
      }
      data=data||network;
    }
    else if(url.pathname==='/api/tasks'&&req.method==='POST'){for await(const _ of req){}created++;res.statusCode=202;data={task_id:task.task_id};}
    else if(url.pathname==='/api/tasks')data=[task];
    else if(url.pathname.endsWith('/export')){res.setHeader('Content-Type','text/csv');res.end('email,coupon\nalpha@example.invalid,plus-1-month-free\n');return;}
    else if(url.pathname==='/api/accounts'){
      let items=accounts;const q=url.searchParams.get('q')||'',filter=url.searchParams.get('filter');
      items=items.filter(a=>a.email.includes(q));
      if(filter==='free')items=items.filter(a=>a.free_trial==='eligible');
      if(filter==='half')items=items.filter(a=>a.half_price==='eligible');
      if(filter==='missing_token')items=items.filter(a=>!a.has_token);
      if(filter==='unknown')items=items.filter(a=>a.free_trial==='unknown');
      data={items,total:items.length,page:1,page_size:50,summary:{total:3,missing_token:1,free_eligible:1,half_eligible:1,pending:1}};
    }else if(/^\/api\/accounts\/\d+\/offers$/.test(url.pathname))data=accounts.find(a=>a.id===Number(url.pathname.split('/')[3]));
    else{const name=url.pathname==='/'?'index.html':url.pathname.slice(1);if(!['index.html','app.js','styles.css'].includes(name)){res.statusCode=404;res.end();return;}res.setHeader('Content-Type',name.endsWith('.js')?'text/javascript':name.endsWith('.css')?'text/css':'text/html');res.end(fs.readFileSync(path.join(__dirname,'..',name)));return;}
    res.setHeader('Content-Type','application/json');res.end(JSON.stringify(data));
  });
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));return `http://127.0.0.1:${server.address().port}`;
}
(async()=>{
  const url=mock?await fixture():(process.env.OFFERS_TEST_URL||'http://127.0.0.1:8787');
  const browser=await chromium.launch({channel:'msedge',headless:true});
  try{
    const context=await browser.newContext({viewport:{width:1440,height:1100},acceptDownloads:true});const page=await context.newPage();const pageErrors=[];
    page.on('pageerror',error=>pageErrors.push(error.message));
    await page.goto(url);await page.locator('#admin-token').fill('wrong');await page.locator('#unlock').click();await page.locator('#login-error').filter({hasText:'无效'}).waitFor();
    await page.locator('#admin-token').fill(token);await page.locator('#unlock').click();await page.locator('#dashboard').waitFor({state:'visible'});await page.locator('#account-rows tr').first().waitFor();
    const total=Number(await page.locator('#stat-total').innerText());assert(total>0);assert.equal(await page.locator('#admin-token').inputValue(),'');
    await page.locator('#proxy-url').waitFor({state:'attached'});await page.waitForFunction(()=>!document.getElementById('save-proxy').disabled);
    if(mock){
      await page.locator('#proxy-settings summary').click();assert((await page.locator('#proxy-current').innerText()).includes('直连'));
      await page.locator('#proxy-url').fill('ftp://proxy.example.invalid:8080');await page.locator('#save-proxy').click();await page.locator('#proxy-feedback.is-error').filter({hasText:'格式无效'}).waitFor();assert.equal(proxyWrites,0);
      const secret='fixture-secret';const credentials=['fixture-user',secret].join(':');const authority=[credentials,'example.invalid:1080'].join('@');await page.locator('#proxy-url').fill(`socks5h://${authority}`);await page.locator('#save-proxy').click();await page.locator('#proxy-feedback').filter({hasText:'代理已保存'}).waitFor();assert.equal(proxyWrites,1);assert.equal(created,0);assert.equal(await page.locator('#proxy-url').inputValue(),'');assert(!(await page.locator('#proxy-current').innerText()).includes(secret));assert((await page.locator('#proxy-current').innerText()).includes('socks5h://***@example.invalid:1080'));
      await page.locator('#refresh').click();await page.waitForTimeout(300);assert((await page.locator('#proxy-current').innerText()).includes('已保存设置'));
      await page.locator('#direct-proxy').click();await page.locator('#proxy-feedback').filter({hasText:'已保存直连设置'}).waitFor();assert.equal(proxyWrites,2);assert((await page.locator('#proxy-current').innerText()).includes('直连'));assert.equal(created,0);
    }
    await page.screenshot({path:path.join(artifactDir,mock?'mock-desktop.png':'sg-desktop.png'),fullPage:true});
    await page.locator('[data-filter="missing_token"]').click();
    await page.waitForFunction(()=>{const rows=[...document.querySelectorAll('#account-rows tr')];return rows.length>0&&rows.every(r=>r.children[2].textContent==='缺少令牌');});
    await page.locator('#account-rows .text-button').first().click();await page.locator('#details').waitFor({state:'visible'});assert((await page.locator('#detail-content').innerText()).includes('缺'));await page.locator('#close-details').click();
    await page.locator('[data-filter="all"]').click();await page.waitForFunction(expected=>document.querySelectorAll('#account-rows tr').length===Math.min(expected,50),total);await page.locator('#select-page').check();assert((await page.locator('#selection-count').innerText()).includes(String(Math.min(total,50))));await page.locator('#clear-selection').click();assert(await page.locator('#check-selected').isDisabled());
    if(mock){await page.locator('#search').fill('beta@');await page.waitForTimeout(600);assert.equal(await page.locator('#account-rows tr').count(),1);await page.locator('#select-page').check();await page.locator('#check-selected').click();await page.waitForTimeout(400);assert.equal(created,1);await page.locator('#search').fill('');await page.waitForTimeout(600);}
    if(await page.locator('.task-export button').count()){const downloadPromise=page.waitForEvent('download');await page.locator('.task-export button').first().click();const download=await downloadPromise;await download.saveAs(path.join(artifactDir,mock?'mock-report.csv':'sg-report.csv'));assert.equal(await download.failure(),null);}
    assert.equal(await page.evaluate(()=>localStorage.length+sessionStorage.length),0);
    await page.locator('#logout').click();await page.locator('#login').waitFor({state:'visible'});assert.equal(await page.locator('#proxy-url').inputValue(),'');await page.reload();assert(await page.locator('#dashboard').isHidden());
    if(mock){await page.locator('#admin-token').fill(token);await page.locator('#unlock').click();await page.locator('#dashboard').waitFor({state:'visible'});await page.waitForFunction(()=>document.getElementById('proxy-current').textContent.includes('直连 · 已保存设置'));await page.locator('#logout').click();}
    assert.equal(pageErrors.length,0,pageErrors.join('\n'));
    console.log(JSON.stringify({mode:mock?'mock':'sg',total_accounts:total,checks:'login, auth failure, proxy settings, table, filtering, details, selection, export, desktop, lock, memory-only token',page_errors:pageErrors.length,artifacts:artifactDir}));
  }finally{await browser.close();if(server)await new Promise(resolve=>server.close(resolve));token='';}
})().catch(error=>{console.error(error.message);if(server)server.close();process.exitCode=1;});

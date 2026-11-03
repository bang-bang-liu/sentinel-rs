const $ = (id) => document.getElementById(id);
const couponNames = ['免费 1 个月', '半价 1 个月', '半价 2 个月', '半价 3 个月'];
const coupons = ['plus-1-month-free', 'plus-1-month-50-pct-off', 'plus-2-months-50-pct-off', 'plus-3-months-50-pct-off'];
const labels = {eligible:'符合', not_eligible:'不符合', unknown:'待确认'};
const reasons = {confirmed:'上游已明确返回资格结果', missing_token:'账号没有访问令牌', invalid_token_format:'访问令牌格式无效', account_missing:'源库中已找不到该账号', auth_required:'令牌失效或需要重新登录', access_denied:'请求被上游拒绝', rate_limited:'上游限流', deferred_rate_limit:'因上游限流，本次未继续查询', upstream_error:'上游服务异常', upstream_offline:'活动暂不可用', unexpected_response:'上游响应不在已知协议范围内', invalid_response:'响应格式不符合预期', network_error:'网络连接失败', timeout:'请求超时', interrupted:'任务中断，未取得结果', task_internal_error:'任务发生内部错误', process_interrupted:'服务重启时发现未完成任务'};
const errors = {authentication_required:'管理口令无效或已更换，请重新输入。', invalid_host:'访问地址不受支持，请使用本机 127.0.0.1 及服务对应端口。', cross_origin_denied:'请求来源不受支持。', task_busy:'已有检测任务在运行，请等待完成。', rate_limited:'上游限流冷却中，请稍后重试。', storage_unavailable:'暂时无法读取数据库，请稍后刷新。', no_accounts_selected:'请先选择账号。', account_not_found:'部分账号已不在源库中，请刷新后重新选择。', invalid_proxy_url:'代理格式无效，请填写 HTTP、HTTPS、SOCKS5 或 SOCKS5H 地址；不包含路径、查询参数或空格。', invalid_proxy_settings:'代理设置不完整，请重新填写。', invalid_proxy:'代理设置无效，请重新填写。', empty_proxy:'请填写代理地址。'};
const taskLabels = {queued:'排队中',running:'检测中',completed:'已完成',interrupted:'已中断',failed:'执行失败'};
let token='', page=1, pageSize=50, filter='all', query='', selected=new Set(), items=[], total=0, summaryTotal=0, busy=false, loading=false, refreshAgain=false, cooldown=0, timer, searchTimer;
let proxyLoaded=false, savingProxy=false, proxyRevision=0;
const time = (n) => n ? new Date(n*1000).toLocaleString('zh-CN',{hour12:false}) : '未检查';
function el(tag,text,cls){const node=document.createElement(tag);if(text!==undefined)node.textContent=text;if(cls)node.className=cls;return node;}
function notice(text){$('notice').textContent=text;$('notice').hidden=!text;}
function lock(){token='';clearTimeout(timer);selected.clear();proxyRevision++;proxyLoaded=false;$('proxy-url').value='';$('proxy-current').textContent='正在读取设置…';$('proxy-feedback').textContent='';$('proxy-settings').open=false;updateProxyActions();$('dashboard').hidden=true;$('login').hidden=false;$('logout').hidden=true;$('admin-token').value='';$('details').close();$('admin-token').focus();}
async function request(path,options={}){
  const response=await fetch(path,{...options,cache:'no-store',headers:{Authorization:`Bearer ${token}`,...(options.body?{'Content-Type':'application/json'}:{}),...options.headers}});
  if(!response.ok){let data={};try{data=await response.json();}catch{}if(response.status===401){lock();}throw new Error(errors[data.error]||`请求失败（${response.status}）`);}
  return response;
}
async function json(path,options){return (await request(path,options)).json();}
function updateProxyActions(){
  $('proxy-url').disabled=!proxyLoaded||savingProxy;$('save-proxy').disabled=!proxyLoaded||savingProxy;$('direct-proxy').disabled=!proxyLoaded||savingProxy;
  $('save-proxy').textContent=savingProxy?'正在保存…':'保存代理';$('proxy-form').setAttribute('aria-busy',String(savingProxy));
  updateActions();
}
function renderProxy(data){
  proxyLoaded=true;const source=data.source==='dashboard'?'已保存设置':'启动配置';
  $('proxy-current').textContent=data.mode==='proxy'?`代理 · ${data.display} · ${source}`:`直连 · ${source}`;updateProxyActions();
}
async function saveProxy(direct=false){
  if(savingProxy||!proxyLoaded)return;const proxy=$('proxy-url').value.trim();
  if(!direct&&!proxy){$('proxy-feedback').textContent='请填写代理地址。';$('proxy-feedback').classList.add('is-error');$('proxy-url').focus();return;}
  savingProxy=true;proxyRevision++;updateProxyActions();$('proxy-feedback').textContent='';$('proxy-feedback').classList.remove('is-error');
  try{const data=await json('/api/settings/proxy',{method:'PUT',body:JSON.stringify(direct?{mode:'direct'}:{mode:'proxy',proxy})});
    if(!token)return;proxyRevision++;renderProxy(data);$('proxy-url').value='';$('proxy-feedback').textContent=direct?'已保存直连设置，下次检测生效。':'代理已保存，下次检测生效。保存仅校验格式，不发起活动查询。';
  }catch(e){if(token){$('proxy-feedback').textContent=e.message;$('proxy-feedback').classList.add('is-error');}}
  finally{savingProxy=false;updateProxyActions();}
}
function tag(value,execution){const effective=execution&&execution!=='finished'?'unknown':value||'unknown';return el('span',execution==='running'?'检测中':execution==='pending'?'等待检测':labels[effective],`tag ${effective}`);}
function updateActions(){
  $('selection-count').textContent=selected.size?`已选择 ${selected.size} 个账号`:'未选择账号';$('clear-selection').hidden=!selected.size;
  $('check-selected').disabled=busy||savingProxy||!proxyLoaded||!selected.size||cooldown>Date.now()/1000;$('check-all').disabled=busy||savingProxy||!proxyLoaded||!summaryTotal||cooldown>Date.now()/1000;
  $('check-all').textContent=`全量检测（${summaryTotal}）`;
  const selectedHere=items.filter(a=>selected.has(a.id)).length;$('select-page').checked=items.length>0&&selectedHere===items.length;$('select-page').indeterminate=selectedHere>0&&selectedHere<items.length;$('select-page').disabled=!items.length;
}
function renderAccounts(data){
  items=data.items;total=data.total;summaryTotal=data.summary.total;cooldown=data.cooldown_until||0;
  for(const [id,key] of Object.entries({'stat-total':'total','stat-free':'free_eligible','stat-half':'half_eligible','stat-pending':'pending','stat-missing':'missing_token'}))$(id).textContent=data.summary[key];
  $('filtered-count').textContent=`${total} 个`;$('empty').hidden=items.length>0;
  const fragment=document.createDocumentFragment();
  for(const account of items){
    const row=el('tr');row.classList.toggle('selected',selected.has(account.id));
    const check=el('input');check.type='checkbox';check.checked=selected.has(account.id);check.setAttribute('aria-label',`选择 ${account.email}`);check.addEventListener('change',()=>{check.checked?selected.add(account.id):selected.delete(account.id);row.classList.toggle('selected',check.checked);updateActions();});const cell=el('td',undefined,'check-column');cell.append(check);row.append(cell);
    const identity=el('td');const email=el('div',account.email,'email');email.title=account.email;identity.append(email,el('span',`#${account.id} · ${account.status}`,'subtext'));row.append(identity);
    row.append(el('td',account.has_token?'已保存':'缺少令牌',account.has_token?'token-ok':'token-missing'));
    for(const coupon of coupons){const c=account.checks.find(c=>c.coupon===coupon);const td=el('td');td.append(tag(c?.eligibility,c?.execution_status));td.title=c?(reasons[c.reason]||c.execution_status):'尚未检测';row.append(td);}
    const when=el('td',time(account.checked_at),'time');row.append(when);
    const detail=el('td');const button=el('button','详情','text-button');button.setAttribute('aria-label',`查看 ${account.email} 的优惠详情`);button.addEventListener('click',()=>openDetail(account.id));detail.append(button);row.append(detail);fragment.append(row);
  }
  $('account-rows').replaceChildren(fragment);
  $('page-info').textContent=total?`显示 ${(page-1)*pageSize+1}–${Math.min(page*pageSize,total)}，共 ${total} 个`:'共 0 个账号';$('page-number').textContent=`${page} / ${Math.max(1,Math.ceil(total/pageSize))}`;$('prev-page').disabled=page<=1;$('next-page').disabled=page*pageSize>=total;
  if(cooldown>Date.now()/1000)notice(`上游限流，${time(cooldown)} 前暂停新检测。已取得的结果已保存。`);
  updateActions();
}
function renderTasks(tasks){
  const active=tasks.find(t=>['queued','running'].includes(t.status));busy=!!active;$('progress-panel').hidden=!active;
  if(active){$('progress-title').textContent='正在检测账号优惠';$('progress-count').textContent=`${active.completed_checks} / ${active.total_checks} 项`;$('task-progress').max=active.total_checks||1;$('task-progress').value=active.completed_checks;$('progress-detail').textContent=`${active.completed_accounts} / ${active.total_accounts} 个账号处理完成 · 任务 ${active.task_id.slice(0,8)}`;}
  const fragment=document.createDocumentFragment();
  for(const task of tasks){const row=el('div',undefined,'task-row');const state=el('div');state.append(el('span',taskLabels[task.status]||task.status,`tag ${['running','queued'].includes(task.status)?'neutral':task.unknown_checks||task.status!=='completed'?'unknown':'not_eligible'}`),el('code',task.task_id.slice(0,8)));const detail=el('div',time(task.created_at));detail.append(el('span',`${task.completed_accounts} / ${task.total_accounts} 个账号${task.stop_reason?' · '+(reasons[task.stop_reason]||'任务中断'):''}`,'subtext'));const count=el('div',`${task.completed_checks} / ${task.total_checks} 项`,'task-count');count.append(el('span',`待确认 ${task.unknown_checks} 项`,'subtext'));const downloads=el('div',undefined,'task-export');for(const format of ['csv','json']){const button=el('button',format.toUpperCase());button.setAttribute('aria-label',`导出任务 ${task.task_id.slice(0,8)} 为 ${format}`);button.addEventListener('click',()=>download(task.task_id,format,button));downloads.append(button);}row.append(state,detail,count,downloads);fragment.append(row);}
  $('task-list').replaceChildren(fragment);$('no-tasks').hidden=tasks.length>0;updateActions();
}
async function refresh(){
  if(!token)return;if(loading){refreshAgain=true;return;}loading=true;clearTimeout(timer);$('refresh').disabled=true;
  try{const revision=proxyRevision;const params=new URLSearchParams({q:query,filter,page:String(page),page_size:String(pageSize)});const [accounts,tasks,network]=await Promise.all([json(`/api/accounts?${params}`),json('/api/tasks'),json('/api/settings/proxy')]);if(!token)return;notice('');renderAccounts(accounts);renderTasks(tasks);if(revision===proxyRevision)renderProxy(network);$('connection-text').textContent='已连接';$('updated-at').textContent=`更新于 ${new Date().toLocaleTimeString('zh-CN',{hour12:false})}`;
  }catch(e){if(token){notice(e.message);$('connection-text').textContent='连接异常';}else $('login-error').textContent=e.message;
  }finally{loading=false;$('refresh').disabled=false;if(token){if(refreshAgain){refreshAgain=false;queueMicrotask(refresh);}else timer=setTimeout(refresh,busy?2000:15000);}}
}
async function startCheck(all){
  if(busy||savingProxy||!proxyLoaded)return;busy=true;updateActions();notice('');
  try{await json('/api/tasks',{method:'POST',body:JSON.stringify(all?{scope:'all'}:{scope:'selected',account_ids:[...selected]})});await refresh();}catch(e){notice(e.message);busy=false;updateActions();}
}
async function openDetail(id){
  try{const a=await json(`/api/accounts/${id}/offers`);const fragment=document.createDocumentFragment();fragment.append(el('div',a.email,'detail-email'),el('p',`注册记录状态：${a.status} · 凭据：${a.has_token?'已保存':'缺失'} · 最近检查：${time(a.checked_at)}`,'detail-meta'));
    coupons.forEach((coupon,index)=>{const c=a.checks.find(c=>c.coupon===coupon),old=a.last_confirmed.find(c=>c.coupon===coupon);const section=el('section',undefined,'detail-offer');const title=el('div',undefined,'detail-offer-title');title.append(el('strong',couponNames[index]),tag(c?.eligibility,c?.execution_status));section.append(title,el('p',coupon));section.append(el('p',c?`${reasons[c.reason]||(['running','pending'].includes(c.execution_status)?'等待本次检测结果':'尚未取得明确结果')} · 尝试 ${c.attempts} 次${c.http_status?` · HTTP ${c.http_status}`:''}`:'尚未发起查询。'));
      if(c?.checked_at)section.append(el('p',`本次时间：${time(c.checked_at)}`));if(old&&old.task_id!==a.latest_task_id)section.append(el('p',`上次明确结果：${labels[old.eligibility]} · ${time(old.checked_at)}。仅供历史参考，不代表当前资格。`,'history-line'));fragment.append(section);});$('detail-content').replaceChildren(fragment);$('details').showModal();
  }catch(e){notice(e.message);}
}
async function download(id,format,button){button.disabled=true;try{const response=await request(`/api/tasks/${id}/export?format=${format}`);const url=URL.createObjectURL(await response.blob());const a=el('a');a.href=url;a.download=`offers-${id}.${format}`;document.body.append(a);a.click();a.remove();setTimeout(()=>URL.revokeObjectURL(url),10000);}catch(e){notice(e.message);}finally{button.disabled=false;}}
$('login-form').addEventListener('submit',async e=>{e.preventDefault();$('login-error').textContent='';token=$('admin-token').value.trim();$('unlock').disabled=true;try{await json('/api/tasks');$('admin-token').value='';$('login').hidden=true;$('dashboard').hidden=false;$('logout').hidden=false;await refresh();}catch(e){token='';$('login-error').textContent=e.message;}finally{$('unlock').disabled=false;}});
$('logout').addEventListener('click',lock);$('refresh').addEventListener('click',refresh);$('check-selected').addEventListener('click',()=>startCheck(false));$('check-all').addEventListener('click',()=>startCheck(true));
$('proxy-form').addEventListener('submit',e=>{e.preventDefault();saveProxy();});$('direct-proxy').addEventListener('click',()=>saveProxy(true));
$('clear-selection').addEventListener('click',()=>{selected.clear();$('account-rows').querySelectorAll('input[type=checkbox]').forEach(c=>{c.checked=false;c.closest('tr').classList.remove('selected');});updateActions();});
$('select-page').addEventListener('change',e=>{for(const a of items)e.target.checked?selected.add(a.id):selected.delete(a.id);$('account-rows').querySelectorAll('input[type=checkbox]').forEach(c=>{c.checked=e.target.checked;c.closest('tr').classList.toggle('selected',c.checked);});updateActions();});
$('filter-tabs').addEventListener('click',e=>{const button=e.target.closest('[data-filter]');if(!button)return;filter=button.dataset.filter;page=1;for(const b of $('filter-tabs').children)b.setAttribute('aria-pressed',String(b===button));refresh();});
$('search').addEventListener('input',()=>{clearTimeout(searchTimer);searchTimer=setTimeout(()=>{query=$('search').value.trim();page=1;refresh();},250);});
$('prev-page').addEventListener('click',()=>{if(page>1){page--;refresh();}});$('next-page').addEventListener('click',()=>{page++;refresh();});$('page-size').addEventListener('change',()=>{pageSize=Number($('page-size').value);page=1;refresh();});$('close-details').addEventListener('click',()=>$('details').close());
$('timezone').textContent=`显示时区：${Intl.DateTimeFormat().resolvedOptions().timeZone}`;

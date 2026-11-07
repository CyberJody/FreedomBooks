async function finishRecovery(page) {
  await page.getByRole('button', {name:'Remind me later'}).click();
  await page.waitForURL(/\/settings\/book-protection\/$/);
  await Promise.all([page.waitForURL(/\/home\/$/), page.locator('a[href$="/home/"]').first().click()]);
}
'use strict';
// WAVE4 item 5: the plugin flows in the real app, fakes only (numeric loopback; no real network,
// sign-in or bank). Needs a fake-network native build (`FB_NET_FAKE=1 npm run build:native`) and the
// fake providers binary (`cargo build -p fb-app --example smoke_fakes`, path in FB_SMOKE_FAKES).
// Connect the fake SimpleFIN on the connect screen, map, pull to the review screen, accept a line,
// lock and reopen with the mask restored, then Sign in with ChatGPT against the fake OAuth issuer.
const {_electron:electron}=require('playwright');
const fs=require('node:fs');
const path=require('node:path');
const {spawn}=require('node:child_process');
const readline=require('node:readline');
const assert=require('node:assert/strict');

const PASSWORD='plugins-synthetic-lock';
// The fake bridge's synthetic credential (crates/fb-app/examples/smoke_fakes.rs).
const SECRET='SmokePassZ8vN3bW6yHc1Jd5Fs';

async function startFakes(){
  const bin=process.env.FB_SMOKE_FAKES;
  assert.ok(bin && fs.existsSync(bin),'FB_SMOKE_FAKES names the smoke_fakes example');
  const child=spawn(bin,[],{stdio:['pipe','pipe','inherit']});
  const line=await new Promise((resolve,reject)=>{
    readline.createInterface({input:child.stdout}).once('line',resolve);child.once('exit',()=>reject(new Error('fakes exited')));
  });
  return {child,info:JSON.parse(line)};
}

module.exports=async()=>{
  const root=path.resolve(__dirname,'..');
  const data=fs.mkdtempSync(path.join(process.env.TMPDIR,'fb-plugins-smoke-'));
  const shots=process.env.FB_SMOKE_SHOTS||path.join(root,'staging-run');
  let checks=0;const pass=text=>console.log(`PASS PLUGINS ${++checks}: ${text}`);
  const fakes=await startFakes();
  const env={...process.env,XDG_CONFIG_HOME:path.join(data,'config'),FB_DESKTOP_USER_DATA:data,
    FB_NET_FAKE_SIMPLEFIN_PORT:String(fakes.info.simplefin_port),FB_NET_FAKE_OPENAI:fakes.info.openai};
  const app=await electron.launch({ chromiumSandbox: true,args:[root],cwd:root,env,timeout:60000});
  try{
    const page=await app.firstWindow();await page.setViewportSize({width:1440,height:1000});page.setDefaultTimeout(180000);
    const errors=[];page.on('pageerror',e=>errors.push(e.message));
    const shot=name=>page.screenshot({path:path.join(shots,`plugins-${name}.png`)});
    const heading=()=>page.locator('main h1').first().innerText();
    await page.fill('#id_owner_name','Dana Whitfield');await page.fill('#id_company_name','Plugin Flow Supply');
    await page.selectOption('#id_starter_chart', 'empty');
    await page.check('#id_protect');await page.fill('#id_lock_password',PASSWORD);await page.fill('#id_lock_password2',PASSWORD);
    await Promise.all([page.waitForURL(/\/settings\/recovery-key\/$/),page.click('button:has-text("Create books")')]);
    await finishRecovery(page);
    const home=page.url();
    const company=/\/c\/([0-9a-f-]{36})\//.exec(home)[1];
    const go=p=>page.goto(`fb://app/c/${company}${p}`);
    for(const [code,name,type] of [['100','Checking','BANK'],['400','Service income','INCOME']]){
      await go('/accounts/new/');
      await page.fill('#id_code',code);await page.fill('#id_name',name);await page.selectOption('#id_detail_type',type);
      await Promise.all([page.waitForURL(/accounts\/$/),page.click('button:has-text("Save")')]);
    }
    const ids=await app.evaluate(({app:a},c)=>{
      const rows=JSON.parse(process.getBuiltinModule('module').createRequire(a.getAppPath()+'/package.json')('./native/fb-node.node').query(JSON.stringify({type:'Accounts',company_id:c})));
      return {bank:rows.find(r=>r.code==='100').id,income:rows.find(r=>r.code==='400').id};
    },company);
    pass('created an isolated encrypted book with a bank and an income account');

    // 1. Connect the fake SimpleFIN through the connect screen (the claim runs in fb-net).
    await go('/bank-feeds/');
    assert.equal(await heading(),'Bank feeds');
    await page.click('a:has-text("Connect a bank")');
    await page.fill('#id_label','Smoke bank');await page.fill('#id_setup_token',fakes.info.setup_token);
    await shot('connect');
    await Promise.all([page.waitForURL(/bank-feeds\/[0-9a-f-]+\/accounts\/$/),page.click('button:has-text("Connect bank")')]);
    pass('connect screen claimed the fake setup token and opened the new connection');
    // Map the downloaded account once a pull has listed it; the first pull lists accounts.
    await go('/bank-feeds/');
    await Promise.all([page.waitForLoadState('load'),page.click('button:has-text("Sync now")')]);
    await page.waitForSelector('.notice[role=alert]');
    const notice=await page.locator('.notice[role=alert]').innerText();
    assert.match(notice,/^Smoke bank: .*new transaction/,notice);
    assert.ok(!notice.includes(SECRET));
    await shot('synced');
    pass(`pull through fb-net stored the fake bank lines (${notice})`);
    await page.click('a:has-text("Smoke bank")');
    const select=page.locator('select[name^="map_"]').first();
    await select.selectOption(ids.bank);
    await Promise.all([page.waitForURL(/bank-feeds\/$/),page.click('button:has-text("Save mapping")')]);
    pass('mapped the downloaded account to the bank account');

    // 2. The review screen shows the pulled lines; accept one as a journal entry.
    await page.click('a:has-text("Review downloaded transactions")');
    assert.match(await page.locator('main').innerText(),/DEPOSIT MOBILE/);
    await shot('review');
    pass('the pull reached the review screen');
    const row=page.locator('tr',{hasText:'DEPOSIT MOBILE'}).first();
    await row.locator('select[name^="kind_"]').selectOption('JOURNAL');
    await row.locator('select[name^="account_"]').selectOption(ids.income);
    await Promise.all([page.waitForLoadState('load'),row.locator('button[name=accept_one]').click()]);
    await page.waitForFunction(()=>/recorded as JE/.test(document.body.innerText));
    await shot('accepted');
    pass('accepted a line: recorded as a journal entry');

    // 3. Lock and reopen: the connection's mask is restored before any command.
    await Promise.all([page.waitForURL('fb://app/'),app.evaluate(({Menu})=>{
      Menu.getApplicationMenu().items.find(i=>i.label==='File').submenu.items.find(i=>i.label==='Lock').click();
    })]);
    await page.fill('#id_password',PASSWORD);
    await Promise.all([page.waitForURL(home),page.click('button:has-text("Unlock")')]);
    await go('/bank-feeds/');
    const list=await page.locator('main').innerText();
    assert.match(list,/Connected/);assert.doesNotMatch(list,/Reconnect needed|couldn't be read/);
    await go('/edit/');
    await page.fill('#id_name',`Plugin Flow ${SECRET}`);
    await Promise.all([page.waitForLoadState('load'),page.click('button:has-text("Save")')]);
    const name=await app.evaluate(({app:a})=>JSON.parse(process.getBuiltinModule('module').createRequire(a.getAppPath()+'/package.json')('./native/fb-node.node').query(JSON.stringify({type:'Companies'})))[0].name);
    assert.ok(!name.includes(SECRET)&&name.includes('***'),name);
    pass('reopened the book: the connection is restored and its credential is masked on a later write');

    // 4. Sign in with ChatGPT against the fake OAuth issuer: system browser (captured) + fb-net loopback.
    await app.evaluate(({shell})=>{globalThis.__opened=[];shell.openExternal=async url=>{globalThis.__opened.push(url);};});
    await go('/ai/');
    const ai=await page.locator('main').innerText();
    assert.match(ai,/for powering your personal finance/i);assert.match(ai,/Continue with ChatGPT/);
    await shot('chatgpt');
    await Promise.all([page.waitForURL(/\/ai\/$/),page.click('button:has-text("Continue with ChatGPT")')]);
    const opened=await app.evaluate(()=>globalThis.__opened);
    assert.equal(opened.length,1);assert.ok(opened[0].startsWith(`${fakes.info.openai}/authorize?`),opened[0]);
    assert.ok(!opened[0].includes('id_token_hint'));
    pass('Continue with ChatGPT opened the issuer in the system browser, not the window');
    const approved=await fetch(opened[0],{redirect:'manual'});
    const callback=approved.headers.get('location');
    assert.ok(callback&&callback.startsWith('http://127.0.0.1:'),String(callback));
    const landed=await fetch(callback);assert.equal(landed.status,200);
    let connected='';
    for(let n=0;n<100&&!/Using ChatGPT plan/.test(connected);n++){await page.waitForTimeout(100);await go('/ai/');connected=await page.locator('main').innerText();}
    assert.match(connected,/Using ChatGPT plan/);assert.match(connected,/Manage usage/);
    await shot('chatgpt-connected');
    pass('the fake issuer callback finished the sign-in: Using ChatGPT plan');
    await Promise.all([page.waitForLoadState('load'),page.click('button:has-text("Disconnect")')]);
    await go('/ai/');
    assert.match(await page.locator('main').innerText(),/Continue with ChatGPT/);
    pass('Disconnect wiped the sign-in');
    assert.deepEqual(errors,[]);pass('no page errors under the production CSP');
    console.log(`SMOKE PLUGINS OK: ${checks} checks`);
  }finally{await app.close();fakes.child.stdin.end();fs.rmSync(data,{recursive:true,force:true});}
};
if(require.main===module)module.exports().catch(error=>{console.error(error.stack||error);process.exitCode=1;});

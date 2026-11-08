'use strict';
// Isolated Electron wiring smoke, using the established synthetic OS-store simulator.
// This verifies sealing/launcher wiring, not the availability of a real desktop keyring.
const {_electron:electron}=require('playwright');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const {spawn}=require('node:child_process');
const {createInterface}=require('node:readline');
const root=path.resolve(__dirname,'..');
const fixture=JSON.parse(fs.readFileSync(path.join(process.argv[2],'fixture.json')));
const nativePath=path.resolve(process.argv[3]);
const data=fs.mkdtempSync(path.join(process.env.TMPDIR,'fb-ai-desktop-'));
assert.equal(fixture.builtin,true,'generate this desktop fixture with --builtin to exercise identity-only lock');
fs.mkdirSync(path.join(data,'books'),{recursive:true});
const localBook=path.join(data,'books','ai-fixture.fbbook');
fs.copyFileSync(fixture.book,localBook);fs.copyFileSync(fixture.book+'.keys.json',localBook+'.keys.json');fixture.book=localBook;
const entry=path.join(data,'entry.cjs');
fs.writeFileSync(entry,`const {safeStorage}=require('electron');const c=require('node:crypto');
Object.assign(safeStorage,{isEncryptionAvailable:()=>true,getSelectedStorageBackend:()=> 'gnome_libsecret',encryptString:s=>{const n=c.randomBytes(12),x=c.createCipheriv('aes-256-gcm',Buffer.alloc(32,19),n);const b=Buffer.concat([x.update(s,'utf8'),x.final()]);return Buffer.concat([n,x.getAuthTag(),b]);},decryptString:b=>{const x=c.createDecipheriv('aes-256-gcm',Buffer.alloc(32,19),b.subarray(0,12));x.setAuthTag(b.subarray(12,28));return Buffer.concat([x.update(b.subarray(28)),x.final()]).toString('utf8');}});require(${JSON.stringify(path.join(root,'main/entry.cjs'))});`,{mode:0o600});
const env={...process.env,FB_DESKTOP_USER_DATA:data,FB_NATIVE_MODULE:nativePath,ELECTRON_ENABLE_LOGGING:'0'};
let app,child;
(async()=>{
  assert.ok(process.env.DISPLAY && ![':0',':0.0',':20',':20.0'].includes(process.env.DISPLAY),'isolated Xvfb display required');
  app=await electron.launch({chromiumSandbox:true,args:[entry],cwd:root,env,timeout:120000});
  const page=await app.firstWindow();
  await app.evaluate(({app:a},input)=>{
    const req=process.getBuiltinModule('module').createRequire(input.nativePath);
    req(input.nativePath).openBook(input.book,input.key);
  },{nativePath,book:fixture.book,key:fixture.key_hex});
  await page.goto(`fb://app/c/${fixture.company_id}/home/`);
  await page.waitForURL(/\/desktop\/sign-in\/$/);
  await page.fill('#id_username',fixture.username);await page.fill('#id_password',fixture.user_password);
  await page.getByRole('button',{name:'Sign in',exact:true}).click();
  await page.waitForURL(url=>url.pathname!=='/desktop/sign-in/');
  await page.goto(`fb://app/c/${fixture.company_id}/home/`);
  assert.match(await page.locator('main').innerText(),/Synthetic local AI/);
  assert.equal(await app.evaluate((_e,p)=>JSON.parse(process.mainModule.require(p).appState()).can_lock,nativePath),false);
  const prefs=await app.evaluate(({BrowserWindow})=>{const p=BrowserWindow.getAllWindows()[0].webContents.getLastWebPreferences();return {sandbox:p.sandbox,contextIsolation:p.contextIsolation,nodeIntegration:p.nodeIntegration};});
  assert.deepEqual(prefs,{sandbox:true,contextIsolation:true,nodeIntegration:false});
  const register=()=>app.evaluate((_e,input)=>JSON.parse(process.mainModule.require(input.nativePath).query(JSON.stringify({type:'Register',company_id:input.company,account_id:input.bank}))),{nativePath,company:fixture.company_id,bank:fixture.bank});
  const before=await register();
  await page.getByRole('link',{name:'Review transactions',exact:true}).click();
  await page.waitForSelector('#review-tools:not([hidden])');
  const row=page.locator('[data-review-row]').first();
  await row.locator('[name=sel]').focus();await page.keyboard.press('Space');
  await page.selectOption('#bulk-kind','JOURNAL');await page.selectOption('#bulk-category',fixture.expense);
  await page.check('#bulk-set-payee');await page.fill('#bulk-payee','Synthetic desktop reviewed vendor');
  await page.click('#bulk-apply');
  assert.deepEqual(await register(),before,'bulk draft changes must not post');
  await page.getByRole('button',{name:'Accept selected',exact:true}).click();
  await page.waitForFunction(()=>document.querySelector('.results')?.textContent.includes('recorded as'));
  assert.notEqual((await register()).closing,before.closing);
  if(process.env.FB_SMOKE_SHOTS){fs.mkdirSync(process.env.FB_SMOKE_SHOTS,{recursive:true});await page.screenshot({path:path.join(process.env.FB_SMOKE_SHOTS,'combined-review-accepted.png')});}
  console.log('PASS: signed-in desktop keyboard/bulk review posts through the real preload and native accounting boundary only on Accept');
  await page.goto(`fb://app/c/${fixture.company_id}/home/`);
  await app.evaluate(async({Menu,dialog})=>{
    let connected;
    const finished=new Promise(resolve=>connected=resolve);
    dialog.showMessageBox=async(_window,options)=>{
      if(options.title==='Connect a local AI')return {response:2};
      if(options.title==='Local AI connected'){connected();return {response:1};}
      return {response:0};
    };
    Menu.getApplicationMenu().items.find(i=>i.label==='Settings').submenu.items.find(i=>i.label==='Connect local AI…').click();
    await finished;
  });
  const config=await app.evaluate(async({clipboard})=>JSON.parse(await clipboard.readText()));
  assert.ok(config.mcpServers.freedombooks.args.includes('--ai-mcp'));
  const sealed=config.mcpServers.freedombooks.args.at(-1);
  assert.equal(fs.statSync(sealed).mode&0o777,0o600);
  console.log('PASS: real desktop menu creates a private sealed grant and secret-free MCP launcher configuration');
  child=spawn(config.mcpServers.freedombooks.command,[entry,'--ai-mcp',sealed],{env,stdio:['pipe','pipe','pipe']});
  let seq=0;const pending=new Map();let stderr='';child.stderr.on('data',c=>stderr+=c);
  createInterface({input:child.stdout}).on('line',line=>{try{const v=JSON.parse(line);pending.get(v.id)?.(v);pending.delete(v.id);}catch{}});
  const rpc=(method,params)=>new Promise((resolve,reject)=>{const id=++seq;const deadline=setTimeout(()=>{pending.delete(id);reject(new Error('Synthetic MCP request timeout'));},20000);pending.set(id,v=>{clearTimeout(deadline);resolve(v);});child.stdin.write(JSON.stringify({jsonrpc:'2.0',id,method,params})+'\n');});
  const timer=setTimeout(()=>{console.error('Synthetic sealed launcher timeout');child.kill();},30000);
  try {
    const initialized=await rpc('initialize',{protocolVersion:'2025-06-18'});assert.equal(initialized.result.serverInfo.name,'freedombooks-local');
    const tools=await rpc('tools/list',{});assert.equal(tools.result.tools.length,91);
    const result=await rpc('tools/call',{name:'query_Accounts',arguments:{}});assert.equal(result.result.isError,false);
    console.log('PASS: a separate Electron MCP utility unseals the grant and calls the running desktop without a second book opener');
    // Hold the real native result after it completes but before main's exclusive finalizer.
    // This deterministically reproduces the deferred identity-only-lock schedule.
    await app.evaluate((_e,nativePath)=>{
      const n=process.mainModule.require(nativePath),original=n.aiRequestAsync;
      const hold={};hold.ready=new Promise(resolve=>hold.markReady=resolve);hold.gate=new Promise(resolve=>hold.release=resolve);globalThis.fbSyntheticHold=hold;
      n.aiRequestAsync=(...args)=>{n.aiRequestAsync=original;return original(...args).then(value=>{hold.markReady();return hold.gate.then(()=>value);});};
    },nativePath);
    const held=rpc('tools/call',{name:'query_Accounts',arguments:{}});
    await app.evaluate(()=>globalThis.fbSyntheticHold.ready);
    await app.evaluate(({powerMonitor})=>powerMonitor.emit('lock-screen'));
    await app.evaluate(()=>globalThis.fbSyntheticHold.release());
    assert.equal((await held).result.isError,true,'old native output must not escape after deferred sign-out');
    await page.waitForURL(/\/desktop\/sign-in\/$/);
    assert.equal(await app.evaluate((_e,p)=>JSON.parse(process.mainModule.require(p).appState()).signed_in,nativePath),false);
    await page.fill('#id_username',fixture.username);await page.fill('#id_password',fixture.user_password);await page.getByRole('button',{name:'Sign in',exact:true}).click();await page.waitForURL(url=>url.pathname!=='/desktop/sign-in/');
    await page.goto(`fb://app/c/${fixture.company_id}/home/`);
    const locked=await rpc('tools/call',{name:'query_Accounts',arguments:{}});assert.equal(locked.result.isError,true);
    console.log('PASS: busy-worker OS lock suppresses real MCP results and old credentials stay revoked after same-user sign-in');
  } finally {clearTimeout(timer);}
})().catch(error=>{console.error(error);process.exitCode=1;}).finally(async()=>{if(child)child.kill();if(app)await app.close();});

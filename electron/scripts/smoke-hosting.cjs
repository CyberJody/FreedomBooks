// "Host these books", end to end in two real isolated Electron apps on one isolated Xvfb display:
// the host app turns hosting on from Settings › Host these books… (127.0.0.1 only, development
// override FB_HOSTING_LOOPBACK=1), pairs a computer with a one-time code, and a second app (another
// profile) pairs, signs in on the host as a bookkeeper and saves work into the host's book while
// the host's own person works too. Then the host locks (only they are signed out), revokes the
// computer and stops hosting. Synthetic book, names and passwords only; the keyrings are synthetic
// test wrappers (as smoke-keys and smoke-remote do).
//
//   FB_REMOTE_HELPER=<electron/native/fb-remote> node scripts/smoke-hosting.cjs
// Also run as the second part of smoke-remote.
'use strict';
const {_electron: electron} = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const net = require('node:net');
const path = require('node:path');
const {spawn} = require('node:child_process');
const {createHmac, randomUUID} = require('node:crypto');
const {once} = require('node:events');
const {createInterface} = require('node:readline');
const {setTimeout: delay} = require('node:timers/promises');
const {seedConnectionKey} = require('./ai-fixture-key.cjs');
const root = path.resolve(__dirname, '..');
const KEEPER = 'Synthetic hosted bookkeeper';
const KEEPER_PASSWORD = 'Synthetic hosted keeper password';

function mfaCode(secret) {
  const bits=[...secret].map(letter=>'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567'.indexOf(letter).toString(2).padStart(5,'0')).join('');
  const key=Buffer.from(bits.match(/.{8}/g).map(byte=>parseInt(byte,2)));
  const counter=Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(Math.floor(Date.now()/30000)));
  const mac=createHmac('sha1',key).update(counter).digest();
  return String((mac.readUInt32BE(mac[19]&15)&0x7fffffff)%1000000).padStart(6,'0');
}

function keyringEntry(dir, seed) {
  const entry = path.join(dir, 'test-keyring.cjs');
  // This wrapper runs only in the test, with an intentionally synthetic keyring key.
  fs.writeFileSync(entry, `const {safeStorage}=require('electron');const c=require('node:crypto');\nObject.assign(safeStorage,{isEncryptionAvailable:()=>true,getSelectedStorageBackend:()=> 'gnome_libsecret',encryptString:s=>{const n=c.randomBytes(12),x=c.createCipheriv('aes-256-gcm',Buffer.alloc(32,${seed}),n);const b=Buffer.concat([x.update(s,'utf8'),x.final()]);return Buffer.concat([n,x.getAuthTag(),b]);},decryptString:b=>{const x=c.createDecipheriv('aes-256-gcm',Buffer.alloc(32,${seed}),b.subarray(0,12));x.setAuthTag(b.subarray(12,28));return Buffer.concat([x.update(b.subarray(28)),x.final()]).toString('utf8');}});require(${JSON.stringify(path.join(root, 'main/entry.cjs'))});\n`, {mode: 0o600});
  return entry;
}

async function freePort() {
  const probe = net.createServer();
  await new Promise((resolve, reject) => { probe.once('error', reject); probe.listen(0, '127.0.0.1', resolve); });
  const port = probe.address().port;
  await new Promise(resolve => probe.close(resolve));
  return port;
}

// One isolated app: its own HOME, XDG folders and FreedomBooks profile.
async function open(base, name, seed, helper, extra = {}) {
  const home = path.join(base, name, 'home'), data = path.join(base, name, 'data');
  fs.mkdirSync(home, {recursive: true});
  const entry = keyringEntry(home, seed);
  const env = {...process.env, HOME: home, XDG_CONFIG_HOME: path.join(home, 'config'), XDG_DATA_HOME: path.join(home, 'share'), XDG_CACHE_HOME: path.join(home, 'cache'), FB_DESKTOP_USER_DATA: data, FB_REMOTE_HELPER: helper, ELECTRON_ENABLE_LOGGING: '0', ...extra};
  const app = await electron.launch({args: [entry, '--password-store=basic'], cwd: root, env, timeout: 180000});
  await app.evaluate(({dialog}) => { dialog.showErrorBox = (_t, m) => { throw new Error(m); }; });
  const page = await app.firstWindow();
  page.setDefaultTimeout(45000);
  await page.setViewportSize({width: 1440, height: 1000});
  await page.waitForSelector('h1');
  const menu = id => app.evaluate(({Menu}, id) => Menu.getApplicationMenu().getMenuItemById(id).click(), id);
  const at = async (re, timeout = 30000) => {
    const end = Date.now() + timeout;
    while (Date.now() < end) {
      if (re.test(new URL(page.url()).pathname)) { await page.waitForLoadState('load').catch(() => {}); if (re.test(new URL(page.url()).pathname)) return; }
      await new Promise(r => setTimeout(r, 100));
    }
    throw new Error(`${name}: timed out waiting for ${re}; at ${page.url()}`);
  };
  return {app, page, menu, at, data, entry, env};
}

async function aiLauncher(host, file) {
  const data=path.join(path.dirname(host.data),'agent-profile');
  // The service environment can differ from the host desktop's XDG data directory.
  const env={...host.env,FB_DESKTOP_USER_DATA:data,XDG_DATA_HOME:path.join(data,'share')};
  const child=spawn(require('electron'),[host.entry,'--ai-api',file,'--password-store=basic'],{cwd:root,env,stdio:['pipe','pipe','ignore']});
  const exited=once(child,'exit');
  const lines=createInterface({input:child.stdout});
  return {
    call:async request=>{
      const signal=AbortSignal.timeout(20000);
      const reply=once(lines,'line',{signal});
      child.stdin.write(JSON.stringify(request)+'\n');
      return JSON.parse((await reply)[0]);
    },
    close:async()=>{
      child.stdin.end();
      // The agent waits for its last response before it exits on EOF.
      const done=await Promise.race([exited.then(()=>true),delay(5000,false,{ref:false})]);
      if(!done){child.kill();await exited;}
    },
  };
}

async function dialogReplies(host, replies) {
  await host.app.evaluate(({dialog}, replies)=>{
    dialog.showMessageBox=async(_window,options)=>{
      const key=options.title ?? options.message;
      const reply=replies[key];
      if(reply===undefined)throw Error('Unexpected dialog '+key);
      if(options.title==='Connect an AI assistant') {
        if(options.checkboxChecked!==true || options.defaultId!==0 || !options.checkboxLabel.includes('while FreedomBooks is locked'))throw Error('Locked access must be offered ticked with Cancel default to an Administrator');
      }
      return {response:reply,checkboxChecked:false};
    };
  }, replies);
}

async function until(check) {
  const deadline=Date.now()+10000;
  while(!check()){
    if(Date.now()>=deadline)throw Error('AI settings did not publish the expected change');
    await delay(50);
  }
}

async function run({helper = process.env.FB_REMOTE_HELPER, shots: shotsDir, label = 'HOSTING'} = {}) {
  assert.ok(process.env.DISPLAY && ![':0', ':20', ':0.0', ':20.0'].includes(process.env.DISPLAY), 'isolated Xvfb display required');
  assert.ok(helper && fs.existsSync(helper), 'FB_REMOTE_HELPER must name a built fb-remote');
  const base = fs.mkdtempSync(path.join(process.env.TMPDIR, 'fb-hosting-smoke-'));
  const fixture = process.env.FB_HOSTING_FIXTURE && JSON.parse(fs.readFileSync(path.join(process.env.FB_HOSTING_FIXTURE, 'fixture.json')));
  const OWNER = fixture?.username || 'Synthetic host owner';
  const OWNER_PASSWORD = fixture?.user_password || 'Synthetic host owner password';
  if (fixture) {
    assert.equal(fixture.builtin, true);
    const book = path.join(base, 'host', 'data', 'books', 'default.fbbook');
    fs.mkdirSync(path.dirname(book), {recursive: true});
    fs.copyFileSync(fixture.book, book);
    fs.copyFileSync(fixture.book + '.keys.json', book + '.keys.json');
    seedConnectionKey(path.join(base, 'host', 'data'), 31, fixture);
  }
  const shots = shotsDir || process.env.FB_SMOKE_SHOTS || path.join(base, 'screenshots');
  fs.mkdirSync(shots, {recursive: true});
  let checks = 0;
  const pass = s => console.log(`PASS ${label} ${++checks}: ${s}`);
  let host, client, agent;
  try {
    // --- The host computer: first run, the owner, and a bookkeeper. ---
    host = await open(base, 'host', 31, helper, {FB_HOSTING_LOOPBACK: '1'});
    let h = host.page;
    const title = () => host.app.evaluate(({BrowserWindow}) => BrowserWindow.getAllWindows()[0].getTitle());
    const titled = async (re, timeout = 15000) => {
      const end = Date.now() + timeout;
      let now = '';
      while (Date.now() < end) { now = await title(); if (re.test(now)) return now; await new Promise(r => setTimeout(r, 250)); }
      throw new Error(`host window title ${JSON.stringify(now)} does not match ${re}`);
    };
    const listening = port => new Promise(resolve => {
      const s = net.connect({host: '127.0.0.1', port}, () => { s.destroy(); resolve(true); });
      s.on('error', () => resolve(false));
    });
    const hshot = n => h.screenshot({path: path.join(shots, `hosting-${n}.png`)});
    const openCompany = async () => {
      if (fixture) {
        await h.waitForURL(url => url.pathname !== '/desktop/sign-in/');
        await h.goto(`fb://app/c/${fixture.company_id}/home/`);
      }
      await host.at(/\/home\/$/);
    };
    const hostSignIn = async (name, password, recovery) => {
      await host.at(/\/desktop\/sign-in\/$/);
      if (await h.locator('#id_username').count()) await h.fill('#id_username', name);
      await h.fill('#id_password', password);
      await h.getByRole('button', {name: 'Sign in', exact: true}).click();
      if(recovery){
        await h.fill('#id_code',recovery);
        await h.getByRole('button',{name:'Verify and sign in',exact:true}).click();
      }
      await openCompany();
    };
    if (fixture) {
      await hostSignIn(OWNER, OWNER_PASSWORD);
    } else {
      await h.fill('#id_owner_name', OWNER);
      await h.fill('#id_company_name', 'Synthetic hosted company');
      await h.selectOption('#id_starter_chart', 'empty');
      await h.getByRole('button', {name: 'Create books', exact: true}).click();
      await h.waitForURL(/\/desktop\/sign-in\/$/);
      await h.fill('#id_password', OWNER_PASSWORD);
      await h.fill('#id_password_repeat', OWNER_PASSWORD);
      if (await h.locator('input[name="confirm_owner"]').count()) await h.check('input[name="confirm_owner"]');
      await h.getByRole('button', {name: 'Set password and continue', exact: true}).click();
      await host.at(/\/(home|settings\/recovery-key)\/$/);
      if (h.url().endsWith('/settings/recovery-key/')) {
        const later = h.getByRole('button', {name: 'Remind me later', exact: true});
        if (await later.count()) await later.click();
      }
      await host.at(/\/home\/$/);
    }
    const company = /\/c\/([0-9a-f-]{36})\//.exec(h.url())[1];
    await h.click('nav.appnav a:has-text("Settings")');
    await h.locator('main').getByRole('link', {name: 'Users', exact: true}).click();
    await h.waitForSelector('#id_role');
    await h.fill('#id_username', KEEPER);
    await h.selectOption('#id_role', 'bookkeeper');
    await h.check(`input[name="company"][value="${company}"]`);
    await h.fill('#id_password', KEEPER_PASSWORD);
    await h.fill('#id_password_repeat', KEEPER_PASSWORD);
    await h.getByRole('button', {name: 'Save user and sign out', exact: true}).click();
    await hostSignIn(OWNER, OWNER_PASSWORD);
    pass('the host computer has synthetic books, its owner and a bookkeeper');

    const catalogue={catalogue:true};
    await dialogReplies(host,{'Connect an AI assistant':2,'AI assistant connected':0});
    await host.menu('ai-connect');
    const approvals=path.join(host.data,'ai-connections');
    const savedFiles=()=>fs.existsSync(approvals)?fs.readdirSync(approvals).filter(name=>/^[0-9a-f-]{36}\.sealed$/.test(name)):[];
    await until(()=>savedFiles().length===1);
    const file=path.join(approvals,savedFiles()[0]);
    agent=await aiLauncher(host,file);
    assert.equal((await agent.call(catalogue)).data.unattended,false);
    await host.menu('lock-books');
    await host.at(/^\/desktop\/sign-in\/$/);
    assert.equal((await agent.call(catalogue)).code,'paused_signed_out');
    await hostSignIn(OWNER,OWNER_PASSWORD);
    assert.equal((await agent.call(catalogue)).data.unattended,false);
    const old=fs.readFileSync(file);
    await host.app.close();
    assert.equal((await agent.call(catalogue)).code,'app_not_running');
    host=await open(base,'host',31,helper,{FB_HOSTING_LOOPBACK:'1'});
    h=host.page;
    await host.at(/^\/desktop\/sign-in\/$/);
    assert.equal((await agent.call(catalogue)).code,'paused_signed_out');
    await hostSignIn(OWNER,OWNER_PASSWORD);
    assert.equal((await agent.call(catalogue)).data.unattended,false);
    assert.deepEqual(savedFiles(),[path.basename(file)]);
    assert.ok(!fs.readFileSync(file).equals(old));
    pass('the same attended launcher resumes after standalone lock, sign-out and restart');

    const attended=fs.readFileSync(file);
    await dialogReplies(host,{'AI assistant connections':2,'Change AI locked access':1});
    await host.menu('ai-manage');
    await until(()=>!fs.readFileSync(file).equals(attended));
    assert.deepEqual(savedFiles(),[path.basename(file)]);
    const epoch = path.join(approvals, '.approval-epoch.sealed');
    const anchors = path.join(approvals, 'unattended');
    const anchorNames = fs.readdirSync(anchors);
    assert.equal(anchorNames.length, 1);
    const anchor = path.join(anchors, anchorNames[0]);
    const replay = [file, epoch, anchor].map(name => [name, fs.readFileSync(name)]);
    await host.menu('lock-books');
    await host.at(/^\/desktop\/sign-in\/$/);
    assert.equal((await agent.call(catalogue)).data.unattended,true);
    const saveThroughAI=display_name=>agent.call({operation:'command_SaveCustomer',arguments:{display_name}});
    assert.equal((await saveThroughAI('Synthetic standalone locked AI customer')).ok,true);
    if(fixture){
      const write=await agent.call({operation:'command_SaveAccount',arguments:{code:'601',name:'Synthetic hosted '+fixture.bank_secret,detail_type:'EXPENSE'}});
      assert.equal(write.ok,true);
      const accounts=await agent.call({operation:'query_Accounts',arguments:{}});
      assert.ok(JSON.stringify(accounts.data).includes('Synthetic hosted ***'));
    }
    assert.equal(new URL(h.url()).pathname,'/desktop/sign-in/');
    await hostSignIn(OWNER,OWNER_PASSWORD);
    pass('Manage enables locked reads and writes off-hosting with the original approval file');
    if(fixture){
      const existing=new Set(fs.readdirSync(anchors));
      await host.app.evaluate((_electron,{company,epoch,nativePath})=>{
        const native=process.mainModule.require(nativePath);
        const grant=JSON.parse(native.aiGrant(company,'Synthetic stale approval','read_only'));
        native.aiSetUnattended(grant.id,epoch,true);
        native.aiRevoke(grant.id);
      },{company,epoch:randomUUID(),nativePath:process.env.FB_NATIVE_MODULE || path.join(root,'native','fb-node.node')});
      const missing=fs.readdirSync(anchors).filter(name=>!existing.has(name));
      assert.equal(missing.length,1);
      fs.unlinkSync(path.join(anchors,missing[0]));
    }

    // --- Settings › Host these books…: off by default, then on for one address. ---
    await host.menu('host-books');
    await host.at(/^\/hosting\/$/);
    const off = await h.locator('main').innerText();
    assert.match(off, /Hosting is off/);
    assert.equal(await h.locator('input[name=address][value="0.0.0.0"]').count(), 0, 'never all interfaces');
    assert.equal(await h.locator('input[name=address][value="127.0.0.1"]').count(), 1);
    await hshot('01-off');
    pass('Host these books is off by default and offers only this computer’s own private addresses');
    const port = await freePort();
    await h.check('input[name=address][value="127.0.0.1"]');
    await h.fill('input[name=port]', String(port));
    await h.getByRole('button', {name: 'Turn on hosting', exact: true}).click();
    await h.waitForSelector('#fb-hosting-addr');
    const addr = (await h.locator('#fb-hosting-addr').innerText()).trim();
    assert.equal(addr, `127.0.0.1:${port}`);
    assert.match(await h.locator('main').innerText(), /Hosting is on/);
    await hshot('02-on');
    pass('an Administrator turns hosting on; the screen shows the address to give the other computers');
    await h.getByRole('button', {name: 'Pair a computer…', exact: true}).click();
    await h.waitForSelector('#fb-hosting-code');
    const code = (await h.locator('#fb-hosting-code').innerText()).trim();
    assert.match(code, /^[0-9A-Z]{4}-[0-9A-Z]{4}-[0-9A-Z]{4}-[0-9A-Z]{4}$/);
    await hshot('03-code');
    pass('Pair a computer shows a one-time code');

    // --- The second computer pairs, signs in on the host and saves into its book. ---
    client = await open(base, 'client', 37, helper);
    const c = client.page;
    const cshot = n => c.screenshot({path: path.join(shots, `hosting-client-${n}.png`)});
    assert.ok(await c.locator('#id_owner_name').count(), 'the second computer has no books');
    await client.menu('remote-connect');
    await client.at(/^\/remote\/connect\/$/);
    await c.fill('input[name=addr]', addr);
    await c.fill('input[name=code]', code);
    await c.fill('input[name=device_name]', 'Synthetic laptop');
    await c.getByRole('button', {name: 'Pair and connect', exact: true}).click();
    await client.at(/^\/desktop\/sign-in\/$/);
    assert.match(await c.locator('#fb-remote-bar').innerText(), /Connected to/);
    await c.fill('#id_username', KEEPER);
    await c.fill('#id_password', KEEPER_PASSWORD);
    await c.getByRole('button', {name: 'Sign in', exact: true}).click();
    await client.at(/\/home\/$/);
    assert.match(await c.locator('.operator').innerText(), new RegExp(KEEPER));
    await cshot('01-signed-in');
    pass('the second computer pairs with the code and its bookkeeper signs in on the host');
    if(fixture)pass('client sign-in succeeds with a missing AI anchor and a readable bank sign-in on the host');

    const addCustomer = async (page, at, name) => {
      await page.goto(`fb://app/c/${company}/lists/customers/new/`);
      await page.fill('#id_display_name', name);
      await page.getByRole('button', {name: /^Save/}).first().click();
      await page.waitForURL(url => !String(url).endsWith('/new/'));
      await page.waitForLoadState('load');
    };
    await addCustomer(c, client.at, 'Laptop customer');
    await addCustomer(h, host.at, 'Host desk customer');
    for (const [page, who] of [[h, 'host'], [c, 'client']]) {
      await page.goto(`fb://app/c/${company}/lists/customers/`);
      const text = await page.locator('main').innerText();
      assert.match(text, /Laptop customer/, who);
      assert.match(text, /Host desk customer/, who);
    }
    await cshot('02-both-customers');
    pass('both people work on one book at the same time: each sees the other’s customer');
    await h.goto(`fb://app/c/${company}/history/`);
    const history = await h.locator('main').innerText();
    assert.match(history, new RegExp(KEEPER), 'the book’s audit names the remote bookkeeper');
    assert.match(history, new RegExp(OWNER), 'and the host’s owner');
    pass('the book’s own history records each change under its own person');

    await host.menu('host-books');
    await host.at(/^\/hosting\/$/);
    const listed = await h.locator('table.fb-hosting-devices').innerText();
    assert.match(listed, /Synthetic laptop/);
    assert.match(listed, /Connected now/);
    await hshot('04-paired-list');
    await titled(/Hosting on — 1 computer connected$/);
    await h.goto(`fb://app/c/${company}/home/`);
    await titled(/Hosting on — 1 computer connected$/);
    pass('the host lists the paired computer as connected, and the window title says so on every screen');

    // --- The host person's lock signs out only them. ---
    await host.menu('lock-books');
    await host.at(/^\/desktop\/sign-in\/$/);
    await c.goto(`fb://app/c/${company}/lists/customers/`);
    assert.match(await c.locator('main').innerText(), /Laptop customer/, 'the laptop keeps working');
    assert.equal((await agent.call(catalogue)).data.unattended,true);
    assert.equal((await saveThroughAI('Synthetic hosted locked AI customer')).ok,true);
    await c.reload();
    assert.match(await c.locator('main').innerText(),/Synthetic hosted locked AI customer/);
    assert.equal(new URL(h.url()).pathname,'/desktop/sign-in/');
    const clientAi=await client.app.evaluate(({Menu})=>['ai-connect','ai-manage'].map(id=>Menu.getApplicationMenu().getMenuItemById(id).enabled));
    assert.deepEqual(clientAi,[false,false]);
    pass('unattended reads and writes work while the host is signed out and the client is signed in');
    await hostSignIn(OWNER, OWNER_PASSWORD);
    pass('locking the host signs out only the host’s person; the connected computer keeps working');

    // --- Revoke: the computer is cut off at once. ---
    await host.menu('host-books');
    await host.at(/^\/hosting\/$/);
    await dialogReplies(host, {'Revoke Synthetic laptop?': 1});
    await h.locator('tr', {hasText: 'Synthetic laptop'}).getByRole('button', {name: 'Revoke', exact: true}).click();
    await host.at(/^\/hosting\/$/);
    await h.waitForSelector('tr:has-text("Synthetic laptop"):has-text("Revoked")');
    assert.doesNotMatch(await h.locator('main').innerText(), /Not saved/, 'saved in the books and by the listener');
    await hshot('05-revoked');
    await c.goto(`fb://app/c/${company}/home/`);
    assert.match(await c.locator('main').innerText(), /did not accept this computer|not reachable|ended/i);
    await cshot('03-revoked');
    pass('revoking the computer cuts it off at once and the host shows it revoked, saved');

    // --- Restart with hosting on: it starts again with nobody signed in at the host and stays on. ---
    await host.app.close();
    host = await open(base, 'host', 31, helper, {FB_HOSTING_LOOPBACK: '1'});
    h = host.page;
    await host.at(/^\/desktop\/sign-in\/$/);
    await new Promise(r => setTimeout(r, 12000));
    assert.equal(new URL(h.url()).pathname, '/desktop/sign-in/', 'nobody signed in at the host');
    await titled(/Hosting on — 0 computers connected$/, 1000);
    assert.ok(await listening(port), 'still listening after 12 s with nobody signed in');
    await hshot('05b-restarted');
    assert.equal((await agent.call(catalogue)).data.unattended,true);
    assert.equal((await saveThroughAI('Synthetic restarted locked AI customer')).ok,true);
    assert.equal(new URL(h.url()).pathname,'/desktop/sign-in/');
    pass('the same unattended launcher reads and writes after hosting restarts without sign-in');
    await hostSignIn(OWNER, OWNER_PASSWORD);
    await host.menu('host-books');
    await host.at(/^\/hosting\/$/);
    assert.match(await h.locator('main').innerText(), /Hosting is on/);
    assert.match(await h.locator('table.fb-hosting-devices').innerText(), /Revoked/, 'the revocation held over the restart');
    pass('hosting left on starts again after a restart and stays on with nobody signed in at the host');

    // --- Stop hosting: the desktop is the standalone desktop again. ---
    await h.getByRole('button', {name: 'Stop hosting', exact: true}).click();
    await host.at(/^\/hosting\/$/);
    await h.waitForSelector('text=Hosting is off');
    await titled(/^(?!.*Hosting on)/);
    await h.goto(`fb://app/c/${company}/lists/customers/`);
    assert.match(await h.locator('main').innerText(), /Laptop customer/);
    await h.goto('fb://app/desktop/mfa/');
    await h.fill('#id_password',OWNER_PASSWORD);
    await h.getByRole('button',{name:'Continue',exact:true}).click();
    await h.getByRole('button',{name:'Set up authenticator',exact:true}).click();
    await h.locator('.mfa-manual summary').click();
    await h.fill('#id_code',mfaCode(await h.locator('#setup_key').inputValue()));
    await h.getByRole('button',{name:'Verify and enable MFA',exact:true}).click();
    const recovery=(await h.locator('#recovery_codes').inputValue()).trim().split(/\s+/);
    await h.getByRole('button',{name:'I saved my codes',exact:true}).click();
    await host.menu('lock-books');
    await host.at(/^\/desktop\/sign-in\/$/);
    assert.equal((await agent.call(catalogue)).code,'paused_signed_out');
    await hostSignIn(OWNER,OWNER_PASSWORD,recovery[0]);
    await h.goto(`fb://app/c/${company}/home/`);
    const afterMfa=await agent.call(catalogue);
    assert.equal(afterMfa.ok,true);
    assert.equal(afterMfa.data.unattended,false);
    await dialogReplies(host,{'AI assistant connections':2,'Change AI locked access':1});
    const beforeReenable=fs.readFileSync(file);
    await host.menu('ai-manage');
    await until(()=>!fs.readFileSync(file).equals(beforeReenable));
    assert.deepEqual(savedFiles(),[path.basename(file)]);
    assert.equal((await agent.call(catalogue)).data.unattended,true);
    pass('an MFA change pauses the same launcher until sign-in, and Manage restores locked access in place');
    await dialogReplies(host,{'AI assistant connections':1});
    await host.menu('ai-manage');
    await until(()=>!fs.existsSync(file));
    assert.equal((await agent.call(catalogue)).code,'approval_gone');
    const hosted = fs.readdirSync(path.join(host.data, 'hosting'));
    assert.equal(hosted.length, 1, 'one hosting folder for these books');
    const folder = path.join(host.data, 'hosting', hosted[0]);
    const audit = fs.readFileSync(path.join(folder, 'remote-audit.jsonl'), 'utf8');
    for (const kind of ['pair.armed', 'pair.succeeded', 'sign_in.succeeded', 'device.revoked', 'host.stopped']) assert.match(audit, new RegExp(`"kind":"${kind.replace('.', '\\.')}"`), kind);
    assert.ok(!audit.includes(KEEPER_PASSWORD) && !audit.includes(OWNER_PASSWORD), 'no password in the audit');
    assert.equal(fs.statSync(path.join(folder, 'remote-audit.jsonl')).mode & 0o077, 0);
    assert.equal(fs.statSync(path.join(folder, 'gateway')).mode & 0o077, 0);
    await hshot('06-stopped');
    pass('stopping hosting keeps the host signed in on its own books; the remote-access audit is private and has no password');
    await host.app.close();
    for (const [name, bytes] of replay) fs.writeFileSync(name, bytes, {mode: 0o600});
    host = await open(base, 'host', 31, helper, {FB_HOSTING_LOOPBACK: '1'});
    h = host.page;
    await host.at(/^\/desktop\/sign-in\/$/);
    await until(() => fs.existsSync(path.join(approvals, '.endpoint.sealed')));
    assert.equal((await agent.call(catalogue)).code, 'approval_gone');
    await hostSignIn(OWNER, OWNER_PASSWORD, recovery[1]);
    assert.equal((await saveThroughAI('Synthetic forgotten approval must not write')).code, 'approval_gone');
    pass('Forget survives replay of the sealed approval, epoch and anchor across restart and sign-in');
    console.log(`hosting smoke: ${checks} checks passed; screenshots in ${shots}`);
    return checks;
  } finally {
    if (agent) await agent.close();
    for (const one of [client, host]) if (one) await one.app.close().catch(() => {});
    if (!process.env.FB_SMOKE_KEEP) fs.rmSync(base, {recursive: true, force: true});
  }
}

module.exports = {run};
if (require.main === module) run().catch(e => { console.error(e); process.exit(1); });

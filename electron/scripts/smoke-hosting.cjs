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
const root = path.resolve(__dirname, '..');
const OWNER = 'Synthetic host owner';
const OWNER_PASSWORD = 'Synthetic host owner password';
const KEEPER = 'Synthetic hosted bookkeeper';
const KEEPER_PASSWORD = 'Synthetic hosted keeper password';

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
  return {app, page, menu, at, data};
}

async function run({helper = process.env.FB_REMOTE_HELPER, shots: shotsDir, label = 'HOSTING'} = {}) {
  assert.ok(process.env.DISPLAY && ![':0', ':20', ':0.0', ':20.0'].includes(process.env.DISPLAY), 'isolated Xvfb display required');
  assert.ok(helper && fs.existsSync(helper), 'FB_REMOTE_HELPER must name a built fb-remote');
  const base = fs.mkdtempSync(path.join(process.env.TMPDIR, 'fb-hosting-smoke-'));
  const shots = shotsDir || process.env.FB_SMOKE_SHOTS || path.join(base, 'screenshots');
  fs.mkdirSync(shots, {recursive: true});
  let checks = 0;
  const pass = s => console.log(`PASS ${label} ${++checks}: ${s}`);
  let host, client;
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
    const company = /\/c\/([0-9a-f-]{36})\//.exec(h.url())[1];
    const hostSignIn = async (name, password) => {
      await host.at(/\/desktop\/sign-in\/$/);
      if (await h.locator('#id_username').count()) await h.fill('#id_username', name);
      await h.fill('#id_password', password);
      await h.getByRole('button', {name: 'Sign in', exact: true}).click();
      await host.at(/\/home\/$/);
    };
    await h.click('nav.appnav a:has-text("Settings")');
    await h.locator('main').getByRole('link', {name: 'Users', exact: true}).click();
    await h.waitForSelector('#id_role');
    await h.fill('#id_username', KEEPER);
    await h.selectOption('#id_role', 'bookkeeper');
    await h.check('input[name="company"]');
    await h.fill('#id_password', KEEPER_PASSWORD);
    await h.fill('#id_password_repeat', KEEPER_PASSWORD);
    await h.getByRole('button', {name: 'Save user and sign out', exact: true}).click();
    await hostSignIn(OWNER, OWNER_PASSWORD);
    pass('the host computer has synthetic books, its owner and a bookkeeper');

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
    await hostSignIn(OWNER, OWNER_PASSWORD);
    pass('locking the host signs out only the host’s person; the connected computer keeps working');

    // --- Revoke: the computer is cut off at once. ---
    await host.menu('host-books');
    await host.at(/^\/hosting\/$/);
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
    console.log(`hosting smoke: ${checks} checks passed; screenshots in ${shots}`);
    return checks;
  } finally {
    for (const one of [client, host]) if (one) await one.app.close().catch(() => {});
    if (!process.env.FB_SMOKE_KEEP) fs.rmSync(base, {recursive: true, force: true});
  }
}

module.exports = {run};
if (require.main === module) run().catch(e => { console.error(e); process.exit(1); });

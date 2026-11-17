// Remote access, real isolated Electron UI as a thin client of a synthetic host (crates/fb-host
// example dev_host) on 127.0.0.1. Synthetic book, names and passwords only; an isolated Xvfb
// display and profile. The keyring is a synthetic test wrapper (as smoke-keys does).
//
//   FB_REMOTE_HELPER=<target>/debug/fb-remote FB_DEV_HOST=<target>/debug/examples/dev_host node scripts/smoke-remote.cjs
'use strict';
const {_electron: electron} = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const readline = require('node:readline');
const {spawn} = require('node:child_process');
const root = path.resolve(__dirname, '..');
const KEEPER = 'Synthetic bookkeeper';
const PASSWORD = 'Synthetic individual password';

async function main() {
  assert.ok(process.env.DISPLAY && ![':0', ':20', ':0.0', ':20.0'].includes(process.env.DISPLAY), 'isolated Xvfb display required');
  const helper = process.env.FB_REMOTE_HELPER, hostExe = process.env.FB_DEV_HOST;
  assert.ok(helper && fs.existsSync(helper), 'FB_REMOTE_HELPER must name a built fb-remote');
  assert.ok(hostExe && fs.existsSync(hostExe), 'FB_DEV_HOST must name the built dev_host example');
  const base = fs.mkdtempSync(path.join(process.env.TMPDIR, 'fb-remote-smoke-'));
  const home = path.join(base, 'home'), data = path.join(base, 'data'), hostDir = path.join(base, 'host');
  for (const d of [home, hostDir]) fs.mkdirSync(d, {recursive: true});
  const shots = process.env.FB_SMOKE_SHOTS || path.join(base, 'screenshots');
  fs.mkdirSync(shots, {recursive: true});
  let checks = 0;
  const pass = s => console.log(`PASS REMOTE ${++checks}: ${s}`);

  const host = spawn(hostExe, [hostDir, helper], {stdio: ['pipe', 'pipe', 'inherit']});
  const lines = readline.createInterface({input: host.stdout})[Symbol.asyncIterator]();
  const next = async () => JSON.parse((await lines.next()).value);
  const ask = async command => { host.stdin.write(command + '\n'); return next(); };
  const started = await next();
  assert.match(started.addr, /^127\.0\.0\.1:\d+$/);

  const entry = path.join(home, 'test-keyring.cjs');
  // This wrapper runs only in the test, with an intentionally synthetic keyring key.
  fs.writeFileSync(entry, `const {safeStorage}=require('electron');const c=require('node:crypto');\nObject.assign(safeStorage,{isEncryptionAvailable:()=>true,getSelectedStorageBackend:()=> 'gnome_libsecret',encryptString:s=>{const n=c.randomBytes(12),x=c.createCipheriv('aes-256-gcm',Buffer.alloc(32,23),n);const b=Buffer.concat([x.update(s,'utf8'),x.final()]);return Buffer.concat([n,x.getAuthTag(),b]);},decryptString:b=>{const x=c.createDecipheriv('aes-256-gcm',Buffer.alloc(32,23),b.subarray(0,12));x.setAuthTag(b.subarray(12,28));return Buffer.concat([x.update(b.subarray(28)),x.final()]).toString('utf8');}});require(${JSON.stringify(path.join(root, 'main/entry.cjs'))});\n`, {mode: 0o600});
  const env = {...process.env, HOME: home, XDG_CONFIG_HOME: path.join(home, 'config'), XDG_DATA_HOME: path.join(home, 'share'), XDG_CACHE_HOME: path.join(home, 'cache'), FB_DESKTOP_USER_DATA: data, FB_REMOTE_HELPER: helper, ELECTRON_ENABLE_LOGGING: '0'};
  let app, page;
  const launch = async () => {
    app = await electron.launch({args: [entry, '--password-store=basic'], cwd: root, env, timeout: 180000});
    await app.evaluate(({dialog}) => { dialog.showErrorBox = (_t, m) => { throw new Error(m); }; });
    page = await app.firstWindow();
    page.setDefaultTimeout(45000);
    await page.setViewportSize({width: 1440, height: 1000});
    await page.waitForSelector('h1');
  };
  const shot = n => page.screenshot({path: path.join(shots, n + '.png')});
  const menu = id => app.evaluate(({Menu}, id) => Menu.getApplicationMenu().getMenuItemById(id).click(), id);
  // Wait for the window to settle on a path; navigations replaced by the next one (the lock's
  // "Locked" page, then the sign-in or connect screen) abort, which waitForURL reports as an error.
  const at = async (re, timeout = 30000) => {
    const end = Date.now() + timeout;
    while (Date.now() < end) {
      if (re.test(new URL(page.url()).pathname)) { await page.waitForLoadState('load').catch(() => {}); if (re.test(new URL(page.url()).pathname)) return; }
      await new Promise(r => setTimeout(r, 100));
    }
    throw new Error(`timed out waiting for ${re}; at ${page.url()}`);
  };
  const title = () => app.evaluate(({BrowserWindow}) => BrowserWindow.getAllWindows()[0].getTitle());
  try {
    await launch();
    // A computer with no books: the local first-run screen, with the connect choice in the File menu.
    assert.ok(await page.locator('#id_owner_name').count(), 'local first-run screen');
    await menu('remote-connect');
    await page.waitForURL(/\/remote\/connect\/$/);
    await shot('remote-01-connect');
    await page.fill('input[name=addr]', started.addr);
    await page.fill('input[name=code]', 'AAAA-AAAA-AAAA-AAAA');
    await page.fill('input[name=device_name]', 'Synthetic laptop');
    await page.getByRole('button', {name: 'Pair and connect', exact: true}).click();
    await page.waitForSelector('.fb-remote-errors');
    assert.match(await page.locator('.fb-remote-errors').innerText(), /did not work/);
    pass('a wrong pairing code is refused with one plain message');

    await page.fill('input[name=addr]', started.addr);
    await page.fill('input[name=code]', started.code.toLowerCase());
    await page.fill('input[name=device_name]', 'Synthetic laptop');
    await page.getByRole('button', {name: 'Pair and connect', exact: true}).click();
    await page.waitForURL(/\/desktop\/sign-in\/$/);
    assert.match(await page.locator('#fb-remote-bar').innerText(), /Connected to Synthetic office host/);
    assert.match(await title(), /connected to Synthetic office host/);
    assert.ok(fs.existsSync(path.join(data, 'remote-host.sealed')), 'the pairing is kept sealed');
    assert.ok(!fs.readFileSync(path.join(data, 'remote-host.sealed'), 'latin1').includes('Synthetic office host'), 'sealed, not plain');
    await shot('remote-02-host-sign-in');
    pass('pairing with the host code connects and shows the host’s sign-in with a connected-host bar and title');

    await page.fill('#id_username', KEEPER);
    await page.fill('#id_password', 'not the password');
    await page.getByRole('button', {name: 'Sign in', exact: true}).click();
    await page.waitForSelector('.errorlist');
    await page.fill('#id_username', KEEPER);
    await page.fill('#id_password', PASSWORD);
    await page.getByRole('button', {name: 'Sign in', exact: true}).click();
    await page.waitForURL(/\/home\/$/);
    assert.match(await page.locator('.operator').innerText(), /Synthetic bookkeeper/);
    pass('application sign-in on the host is required; a wrong password is refused, the right one signs in');

    await page.goto(`fb://app/c/${started.company}/lists/customers/new/`);
    await page.fill('#id_display_name', 'Remote smoke customer');
    await page.getByRole('button', {name: /^Save/}).first().click();
    await page.waitForURL(url => !String(url).endsWith('/new/'));
    await page.goto(`fb://app/c/${started.company}/lists/customers/`);
    assert.match(await page.locator('main').innerText(), /Remote smoke customer/);
    await shot('remote-03-customer-saved');
    pass('a form saved in the client is saved in the host’s book');

    await page.goto('fb://app/settings/book-protection/');
    assert.match(await page.locator('main').innerText(), /Only on the host computer/);
    assert.equal(await page.locator('form[data-gate]').count(), 0, 'a page at a host address carries no pairing or disconnect form');
    assert.ok(await page.locator('a[href="/remote/disconnect/"]').count(), 'it links to the disconnect screen instead');
    pass('host-only screens are refused in the client, and their notice carries no connection forms');

    const signIn = async () => {
      await page.fill('#id_username', KEEPER);
      await page.fill('#id_password', PASSWORD);
      await page.getByRole('button', {name: 'Sign in', exact: true}).click();
      await page.waitForURL(/\/home\/$/);
    };
    // Lock with the host reachable: this person is signed out there.
    await page.goto(`fb://app/c/${started.company}/home/`);
    await menu('lock-books');
    await at(/^\/desktop\/sign-in\/$/);
    await page.goto(`fb://app/c/${started.company}/home/`);
    await at(/^\/desktop\/sign-in\/$/);
    pass('Lock signs this person out on the host');

    // Lock while the host cannot answer (its book process frozen): the page is replaced at once and
    // the connection closed, so reconnecting starts a new session that needs a sign-in.
    await signIn();
    process.kill(host.pid, 'SIGSTOP');
    try {
      await menu('lock-books');
      await at(/^\/remote\/locked\/$/, 3000);
      assert.doesNotMatch(await page.locator('body').innerText(), /Remote smoke customer|Synthetic bookkeeper/);
      await shot('remote-05-locked');
      await at(/^\/remote\/connect\/$/, 20000);
      assert.match(await page.locator('.fb-remote-errors').innerText(), /could not confirm the sign-out/);
      await shot('remote-06-locked-unreachable');
    } finally {
      process.kill(host.pid, 'SIGCONT');
    }
    await page.getByRole('button', {name: 'Connect', exact: true}).click();
    await at(/^\/desktop\/sign-in\/$/);
    await page.goto(`fb://app/c/${started.company}/home/`);
    await at(/^\/desktop\/sign-in\/$/);
    await signIn();
    pass('Lock with the host unreachable fails closed: no host page stays, and reconnecting needs a sign-in');

    assert.equal(fs.existsSync(path.join(data, 'books')) && fs.readdirSync(path.join(data, 'books')).some(n => n.endsWith('.fbbook')), false, 'the client computer has no book of its own');
    pass('the client never creates or opens a local book');

    // Revocation on the host: the next page says this computer is not accepted, and client mode ends.
    const devices = (await ask('devices')).devices;
    assert.equal(devices.length, 1);
    assert.equal((await ask('revoke ' + devices[0].id)).revoked, true);
    await page.goto(`fb://app/c/${started.company}/home/`);
    assert.match(await page.locator('main').innerText(), /did not accept this computer|not reachable/);
    await shot('remote-04-revoked');
    await menu('remote-disconnect');
    await page.waitForSelector('#id_owner_name');
    pass('a revoked computer is refused and returns to its own (empty) books');
    console.log(`remote smoke: ${checks} checks passed; screenshots in ${shots}`);
    // Part 2: the desktop's own "Host these books" with a second real app (smoke-hosting.cjs).
    if (app) { await app.close().catch(() => {}); app = null; }
    const hosted = await require('./smoke-hosting.cjs').run({helper, shots, label: 'REMOTE HOSTING'});
    console.log(`remote smoke: ${checks} + ${hosted} hosting checks passed`);
  } finally {
    if (app) await app.close().catch(() => {});
    host.stdin.end();
    await new Promise(r => { host.once('exit', r); setTimeout(r, 5000); });
    if (!process.env.FB_SMOKE_KEEP) fs.rmSync(path.join(base, 'host'), {recursive: true, force: true});
  }
}
main().catch(e => { console.error(e); process.exit(1); });

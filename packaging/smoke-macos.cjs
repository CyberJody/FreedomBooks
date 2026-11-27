'use strict';
// macOS packaged-app check: the signed FreedomBooks.app opens, creates synthetic books with the
// built-in key, signs in, and finds its bundled helpers (bank feeds, hosting, client mode). It uses
// this Mac's real FreedomBooks profile folders, so it refuses to run if any of them exists and
// removes what it created (folders and the app's Keychain item) when it finishes.
//
//   node packaging/smoke-macos.cjs <FreedomBooks.app> <screenshots dir>
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const net = require('node:net');
const {spawn, spawnSync} = require('node:child_process');
const {setTimeout: delay} = require('node:timers/promises');
const {chromium} = require(process.env.FB_SMOKE_PLAYWRIGHT || '../electron/node_modules/playwright');
const {signIn} = require('../electron/scripts/smoke-identity.cjs');
const app = process.argv[2];
const shots = process.argv[3];
const USER_PASSWORD = 'Synthetic installed owner42';
const support = path.join(os.homedir(), 'Library/Application Support');
const profile = path.join(support, 'app.freedombooks.FreedomBooks');
const lockDir = path.join(support, 'FreedomBooks');
const logs = path.join(os.homedir(), 'Library/Logs/FreedomBooks');
const KEYCHAIN_SERVICE = 'FreedomBooks Safe Storage';
let checks = 0, child, browser, port;
const pass = message => console.log(`PASS ${++checks}: ${message}`);
const keychainItem = () => spawnSync('security', ['find-generic-password', '-s', KEYCHAIN_SERVICE], {stdio: 'ignore'}).status === 0;

async function start() {
  const log = fs.openSync(path.join(shots, 'launch.log'), 'a');
  child = spawn(path.join(app, 'Contents/MacOS/FreedomBooks'), ['--remote-debugging-address=127.0.0.1', `--remote-debugging-port=${port}`],
    {stdio: ['ignore', log, log], detached: true});
  fs.closeSync(log);
  let launchError;
  child.once('error', error => {launchError = error;});
  for (let retry = 0; retry < 120; retry++) {
    if (launchError) throw launchError;
    if (child.exitCode !== null || child.signalCode) throw new Error(`App exited ${child.exitCode}/${child.signalCode}; see ${shots}/launch.log`);
    try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, {timeout: 1000}); }
    catch { await delay(500); continue; }
    const context = browser.contexts()[0];
    const page = context.pages().find(page => page.url().startsWith('fb://app/')) || await context.waitForEvent('page', {timeout: 30000});
    page.setDefaultTimeout(300000);
    await page.waitForSelector('main h1');
    return page;
  }
  throw new Error(`No debug endpoint; see ${shots}/launch.log`);
}
async function stop() {
  if (browser) { await browser.close().catch(() => {}); browser = null; }
  if (child && child.exitCode === null && !child.signalCode) {
    try { process.kill(-child.pid, 'SIGTERM'); } catch (error) { if (error.code !== 'ESRCH') throw error; }
    for (let retry = 0; retry < 100 && child.exitCode === null && !child.signalCode; retry++) await delay(100);
    if (child.exitCode === null && !child.signalCode) process.kill(-child.pid, 'SIGKILL');
  }
  child = null;
}
async function heading(page) { return (await page.locator('main h1').first().textContent()).trim(); }
async function shot(page, name) { await page.screenshot({path: path.join(shots, name + '.png')}); }

(async () => {
  assert.equal(process.platform, 'darwin', 'macOS only');
  assert.ok(app && app.endsWith('.app') && shots, 'usage: smoke-macos.cjs <FreedomBooks.app> <screenshots dir>');
  for (const existing of [profile, lockDir, logs]) assert.ok(!fs.existsSync(existing), `Refusing to touch an existing FreedomBooks folder: ${existing}`);
  assert.ok(!keychainItem(), `Refusing to touch an existing "${KEYCHAIN_SERVICE}" Keychain item`);
  const verify = spawnSync('codesign', ['--verify', '--deep', '--strict', app], {encoding: 'utf8'});
  assert.equal(verify.status, 0, verify.stderr);
  pass('the app bundle and its helpers carry a valid (ad hoc) signature');
  fs.mkdirSync(shots, {recursive: true});
  const probe = net.createServer();
  await new Promise((resolve, reject) => {probe.once('error', reject); probe.listen(0, '127.0.0.1', resolve);});
  port = probe.address().port;
  await new Promise(resolve => probe.close(resolve));
  try {
    let page = await start();
    assert.equal(await heading(page), 'Set up your books');
    await shot(page, '01-first-run');
    assert.deepEqual(await page.evaluate(() => ({require: typeof window.require, process: typeof window.process, fb: typeof window.fb})),
      {require: 'undefined', process: 'undefined', fb: 'undefined'});
    const renderers = spawnSync('ps', ['-axo', 'args'], {encoding: 'utf8'}).stdout.split('\n').filter(line => line.includes('FreedomBooks Helper (Renderer)'));
    assert.ok(renderers.length > 0, 'renderer process exists');
    for (const line of renderers) assert.ok(!line.includes('--no-sandbox'), 'renderer is sandboxed');
    pass('the app opens first-time setup; renderers are sandboxed with no Node require/process or command bridge');
    await page.fill('#id_owner_name', 'Dana Morgan');
    await page.fill('#id_company_name', 'Kettle Pond Supply (Demo)');
    assert.equal(await page.isChecked('#id_protect'), false);
    try {
      await Promise.all([page.waitForURL(/\/desktop\/sign-in\/$/, {timeout: 120000}), page.click('button:has-text("Create books")')]);
      await signIn(page, USER_PASSWORD);
    } catch (error) {
      console.error('SETUP FAILURE:', page.url(), await page.locator('body').innerText());
      await shot(page, 'failure-setup');
      throw error;
    }
    assert.equal(await heading(page), 'Home');
    await shot(page, '02-home');
    pass('first run with the built-in key creates synthetic books and shows Home');
    const companyBase = page.url().replace(/home\/$/, '');
    await page.goto(companyBase + 'settings/');
    await page.getByRole('link', {name: 'Bank feeds and rules', exact: true}).click();
    await page.waitForURL(/\/bank-feeds\/$/);
    assert.match(await heading(page), /Bank feeds/i);
    pass('Bank feeds and rules is present (bank-feed helper bundled)');
    await page.goto('fb://app/hosting/');
    assert.equal(await heading(page), 'Host these books');
    const hostingText = await page.locator('main').innerText();
    assert.ok(!/helper is missing/.test(hostingText), hostingText);
    assert.match(hostingText, /Hosting is off/);
    await shot(page, '03-hosting-off');
    pass('the bundled fb-remote helper is found and hosting is off by default');
    await page.goto('fb://app/remote/connect/');
    const connectText = await page.locator('main').innerText();
    assert.ok(!/helper is missing/i.test(connectText), connectText);
    assert.ok(await page.locator('form').count() > 0, 'the connect form is shown');
    await shot(page, '04-connect');
    pass('File › Connect to a FreedomBooks host… shows its form');
    const book = path.join(profile, 'books/default.fbbook');
    assert.notEqual(fs.readFileSync(book).subarray(0, 16).toString(), 'SQLite format 3\0');
    assert.equal(fs.statSync(book).mode & 0o077, 0);
    pass('the book is a private encrypted file');
    await stop();
    page = await start();
    const again = await heading(page);
    assert.ok(/Sign in/i.test(again) || again === 'Home', `after relaunch: ${again}`);
    await shot(page, '05-relaunch');
    pass(`relaunching opens the same books (${again})`);
  } finally {
    await stop();
    // The quitting app can recreate its empty profile folder; let it finish first.
    await delay(3000);
    for (const created of [profile, lockDir, logs]) fs.rmSync(created, {recursive: true, force: true});
    if (keychainItem()) spawnSync('security', ['delete-generic-password', '-s', KEYCHAIN_SERVICE], {stdio: 'ignore'});
    console.log('CLEANED: test profile folders and Keychain item removed');
  }
  console.log(`MACOS SMOKE PASSED (${checks} checks)`);
})().catch(error => { console.error(error); process.exitCode = 1; });

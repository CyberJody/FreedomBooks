'use strict';
// CRASHLOG: the main process's side of the problem log and of fb-node's panic boundary. The real
// main.cjs runs against a fake native module that answers like fb-node after a caught panic.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const vm = require('node:vm');
const diagnostics = require('../main/diagnostics.cjs');

const panicError = info => Object.assign(new Error(`FB_PANIC ${JSON.stringify(info)}`), {code: 'GenericFailure'});

test('the log folder is the isolated profile in development and the state folder otherwise', () => {
  const dev = {isPackaged: false, getPath: () => assert.fail('no Electron path in development')};
  const packaged = {isPackaged: true, getPath: name => { assert.equal(name, 'logs'); return '/Users/someone/Library/Logs/FreedomBooks'; }};
  const env = {FB_DESKTOP_USER_DATA: '/tmp/fb-profile', XDG_STATE_HOME: '/tmp/fb-profile/state'};
  assert.equal(diagnostics.logDir(dev, {env, platform: 'linux'}), '/tmp/fb-profile/logs');
  assert.throws(() => diagnostics.logDir(dev, {env: {FB_DESKTOP_USER_DATA: 'relative'}, platform: 'linux'}), /absolute/);
  // A packaged app ignores the development override; XDG_STATE_HOME is honoured when absolute.
  assert.equal(diagnostics.logDir(packaged, {env, platform: 'linux'}), '/tmp/fb-profile/state/FreedomBooks/logs');
  assert.equal(diagnostics.logDir(packaged, {env: {XDG_STATE_HOME: 'relative'}, platform: 'linux'}), path.join(os.homedir(), '.local/state/FreedomBooks/logs'));
  assert.equal(diagnostics.logDir(dev, {env: {XDG_STATE_HOME: '/tmp/x'}, platform: 'linux'}), '/tmp/x/FreedomBooks/logs');
  assert.equal(diagnostics.logDir(packaged, {env: {}, platform: 'darwin'}), '/Users/someone/Library/Logs/FreedomBooks');
  assert.equal(diagnostics.shown('/home/someone/.local/state/FreedomBooks/logs', '/home/someone'), '~/.local/state/FreedomBooks/logs');
  assert.equal(diagnostics.shown('/home/someoneelse/logs', '/home/someone'), '/home/someoneelse/logs');
});

test('fb-node panic errors are recognised and described in plain words', () => {
  assert.equal(diagnostics.panicInfo(new Error('BUSY')), null);
  assert.equal(diagnostics.panicInfo(null), null);
  const info = diagnostics.panicInfo(panicError({closed: true, was_open: true, saved: 'none', op: 'run'}));
  assert.deepEqual(info, {closed: true, was_open: true, saved: 'none', op: 'run'});
  assert.deepEqual(diagnostics.panicInfo(new Error('FB_PANIC {broken')), {closed: false, was_open: false, saved: 'unknown', op: ''});
  assert.match(diagnostics.panicText({...info, op: 'open_protected_book'}), /while opening your books\. Your books are safe: nothing from this was saved\./);
  const kept = diagnostics.problemPage({closed: false, saved: 'none', op: 'render'}, {open: true, folder: '~/x <b>', version: '0.1.0 (abc)'});
  assert.match(kept, /while opening that page/);
  assert.match(kept, /You can carry on using the rest of FreedomBooks\./);
  assert.match(kept, /<code>~\/x &lt;b&gt;<\/code>/);
  assert.match(kept, /id="report-open-log"/);
  assert.match(kept, /href="\/help\/report\/"/);
  assert.doesNotMatch(kept, /<script/, 'the page needs no script; the preload handles its button');
  const closed = diagnostics.problemPage({closed: true, saved: 'partial', op: 'run'}, {open: false, folder: ''});
  assert.match(closed, /closed your books\. Unlock them to carry on\./);
  assert.match(closed, /part of this may have been saved/);
  assert.match(closed, /could not be recorded/);
  assert.doesNotMatch(closed, /report-open-log|help\/report/);
  assert.match(diagnostics.problemPage({closed: true, saved: 'none', op: 'run'}, {open: true, folder: '~'}), /closed your books and opened them again\. Sign in to carry on\./);
  const c = '0195b4a4-55af-719e-96cb-a2f946b067bf';
  assert.equal(diagnostics.continueFrom(`/c/${c}/accounts/`, {closed: false}), `/c/${c}/home/`);
  assert.equal(diagnostics.continueFrom(`/c/${c}/home/`, {closed: false}), '/', 'Home itself failed');
  assert.equal(diagnostics.continueFrom(`/c/${c}/accounts/`, {closed: true}), '/', 'closed books start over');
  assert.equal(diagnostics.continueFrom('/help/report/', {closed: false}), '/');
  assert.match(diagnostics.problemPage({closed: false, saved: 'none', op: 'render'}, {open: true, folder: '~', back: `/c/${c}/home/`}), new RegExp(`href="/c/${c}/home/">Continue`));
});

function harness({url = 'fb://app/help/report/', summary = 'summary text'} = {}) {
  const sent = [], opened = [], copied = [], boxes = [], handlers = {};
  const native = {
    configureLog: dir => sent.push(['configure', dir]),
    logEvent: json => sent.push(JSON.parse(json)),
    diagnosticSummary: () => summary,
  };
  const win = {isDestroyed: () => false, webContents: {getURL: () => url}};
  const d = diagnostics.create({native, dir: '/tmp/fb-profile/logs', shell: {openPath: async dir => { opened.push(dir); return ''; }},
    clipboard: {writeText: t => copied.push(t)}, dialog: {showErrorBox: (...a) => boxes.push(a)}, window: () => win,
    processRef: {on: (name, fn) => { handlers[name] = fn; }}});
  return {d, sent, opened, copied, boxes, handlers, win};
}

test('main-process failures are forwarded to fb-node to scrub and write; refusals are not', () => {
  const h = harness();
  assert.deepEqual(h.sent[0], ['configure', '/tmp/fb-profile/logs']);
  h.d.note('sync-keys', Object.assign(new Error('keyring locked'), {code: 'EACCES'}));
  h.d.note('sync-keys', Object.assign(new Error('The books are busy.'), {code: 'GenericFailure'}));
  h.d.note('sync-keys', panicError({closed: false, saved: 'none', op: 'render'}));
  h.d.unexpected('books.run', new Error('Refused: link not allowed.'));
  h.d.unexpected('books.run', new TypeError("Cannot read properties of undefined (reading 'x')"));
  const events = h.sent.slice(1);
  assert.equal(events.length, 2);
  assert.deepEqual([events[0].op, events[0].kind, events[0].name, events[0].code], ['sync-keys', 'caught', 'Error', 'EACCES']);
  assert.deepEqual([events[1].op, events[1].kind, events[1].name], ['books.run', 'error', 'TypeError']);
  assert.deepEqual(events[1].frames, []);
  assert.ok(!Object.hasOwn(events[1], 'stack') && !Object.hasOwn(events[1], 'message'));
  h.d.install();
  h.handlers.uncaughtException(new RangeError('bad'));
  h.handlers.unhandledRejection('plain reason');
  assert.deepEqual(h.sent.slice(3).map(e => [e.op, e.kind, e.name]), [['main', 'uncaughtException', 'RangeError'], ['main', 'unhandledRejection', 'string']]);
  assert.equal(h.boxes.length, 1);
  assert.match(h.boxes[0][1], /Your books are safe\. The details were recorded in the problem log \(\/tmp\/fb-profile\/logs\)\./);
  // A native module that cannot log never turns a failure into a second one.
  const quiet = diagnostics.create({native: {configureLog() { throw new Error('no'); }}, dir: '/x', window: () => null, processRef: {on() {}}});
  quiet.note('x', new Error('y'));
  assert.equal(quiet.folder(), '');
});

test('the two log buttons work only on their pages, take no arguments and reveal no path to the page', async () => {
  const h = harness();
  const calls = h.d.calls();
  assert.deepEqual(Object.keys(calls), ['app.openLogFolder', 'app.copyDiagnostics']);
  assert.deepEqual(await calls['app.openLogFolder']({}), {outcome: 'done'});
  assert.deepEqual(h.opened, ['/tmp/fb-profile/logs']);
  assert.deepEqual(await calls['app.copyDiagnostics']({}), {outcome: 'done', data: {copied: true}});
  assert.deepEqual(h.copied, ['summary text']);
  for (const payload of [{path: '/etc'}, [], null, 'x', Object.create({})]) {
    await assert.rejects(async () => calls['app.openLogFolder'](payload), /Invalid request/, JSON.stringify(payload));
  }
  const other = harness({url: 'fb://app/c/0195b4a4-55af-719e-96cb-a2f946b067bf/home/'}).d.calls();
  await assert.rejects(async () => other['app.copyDiagnostics']({}), /not available on this page/);
  const problem = harness({url: 'fb://app/desktop/problem/'}).d.calls();
  assert.deepEqual(await problem['app.copyDiagnostics']({}), {outcome: 'done', data: {copied: true}});
});

// The real main.cjs with a fake fb-node: a render that panics, and a form post that panics and
// closes the books (as fb-node does after any call that could have changed them).
function launchMain(native) {
  const handlers = {};
  const windows = [];
  class BrowserWindow { constructor() { this.webContents = {getURL: () => 'fb://app/', isDestroyed: () => false, send() {},
      navigationHistory: {clear() {}, canGoBack: () => false, canGoForward: () => false}}; windows.push(this); } once() {} loadURL(url) { this.url = url; } isDestroyed() { return false; } }
  const electron = {
    app: {getVersion: () => '0.1.0-test', commandLine: {appendSwitch() {}}, enableSandbox() {}, whenReady: () => new Promise(() => {}), on() {}, isPackaged: false, exit() {}},
    protocol: {registerSchemesAsPrivileged() {}}, ipcMain: {handle: (name, fn) => { handlers[name] = fn; }}, BrowserWindow,
    session: {defaultSession: {protocol: {handle() {}}}}, dialog: {}, shell: {}, clipboard: {}, safeStorage: {},
    Menu: {setApplicationMenu() {}, buildFromTemplate: t => t, getApplicationMenu: () => null}, powerMonitor: {on() {}, getSystemIdleTime: () => 0},
  };
  const store = {exists: () => true, bookPath: () => '/synthetic/books/default.fbbook', mainBook: () => '/synthetic/books/default.fbbook', books: () => [],
    activeChoice: () => 'saved', remember() {}, returnToMain() {}, hasPasswordWrap: () => false, keyringWorks: () => true,
    bookKeyStatus: () => 'present', bookKeyCandidates: () => [], readSealedKey: () => 'k', removeSealedKey() {}, sealBook() {}, hasConnectionKeyMaterial: () => true, connectionKey: () => ({key: null, state: 'unavailable'})};
  const stubs = {electron, './development.cjs': {nativeModule: () => '/native', userData: () => '/synthetic', netHelper: () => '/none', remoteHelper: () => '/none', devTools: () => false, hostingLoopback: () => false}, './hosting.cjs': require('../main/hosting.cjs'), '/native': native, './remote-client.cjs': require('../main/remote-client.cjs'), './remote-mode.cjs': require('../main/remote-mode.cjs'),
    './keys.cjs': {userDataDir: () => '/synthetic', store: () => store, CONNECTION: {UNAVAILABLE: 'unavailable'}, SEALED_UNREADABLE: 'x'},
    './identity.cjs': {configure() {}, claimInstance: () => true}, './about.cjs': {osLabel: () => ''},
    './ai-desktop.cjs': {create: () => ({menuItems: [], start: async () => {}, stop: async () => {}, resume: () => Promise.resolve()})}, './save-file.cjs': {create: () => ({})},
    './printing.cjs': {controller: () => ({menuItems: []})}, './edit-menu.cjs': {attach() {}}, './guards.cjs': {...require('../main/guards.cjs'), sender() {}},
    './navigation.cjs': require('../main/navigation.cjs'),
    './diagnostics.cjs': {...diagnostics, logDir: () => '/synthetic/logs', create: o => ({...diagnostics.create(o), install() {}})}};
  const sandbox = {URL, Response, Buffer, setInterval: () => ({unref() {}}), __dirname: path.join(__dirname, '../main'), module: {exports: {}},
    require: n => Object.hasOwn(stubs, n) ? stubs[n] : require(n)};
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../main/main.cjs'), 'utf8') + '\nmodule.exports={calls,handle,setWin:w=>{win=w;}};', sandbox);
  const win = new BrowserWindow();
  sandbox.module.exports.setWin(win);
  return {...sandbox.module.exports, handlers, win};
}

function fakeNative() {
  const state = {open: true, opened: 0, events: [], renders: 0};
  const native = {
    setHost() {}, pluginsEnabled: () => false, bankfeedsEnabled: () => false, invoiceDeliveryEnabled: () => false, configureBackups() {},
    recurringBillsAsync: async () => '',
    configureLog() {}, logEvent: json => state.events.push(JSON.parse(json)), buildInfo: () => JSON.stringify({version: '0.1.0-test (0123456789)'}),
    appState: () => JSON.stringify({open: state.open, signed_in: state.open, session_generation: 1, can_lock: true, builtin: false, path: state.open ? '/synthetic/books/default.fbbook' : null}),
    hostingWanted: () => false, hostingResumeAsync: () => Promise.resolve('{"ok":true,"running":false}'), hostingState: () => '{"available":false}', hostingActive: () => false, hostingStatus: () => '{"running":false,"bind":"","connected":0}',
    recoverKeyChange: () => false, recoverBookChange: () => 'none', finishBookChange: () => false, protectionState: () => JSON.stringify({known: true, builtin: false, ask_password: false}),
    openBook() { state.open = true; state.opened++; return JSON.stringify({home: '/desktop/sign-in/'}); },
    markCopy() {}, keyForSealing: () => 'k', renderGate: (mode, ctx, errors) => `<h1>${mode}</h1>${JSON.parse(errors).join('')}`,
    render(target) {
      state.renders++;
      if (target.includes('/home/')) throw panicError({closed: false, was_open: true, saved: 'none', op: 'render'});
      return JSON.stringify({html: '<h1>Other page</h1>', status: 200, route: 'other'});
    },
    aiGeneration: () => '1',
    run() { state.open = false; throw panicError({closed: true, was_open: true, saved: 'none', op: 'run'}); },
  };
  return {native, state};
}

test('a panicking page shows the problem page and the rest of the app keeps working', async () => {
  const {native, state} = fakeNative();
  const main = launchMain(native);
  const response = await main.handle({url: 'fb://app/c/0195b4a4-55af-719e-96cb-a2f946b067bf/home/', method: 'GET'});
  assert.equal(response.status, 500);
  const html = await response.text();
  assert.match(html, /Something went wrong/);
  assert.match(html, /while opening that page\. Your books are safe: nothing from this was saved\./);
  assert.match(html, /You can carry on using the rest of FreedomBooks\./);
  assert.match(html, /<code>\/synthetic\/logs<\/code>/);
  assert.match(html, /href="\/" *>Continue|href="\/">Continue/, 'Home itself failed, so Continue starts over');
  assert.equal(state.opened, 0, 'a read panic leaves the books open');
  const other = await main.handle({url: 'fb://app/c/0195b4a4-55af-719e-96cb-a2f946b067bf/accounts/', method: 'GET'});
  assert.equal(other.status, 200);
  assert.match(await other.text(), /Other page/);
  // The problem page itself stays available for a reload until the next problem.
  const again = await main.handle({url: 'fb://app/desktop/problem/', method: 'GET'});
  assert.equal(again.status, 500);
  assert.equal(state.events.length, 0, 'Rust already logged the panic; the shell adds nothing');
});

test('a panicking form post that closed the books reopens them like startup and shows the problem page', async () => {
  const {native, state} = fakeNative();
  const main = launchMain(native);
  const reply = await main.handlers['fb:books.run']({sender: null}, {type: 'SubmitForm', path: '/c/0195b4a4-55af-719e-96cb-a2f946b067bf/customers/new/', token: 't', fields: []});
  assert.deepEqual(JSON.parse(JSON.stringify(reply)), {ok: true, value: {outcome: 'redirect', location: '/desktop/problem/'}});
  assert.equal(main.win.url, 'fb://app/desktop/problem/');
  assert.equal(state.opened, 1, 'reopened with the saved key, the way startup does');
  const page = await main.handle({url: 'fb://app/desktop/problem/', method: 'GET'});
  const html = await page.text();
  assert.match(html, /closed your books and opened them again\. Sign in to carry on\./);
  assert.match(html, /Your books are safe: nothing from this was saved\./);
});

test('without a problem the problem address goes home, and the wiring stays in place', async () => {
  const {native} = fakeNative();
  const main = launchMain(native);
  const response = await main.handle({url: 'fb://app/desktop/problem/', method: 'GET'});
  assert.equal(response.status, 303);
  const preload = fs.readFileSync(path.join(__dirname, '../preload/preload.cjs'), 'utf8');
  assert.match(preload, /if \(result\.outcome === 'retry'\) showRetry\(form, result\);/);
  assert.match(preload, /call\('app\.openLogFolder', \{\}\)/);
  assert.match(preload, /call\('app\.copyDiagnostics', \{\}\)/);
  assert.match(preload, /box\.textContent = /, 'the retry message is text, never HTML');
  assert.doesNotMatch(preload, /exposeInMainWorld/);
});


test('JavaScript logging sends only the native frame contract, without book text or absolute paths', () => {
  const h = harness(), error = new TypeError('Synthetic Customer secret123');
  error.stack = `TypeError: Synthetic Customer secret123
    at SyntheticCustomer (${path.resolve(__dirname, '../main/main.cjs')}:123:45)
    at ${path.resolve(__dirname, '../preload/preload.cjs')}:77:2
    at ${path.resolve(__dirname, '../main/../customer-secret.cjs')}:9:1
    at /home/SyntheticCustomer/private.cjs:1:2
    at node:internal/process/task_queues:12:3`;
  h.d.unexpected('books.run', error);
  const event = h.sent[1];
  assert.deepEqual(event.frames, [{file:'main/main.cjs',line:123,col:45},{file:'preload/preload.cjs',line:77,col:2}]);
  assert.doesNotMatch(JSON.stringify(event), /SyntheticCustomer|secret123|home\//);
});

// Remote access (N7): only this computer's own connect and disconnect screens may pair, connect,
// forget or disconnect; a page whose content came from a host cannot.
test('remote pairing and disconnect calls are refused from any other page', async () => {
  const {native} = fakeNative();
  const main = launchMain(native);
  const from = url => ({senderFrame: {url}});
  for (const name of ['remote.pair', 'remote.connect', 'remote.forget', 'remote.disconnect']) {
    for (const url of ['fb://app/c/0195b4a4-55af-719e-96cb-a2f946b067bf/home/', 'fb://app/desktop/sign-in/', 'fb://app/remote/locked/']) {
      const r = await main.handlers[`fb:${name}`](from(url), {addr: '192.168.1.20:47100', code: 'X', device_name: 'Laptop'});
      assert.equal(r.ok, false, `${name} from ${url}`);
      assert.match(r.error, /unexpected sender/);
    }
  }
  const ok = await main.handlers['fb:remote.disconnect'](from('fb://app/remote/disconnect/'), {});
  assert.deepEqual(JSON.parse(JSON.stringify(ok)), {ok: true, value: {outcome: 'redirect', location: '/'}});
});

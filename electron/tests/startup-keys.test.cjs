'use strict';
// The real main.cjs startup and unlock, with the real key store over a fake keyring and a fake
// native module. Each "launch" is a fresh module load against the same synthetic data folder.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const crypto = require('node:crypto');

const tick = () => new Promise(resolve => setImmediate(resolve));
const BOOK_KEY = 'b'.repeat(64);
const PASSWORD = 'synthetic book password';

function keyring(secret = crypto.randomBytes(32), usable = true) {
  return {
    isEncryptionAvailable: () => true,
    getSelectedStorageBackend: () => usable ? 'gnome_libsecret' : 'basic_text',
    encryptString(text) {
      const nonce = crypto.randomBytes(12), cipher = crypto.createCipheriv('aes-256-gcm', secret, nonce);
      const bytes = Buffer.concat([cipher.update(text, 'utf8'), cipher.final()]);
      return Buffer.concat([nonce, cipher.getAuthTag(), bytes]);
    },
    decryptString(bytes) {
      const cipher = crypto.createDecipheriv('aes-256-gcm', secret, bytes.subarray(0, 12));
      cipher.setAuthTag(bytes.subarray(12, 28));
      // The text Electron really throws; it must never reach the unlock screen.
      try { return Buffer.concat([cipher.update(bytes.subarray(28)), cipher.final()]).toString('utf8'); }
      catch { throw new Error('Error while decrypting the ciphertext provided to safeStorage.decryptString.'); }
    },
  };
}

function fakeNative(root, options = {}) {
  const calls = [];
  const state = {open: false, path: null, signedIn: false, generation: 1, builtin: false};
  const record = () => ({known: true, builtin: false, ask_password: options.ask === true});
  const wrong = () => Object.assign(new Error("That password or key doesn't open these books."), {code: 'FB_BOOK_WRONG_KEY'});
  const native = {
    setHost() {}, pluginsEnabled: () => false, bankfeedsEnabled: () => false, invoiceDeliveryEnabled: () => options.invoicing === true,
    recurringBillsAsync: async () => '',
    configureBackups() {},
    appState: () => JSON.stringify({open: state.open, path: state.path, signed_in: state.signedIn, session_generation: state.generation, can_lock: true, builtin: state.builtin, viewing_restored: false}),
    hostingWanted: () => false, hostingResumeAsync: () => Promise.resolve('{"ok":true,"running":false}'), hostingState: () => '{"available":false}', hostingActive: () => false, hostingStatus: () => '{"running":false,"bind":"","connected":0}',
    recoverKeyChange(book) {
      calls.push(['recoverKeyChange', book]);
      if (options.recoverFails) throw Object.assign(new Error('The interrupted password change needs its verified backup. Restore that backup as a new book.'), {code: 'GenericFailure'});
      return false;
    },
    protectionState: book => { calls.push(['protectionState', book]); return JSON.stringify(record()); },
    openBook(book, key) {
      calls.push(['openBook', book]);
      if (key !== BOOK_KEY) throw wrong();
      Object.assign(state, {open: true, path: book}); return JSON.stringify({home: '/desktop/sign-in/'});
    },
    openProtectedBook(book, secret) {
      calls.push(['openProtectedBook', book]);
      if (secret !== PASSWORD) throw wrong();
      Object.assign(state, {open: true, path: book}); return JSON.stringify({home: '/desktop/sign-in/'});
    },
    configureKeys(key, kind, usable, alternates) { calls.push(['configureKeys', kind, usable, /^[0-9a-f]{64}$/.test(key), JSON.parse(alternates || '[]')]); },
    recoverBookChange() { calls.push(['recoverBookChange']); if (options.changeFails) throw Object.assign(new Error('FreedomBooks could not finish an interrupted change of your main books: a file is in both places. Nothing was deleted.'), {code: 'GenericFailure'}); return 'none'; },
    finishBookChange() { calls.push(['finishBookChange']); return false; },
    keyForSealing() { if (!state.open) throw new Error('Locked'); return BOOK_KEY; },
    query: () => '[]',
    renderGate(mode, context, errors) { const c = JSON.parse(context); calls.push(['renderGate', mode, c.reason, JSON.parse(errors), c]); return `<h1>${mode}</h1>`; },
    lock() { state.open = false; },
    markCopy(copy) { calls.push(['markCopy', copy]); },
    // fb-app's "Use this as my books": here it only points the saved choice at the open book.
    useRestoredCopy(previous) {
      calls.push(['useRestoredCopy', previous]);
      if (options.useFails) throw Object.assign(new Error(options.useFails), {code: 'GenericFailure'});
      fs.writeFileSync(path.join(root, 'active-book.json'), JSON.stringify({path: state.path}));
      return JSON.stringify({main: state.path, set_aside: previous ? previous + '.aside' : null, settled: true});
    },
  };
  return {native, calls, state};
}

// The real problem-log glue, without the process-wide handlers and with the log in the fixture.
const realDiagnostics = require('../main/diagnostics.cjs');
const diagnosticsStub = {...realDiagnostics, logDir: () => '/synthetic/logs', create: options => ({...realDiagnostics.create(options), install() {}})};

function launch(root, storage, native) {
  const handlers = {};
  const windows = [];
  const timers = [];
  class BrowserWindow {
    constructor() {
      // navigation.cjs attaches to the window's web contents; these tests only need it not to throw.
      this.webContents = {on() {}, send() {}, isDestroyed: () => false, getURL: () => this.url || 'fb://app/', navigationHistory: {clear() {}, canGoBack: () => false, canGoForward: () => false}};
      windows.push(this);
    }
    once() {} on() {} loadURL(url) { this.url = url; } isDestroyed() { return false; }
  }
  const electron = {
    app: {getVersion: () => '0.1.0-test', commandLine: {appendSwitch() {}}, enableSandbox() {}, whenReady: () => Promise.resolve(), on() {}, isPackaged: false, exit() {}},
    protocol: {registerSchemesAsPrivileged() {}}, ipcMain: {handle: (name, fn) => { handlers[name] = fn; }},
    session: {defaultSession: {protocol: {handle() {}}}}, BrowserWindow, dialog: {}, shell: {}, clipboard: {}, safeStorage: storage,
    Menu: {setApplicationMenu() {}, buildFromTemplate: t => t, getApplicationMenu: () => null},
    powerMonitor: {on() {}, getSystemIdleTime: () => 0},
  };
  const keysSandbox = {module: {exports: {}}, process, Buffer, require: n => n === 'electron' ? {safeStorage: storage} : require(n)};
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../main/keys.cjs'), 'utf8'), keysSandbox);
  const keys = {...keysSandbox.module.exports, userDataDir: () => root};
  const guards = {...require('../main/guards.cjs'), session() {}, window() {}};
  const stubs = {electron, './development.cjs': {nativeModule: () => '/native', userData: () => root, netHelper: () => '/none', remoteHelper: () => '/none', devTools: () => false, hostingLoopback: () => false}, './hosting.cjs': require('../main/hosting.cjs'), '/native': native, './remote-client.cjs': require('../main/remote-client.cjs'), './remote-mode.cjs': require('../main/remote-mode.cjs'),
    './keys.cjs': keys, './identity.cjs': {configure() {}, claimInstance: () => true}, './about.cjs': {osLabel: () => ''},
    './ai-desktop.cjs': {create: () => ({menuItems: [], resume: () => Promise.resolve()})},
    './save-file.cjs': {create: () => ({})}, './printing.cjs': {controller: () => ({menuItems: []})},
    './edit-menu.cjs': {attach() {}}, './guards.cjs': guards, './diagnostics.cjs': diagnosticsStub,
    './navigation.cjs': require('../main/navigation.cjs')};
  const sandbox = {URL, Response, Buffer, setInterval: fn => { const timer = {fn, cleared: false, unref() {}}; timers.push(timer); return timer; }, clearInterval: timer => { timer.cleared = true; }, __dirname: path.join(__dirname, '../main'), module: {exports: {}},
    require: n => Object.hasOwn(stubs, n) ? stubs[n] : require(n)};
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../main/main.cjs'), 'utf8') + '\nmodule.exports={calls,handle,lock:lockNow,status:()=>({gateReason,pending,startupHome})};', sandbox);
  return {...sandbox.module.exports, windows, handlers, timers, dialog: electron.dialog};
}

function fixture(fn) {
  const root = fs.mkdtempSync(path.join(process.env.TMPDIR, 'fb-startup-keys-'));
  return Promise.resolve().then(() => fn(root)).finally(() => fs.rmSync(root, {recursive: true, force: true}));
}
// A protected book that normally opens by itself, saved on the "old" computer.
function oldComputer(root, oldKeyring) {
  const book = path.join(root, 'books', 'default.fbbook');
  fs.mkdirSync(path.dirname(book), {recursive: true, mode: 0o700});
  fs.writeFileSync(book, 'synthetic book bytes');
  fs.writeFileSync(book + '.keys.json', '{}');
  fs.writeFileSync(book + '.os-key', oldKeyring.encryptString(BOOK_KEY), {mode: 0o600});
  fs.writeFileSync(path.join(root, 'connection-key.sealed'), oldKeyring.encryptString('c'.repeat(64)), {mode: 0o600});
  return book;
}
const names = (calls, name) => calls.filter(c => c[0] === name);

test('after a reinstall the books ask once, recovery and connection setup still run, and the next launch opens by itself', () => fixture(async root => {
  const book = oldComputer(root, keyring());
  const now = keyring();
  let f = fakeNative(root), app = launch(root, now, f.native);
  await tick();
  assert.deepEqual(f.calls.map(c => c[0]).slice(0, 3), ['recoverBookChange', 'recoverKeyChange', 'protectionState'], 'recovery runs first');
  assert.equal(names(f.calls, 'openBook').length, 0, 'an unreadable saved key is not tried');
  assert.deepEqual(names(f.calls, 'configureKeys'), [['configureKeys', 'replaced', true, true, []]]);
  assert.equal(app.status().gateReason, 'unreadable');
  assert.equal(app.status().pending, null, 'no error text on the unlock screen');
  assert.equal(app.windows[0].url, 'fb://app/');
  const aside = fs.readdirSync(root).filter(n => n.startsWith('connection-key.sealed.unreadable-'));
  assert.equal(aside.length, 1, 'the old connection key is set aside, not deleted');
  // A wrong entry: plain words, nothing technical.
  let result = await app.calls['app.unlock']({password: 'not it'});
  assert.equal(result.outcome, 'invalid');
  const shown = names(f.calls, 'renderGate').at(-1)[3];
  assert.match(shown[0], /didn’t unlock the books[\s\S]*not your sign-in password/);
  const context = names(f.calls, 'renderGate').at(-1)[4];
  assert.equal(context.reason, 'unreadable', 'the unlock screen is told why it is showing');
  assert.equal(context.book, book);
  assert.equal(context.books.length, 1);
  // The owner unlocks with the book password: the key is saved for this keyring again.
  result = await app.calls['app.unlock']({password: PASSWORD});
  assert.equal(result.outcome, 'redirect');
  assert.equal(names(f.calls, 'recoverKeyChange').length, 3, 'every unlock recovers an interrupted change first');
  assert.equal(now.decryptString(fs.readFileSync(book + '.os-key')), BOOK_KEY);
  assert.equal(fs.readdirSync(path.dirname(book)).filter(n => n.startsWith('default.fbbook.os-key.unreadable-')).length, 1);
  // Next launch on the same computer: opens without asking; connections are simply ready.
  f = fakeNative(root); app = launch(root, now, f.native);
  await tick();
  assert.deepEqual(names(f.calls, 'openBook'), [['openBook', book]]);
  assert.deepEqual(names(f.calls, 'configureKeys'), [['configureKeys', 'ready', true, true, []]]);
  assert.equal(app.windows[0].url, 'fb://app/desktop/sign-in/');
}));

test('a keyring unavailable for one launch changes nothing on disk, then everything opens as before', () => fixture(async root => {
  const old = keyring(), book = oldComputer(root, old);
  const before = fs.readdirSync(root).concat(fs.readdirSync(path.dirname(book))).sort();
  const sealed = fs.readFileSync(path.join(root, 'connection-key.sealed'));
  const gone = {...old, getSelectedStorageBackend: () => 'basic_text'};
  let f = fakeNative(root), app = launch(root, gone, f.native);
  await tick();
  assert.deepEqual(names(f.calls, 'configureKeys'), [['configureKeys', 'unavailable', false, false, []]]);
  assert.equal(app.status().gateReason, 'keyring');
  assert.equal(names(f.calls, 'recoverKeyChange').length, 1);
  // Unlocking by password this launch must not remove or replace the saved book key either.
  await app.calls['app.unlock']({password: PASSWORD});
  // Only the choice of book is remembered; no key file is added, replaced or removed.
  assert.deepEqual(fs.readdirSync(root).concat(fs.readdirSync(path.dirname(book))).filter(n => n !== 'active-book.json').sort(), before);
  assert.deepEqual(fs.readFileSync(path.join(root, 'connection-key.sealed')), sealed);
  // The keyring is back.
  f = fakeNative(root); app = launch(root, old, f.native);
  await tick();
  assert.deepEqual(names(f.calls, 'openBook'), [['openBook', book]]);
  assert.deepEqual(names(f.calls, 'configureKeys'), [['configureKeys', 'ready', true, true, []]]);
}));

test('a failed key-change recovery is shown plainly and does not stop connection setup', () => fixture(async root => {
  oldComputer(root, keyring());
  const f = fakeNative(root, {recoverFails: true}), app = launch(root, keyring(), f.native);
  await tick();
  assert.equal(names(f.calls, 'openBook').length, 0, 'an unrecovered book is not opened');
  assert.equal(names(f.calls, 'configureKeys').length, 1);
  assert.match(names(f.calls, 'renderGate').at(-1)[3][0], /interrupted password change/);
}));

test('the owner chose to be asked: the gate says so and no key is saved', () => fixture(async root => {
  const book = oldComputer(root, keyring());
  const f = fakeNative(root, {ask: true}), app = launch(root, keyring(), f.native);
  await tick();
  assert.equal(app.status().gateReason, 'ask');
  assert.ok(!fs.existsSync(book + '.os-key'), 'the owner asked not to keep the key on this computer');
  await app.calls['app.unlock']({password: PASSWORD});
  assert.ok(!fs.existsSync(book + '.os-key'));
}));

test('raw keyring or file-system text never reaches the unlock screen', () => fixture(async root => {
  oldComputer(root, keyring());
  const f = fakeNative(root);
  f.native.protectionState = () => { throw new Error('EACCES: permission denied, open /home/someone/.config/FreedomBooks/books/default.fbbook.keys.json'); };
  const app = launch(root, keyring(), f.native);
  await tick();
  const shown = names(f.calls, 'renderGate').at(-1)[3];
  assert.equal(shown.length, 1);
  assert.doesNotMatch(shown[0], /EACCES|safeStorage|decrypt|\/home\//);
  assert.equal(names(f.calls, 'configureKeys').length, 1);
  void app;
}));

test('a saved choice of books that is gone asks the owner to choose, and only listed books can be chosen', () => fixture(async root => {
  const old = keyring(), book = oldComputer(root, old);
  const copy = path.join(root, 'books', 'restored-copy', 'restored-copy.fbbook');
  fs.mkdirSync(path.dirname(copy), {recursive: true}); fs.writeFileSync(copy, 'synthetic copy');
  // The saved choice names a book that has since been moved away.
  fs.writeFileSync(path.join(root, 'active-book.json'), JSON.stringify({path: path.join(root, 'books', 'restored-gone', 'restored-gone.fbbook')}));
  const f = fakeNative(root), app = launch(root, old, f.native);
  await tick();
  assert.equal(names(f.calls, 'openBook').length, 0, 'nothing is opened by guessing');
  assert.equal(app.status().gateReason, 'choose');
  const response = await app.handle({url: 'fb://app/', method: 'GET'});
  void response;
  const shown = names(f.calls, 'renderGate').at(-1);
  assert.equal(shown[1], 'choose');
  assert.deepEqual(shown[4].books.map(b => b.path).sort(), [book, copy].sort());
  // A path that is not one of this computer's books is refused.
  let result = await app.calls['app.chooseBook']({path: '/etc/passwd.fbbook'});
  assert.equal(result.outcome, 'invalid');
  // Choosing a book only points the unlock screen at it: its saved key is not used, it asks.
  result = await app.calls['app.chooseBook']({path: book});
  assert.equal(result.outcome, 'redirect');
  assert.equal(names(f.calls, 'openBook').length, 0, 'no saved key opens a chosen book');
  assert.equal(f.state.open, false);
  await app.handle({url: 'fb://app/', method: 'GET'});
  assert.equal(names(f.calls, 'renderGate').at(-1)[1], 'unlock', 'the chosen book asks for its own password');
  // Unlocked with its password it opens as a copy: the saved choice is not silently replaced.
  await app.calls['app.unlock']({password: PASSWORD});
  assert.equal(f.state.path, book);
  assert.deepEqual(names(f.calls, 'markCopy').at(-1), ['markCopy', true]);
  assert.match(JSON.parse(fs.readFileSync(path.join(root, 'active-book.json'), 'utf8')).path, /restored-gone/);
  assert.deepEqual(fs.readdirSync(path.dirname(book)).filter(n => n.startsWith('default.fbbook.os-key')), ['default.fbbook.os-key'], 'a copy\'s key is neither saved nor replaced');
}));

test('a restored copy opened beside the main book is a copy until "Use this as my books"', () => fixture(async root => {
  const old = keyring(), book = oldComputer(root, old);
  fs.writeFileSync(path.join(root, 'active-book.json'), JSON.stringify({path: book}));
  const copy = path.join(root, 'books', 'restored-copy', 'restored-copy.fbbook');
  fs.mkdirSync(path.dirname(copy), {recursive: true}); fs.writeFileSync(copy, 'synthetic copy');
  const f = fakeNative(root, {ask: true}), app = launch(root, old, f.native);
  await tick();
  // Choose the copy at the gate and unlock it: it opens as a copy, the saved choice is unchanged.
  await app.calls['app.chooseBook']({path: copy});
  await app.calls['app.unlock']({password: PASSWORD});
  assert.equal(f.state.path, copy);
  assert.deepEqual(names(f.calls, 'markCopy').at(-1), ['markCopy', true]);
  assert.equal(JSON.parse(fs.readFileSync(path.join(root, 'active-book.json'), 'utf8')).path, book);
  // The owner confirmed; fb-app makes the change (sets the old book aside, saves the choice, audits).
  f.native.run = () => JSON.stringify({outcome: 'done', data: {desktop_keys: 'use-restored'}});
  f.native.protectionState = () => JSON.stringify({known: true, builtin: false, ask_password: true});
  const done = await app.calls['books.run']({type: 'SubmitForm', path: '/settings/restored-book/', token: 'synthetic', fields: [['action', 'use'], ['confirm', 'on']]});
  assert.deepEqual({...done}, {outcome: 'redirect', location: '/settings/restored-book/?done=1'});
  assert.equal(JSON.parse(fs.readFileSync(path.join(root, 'active-book.json'), 'utf8')).path, copy);
  assert.deepEqual(names(f.calls, 'useRestoredCopy'), [['useRestoredCopy', book]]);
  assert.deepEqual(names(f.calls, 'markCopy').at(-1), ['markCopy', false]);
}));

test('a failed "Use this as my books" changes nothing in the shell and says what fb-app said', () => fixture(async root => {
  const old = keyring(), book = oldComputer(root, old);
  const copy = path.join(root, 'books', 'restored-copy', 'restored-copy.fbbook');
  fs.mkdirSync(path.dirname(copy), {recursive: true}); fs.writeFileSync(copy, 'synthetic copy');
  const message = 'FreedomBooks could not set your previous books aside (Permission denied). Nothing was moved and these books are still a copy.';
  const f = fakeNative(root, {ask: true, useFails: message}), app = launch(root, old, f.native);
  await tick();
  await app.calls['app.chooseBook']({path: copy});
  await app.calls['app.unlock']({password: PASSWORD});
  f.native.run = () => JSON.stringify({outcome: 'done', data: {desktop_keys: 'use-restored'}});
  await assert.rejects(app.calls['books.run']({type: 'SubmitForm', path: '/settings/restored-book/', token: 'synthetic', fields: [['action', 'use'], ['confirm', 'on']]}), e => e.message === message);
  assert.deepEqual(names(f.calls, 'markCopy').at(-1), ['markCopy', true], 'still a copy');
  assert.ok(!fs.existsSync(path.join(root, 'active-book.json')), 'the saved choice is not touched by the shell');
  assert.ok(fs.existsSync(book));
}));

// ---- Review findings (getin-fix). Each of these failed against the code before the fix. ----

// A restored copy beside the main book, with a key sealed for this keyring when it was last opened.
function copyWithSavedKey(root, k) {
  const copy = path.join(root, 'books', 'restored-copy', 'restored-copy.fbbook');
  fs.mkdirSync(path.dirname(copy), {recursive: true}); fs.writeFileSync(copy, 'synthetic copy');
  fs.writeFileSync(copy + '.os-key', k.encryptString(BOOK_KEY), {mode: 0o600});
  return copy;
}

test('finding 2: the unlock screen never opens another book through its saved key (ask, locked, idle)', () => fixture(async root => {
  for (const how of ['ask', 'locked', 'idle']) {
    for (const name of fs.readdirSync(root)) fs.rmSync(path.join(root, name), {recursive: true, force: true});
    const k = keyring(), book = oldComputer(root, k);
    if (how === 'ask') fs.rmSync(book + '.os-key');
    fs.writeFileSync(path.join(root, 'active-book.json'), JSON.stringify({path: book}));
    const copy = copyWithSavedKey(root, k);
    const f = fakeNative(root);
    f.native.protectionState = p => JSON.stringify({known: true, builtin: false, ask_password: how === 'ask' && p === book});
    const app = launch(root, k, f.native);
    await tick();
    if (how !== 'ask') {
      assert.equal(f.state.path, book, 'the main book opened by itself at start');
      vm_lock(app, how);
    }
    assert.equal(app.status().gateReason, how);
    assert.equal(f.state.open, false);
    const result = await app.calls['app.chooseBook']({path: copy});
    assert.equal(result.outcome, 'redirect');
    assert.equal(f.state.open, false, `${how}: choosing the copy did not open it`);
    assert.equal(names(f.calls, 'openBook').filter(c => c[1] === copy).length, 0, `${how}: its saved key was not used`);
    // Still locked after a lock; after "ask every time" the copy (which does not ask) gets the plain prompt.
    assert.equal(app.status().gateReason, how === 'ask' ? '' : how, 'the unlock screen says why it is showing');
    await app.handle({url: 'fb://app/', method: 'GET'});
    const shown = names(f.calls, 'renderGate').at(-1);
    assert.equal(shown[1], 'unlock');
    assert.equal(shown[4].book, copy, 'the copy\'s own password prompt');
    // Its own book password opens it, as a copy.
    await app.calls['app.unlock']({password: PASSWORD});
    assert.equal(f.state.path, copy);
    assert.deepEqual(names(f.calls, 'markCopy').at(-1), ['markCopy', true]);
  }
}));
// File › Lock and the idle timer both call main.cjs's lockNow.
function vm_lock(app, how) { app.lock(how === 'idle' ? 'idle' : 'locked'); }

test('finding 2: with the main book opening by itself, a copy still asks for its own password', () => fixture(async root => {
  const k = keyring(), book = oldComputer(root, k);
  // The saved choice is missing, so the unlock screen lists the books.
  fs.writeFileSync(path.join(root, 'active-book.json'), JSON.stringify({path: path.join(root, 'books', 'restored-gone', 'restored-gone.fbbook')}));
  const copy = copyWithSavedKey(root, k);
  const f = fakeNative(root), app = launch(root, k, f.native);
  await tick();
  assert.equal(app.status().gateReason, 'choose');
  await app.calls['app.chooseBook']({path: copy});
  assert.equal(f.state.open, false);
  assert.equal(names(f.calls, 'openBook').length, 0);
  void book;
}));

test('finding 2: opening a copy with its password never saves a key for it', () => fixture(async root => {
  const k = keyring(), book = oldComputer(root, k);
  fs.writeFileSync(path.join(root, 'active-book.json'), JSON.stringify({path: book}));
  const copy = path.join(root, 'books', 'restored-copy', 'restored-copy.fbbook');
  fs.mkdirSync(path.dirname(copy), {recursive: true}); fs.writeFileSync(copy, 'synthetic copy');
  const f = fakeNative(root), app = launch(root, k, f.native);
  await tick();
  vm_lock(app, 'locked');
  await app.calls['app.chooseBook']({path: copy});
  await app.calls['app.unlock']({password: PASSWORD});
  assert.equal(f.state.path, copy);
  assert.ok(!fs.existsSync(copy + '.os-key'), 'no saved key: the copy can only be opened with its password');
}));

test('finding 3: a keyring that answers differently for one launch does not abandon the connection key', () => fixture(async root => {
  const original = keyring(), book = oldComputer(root, original);
  const transient = keyring();
  const sealed = path.join(root, 'connection-key.sealed');
  const aside = () => fs.readdirSync(root).filter(n => n.startsWith('connection-key.sealed.unreadable-')).sort();
  let f = fakeNative(root); launch(root, transient, f.native);
  await tick();
  assert.equal(names(f.calls, 'configureKeys')[0][1], 'replaced', 'a working keyring that cannot read the key: replaced');
  assert.equal(aside().length, 1);
  // The original keyring is back: the original key is put back and nothing is flagged.
  f = fakeNative(root); launch(root, original, f.native);
  await tick();
  assert.deepEqual(names(f.calls, 'configureKeys')[0].slice(1, 4), ['ready', true, true]);
  assert.equal(original.decryptString(fs.readFileSync(sealed)), 'c'.repeat(64), 'the original key is in place again');
  // Flip back and forth: every distinct key stays on disk, and the folder does not grow.
  for (let i = 0; i < 6; i++) {
    f = fakeNative(root); launch(root, i % 2 ? original : transient, f.native);
    await tick();
    assert.equal(names(f.calls, 'configureKeys')[0][1], 'ready', `flip ${i}`);
  }
  assert.ok(aside().length <= 2, `bounded: ${aside()}`);
  const readable = [sealed, ...aside().map(n => path.join(root, n))].map(file => {
    for (const k of [original, transient]) { try { return k.decryptString(fs.readFileSync(file)); } catch { /* other keyring */ } }
    return null;
  });
  assert.ok(readable.includes('c'.repeat(64)), 'the original key is never lost');
  void book;
}));

test('finding 3: past the set-aside limit nothing more is set aside or replaced', () => fixture(async root => {
  const now = keyring(); oldComputer(root, keyring());
  for (let i = 0; i < 8; i++) {
    fs.writeFileSync(path.join(root, `connection-key.sealed.unreadable-2026010${i}T000000Z`), keyring().encryptString('d'.repeat(64)), {mode: 0o600});
  }
  const before = fs.readdirSync(root).sort();
  const sealed = fs.readFileSync(path.join(root, 'connection-key.sealed'));
  const f = fakeNative(root); launch(root, now, f.native);
  await tick();
  assert.deepEqual(names(f.calls, 'configureKeys')[0].slice(1, 4), ['unavailable', true, false]);
  assert.deepEqual(fs.readdirSync(root).filter(n => n !== 'active-book.json').sort(), before.filter(n => n !== 'active-book.json'));
  assert.deepEqual(fs.readFileSync(path.join(root, 'connection-key.sealed')), sealed, 'nothing replaced, nothing deleted');
}));

test('finding 5: a superseded plaintext key is offered for reading what was saved under it', () => fixture(async root => {
  const k = keyring(); oldComputer(root, k);
  fs.writeFileSync(path.join(root, 'connection-key.private'), 'd'.repeat(64), {mode: 0o600});
  let f = fakeNative(root); launch(root, k, f.native);
  await tick();
  assert.deepEqual(names(f.calls, 'configureKeys'), [['configureKeys', 'ready', true, true, ['d'.repeat(64)]]]);
  assert.equal(fs.readdirSync(root).filter(n => n.startsWith('connection-key.private.superseded-')).length, 1, 'kept aside, not deleted');
  // And on every later launch too, until the owner's connections are re-saved under the sealed key.
  f = fakeNative(root); launch(root, k, f.native);
  await tick();
  assert.deepEqual(names(f.calls, 'configureKeys')[0][4], ['d'.repeat(64)]);
}));

test('finding 3: a saved book key set aside earlier that opens the books is put back', () => fixture(async root => {
  const original = keyring(), book = oldComputer(root, original);
  // One launch's keyring could not read the saved book key: it was set aside (as sealBook does).
  fs.renameSync(book + '.os-key', book + '.os-key.unreadable-20261101T000000Z');
  fs.writeFileSync(book + '.os-key', keyring().encryptString(BOOK_KEY), {mode: 0o600});
  const f = fakeNative(root), app = launch(root, original, f.native);
  await tick();
  assert.equal(f.state.path, book, 'opened with the set-aside key, no password asked');
  assert.equal(original.decryptString(fs.readFileSync(book + '.os-key')), BOOK_KEY, 'the key that opens the books is back in place');
  assert.equal(fs.readdirSync(path.dirname(book)).filter(n => n.startsWith('default.fbbook.os-key.unreadable-')).length, 1, 'the other one is kept aside');
  void app;
}));

test('finding 4: in "choose your books" mode an opened book is a copy and the saved choice is untouched', () => fixture(async root => {
  const k = keyring(), book = oldComputer(root, k);
  fs.writeFileSync(path.join(root, 'active-book.json'), '{damaged');
  const f = fakeNative(root), app = launch(root, k, f.native);
  await tick();
  assert.equal(app.status().gateReason, 'choose');
  await app.calls['app.chooseBook']({path: book});
  await app.calls['app.unlock']({password: PASSWORD});
  assert.equal(f.state.path, book);
  assert.deepEqual(names(f.calls, 'markCopy').at(-1), ['markCopy', true], 'a labelled copy, not silently the main books');
  assert.equal(fs.readFileSync(path.join(root, 'active-book.json'), 'utf8'), '{damaged', 'nothing written, nothing set aside');
  // Making it the main books is its own, confirmed step: with no main book, nothing is named as previous.
  f.native.run = () => JSON.stringify({outcome: 'done', data: {desktop_keys: 'use-restored'}});
  await app.calls['books.run']({type: 'SubmitForm', path: '/settings/restored-book/', token: 'synthetic', fields: [['action', 'use'], ['confirm', 'on']]});
  assert.deepEqual(names(f.calls, 'useRestoredCopy'), [['useRestoredCopy', '']]);
  assert.deepEqual(names(f.calls, 'markCopy').at(-1), ['markCopy', false]);
}));

test('finding 4: a book restored at first run opens as a copy until the owner makes it the books', () => fixture(async root => {
  const f = fakeNative(root);
  const copy = path.join(root, 'books', 'restored-recovered-books', 'restored-recovered-books.fbbook');
  f.native.restoreBookFromGateAsync = () => {
    fs.mkdirSync(path.dirname(copy), {recursive: true}); fs.writeFileSync(copy, 'synthetic restored');
    Object.assign(f.state, {open: true, path: copy});
    return Promise.resolve(JSON.stringify({home: '/desktop/sign-in/'}));
  };
  const app = launch(root, keyring(), f.native);
  await tick();
  app.dialog.showOpenDialog = () => Promise.resolve({canceled: false, filePaths: ['/synthetic/backup.fbbackup']});
  const r = await app.calls['app.restoreBook']({name: 'Recovered books', secret: ''});
  assert.equal(r.outcome, 'redirect');
  assert.deepEqual(names(f.calls, 'markCopy').at(-1), ['markCopy', true]);
  assert.ok(!fs.existsSync(path.join(root, 'active-book.json')), 'not made the main books without the owner');
}));

test('finding 6: an unreadable books folder shows an error, never first-time setup, and creates nothing', () => fixture(async root => {
  const books = path.join(root, 'books');
  fs.mkdirSync(path.join(books, 'restored-locked'), {recursive: true});
  const f = fakeNative(root), app = launch(root, keyring(), f.native);
  await tick();
  fs.chmodSync(path.join(books, 'restored-locked'), 0o000);
  try {
    if (process.getuid && process.getuid() === 0) return; // root reads anything
    await app.handle({url: 'fb://app/', method: 'GET'});
    const shown = names(f.calls, 'renderGate').at(-1);
    assert.equal(shown[1], 'error');
    assert.match(shown[3][0], /couldn’t read the list of books/);
    f.native.createProtectedBookAsync = () => { throw new Error('must not be called'); };
    await assert.rejects(app.calls['app.createBook']({owner_name: 'Owner', company_name: 'Co', protect: 'on', lock_password: 'x', lock_password2: 'x', starter_chart: 'standard'}), /already exist/);
    assert.ok(!fs.existsSync(path.join(books, 'default.fbbook')));
  } finally { fs.chmodSync(path.join(books, 'restored-locked'), 0o700); }
}));

test('finding 6: first-time setup is refused unless the unlock screen is in setup mode', () => fixture(async root => {
  const k = keyring(), book = oldComputer(root, k);
  const copy = copyWithSavedKey(root, k);
  fs.rmSync(book); fs.rmSync(book + '.keys.json'); fs.rmSync(book + '.os-key');
  const f = fakeNative(root), app = launch(root, k, f.native);
  await tick();
  await app.handle({url: 'fb://app/', method: 'GET'});
  assert.equal(names(f.calls, 'renderGate').at(-1)[1], 'choose', 'books exist: choose, not setup');
  f.native.createProtectedBookAsync = () => { throw new Error('must not be called'); };
  await assert.rejects(app.calls['app.createBook']({owner_name: 'Owner', company_name: 'Co', protect: 'on', lock_password: 'x', lock_password2: 'x', starter_chart: 'standard'}), /already exist/);
  assert.ok(!fs.existsSync(book) && fs.existsSync(copy));
}));

test('finding 1: an interrupted change of main books that cannot be settled opens nothing', () => fixture(async root => {
  const k = keyring(); oldComputer(root, k);
  const f = fakeNative(root, {changeFails: true}), app = launch(root, k, f.native);
  await tick();
  assert.equal(f.calls[0][0], 'recoverBookChange', 'settled before anything else reads which books to open');
  assert.equal(names(f.calls, 'openBook').length + names(f.calls, 'openProtectedBook').length, 0);
  await app.handle({url: 'fb://app/', method: 'GET'});
  const shown = names(f.calls, 'renderGate').at(-1);
  assert.equal(shown[1], 'error');
  assert.match(shown[3][0], /interrupted change of your main books/);
  assert.equal((await app.calls['app.unlock']({password: PASSWORD})).outcome, 'invalid');
  assert.equal(f.state.open, false);
}));

test('the unlock screen is served with the content security policy', () => fixture(async root => {
  oldComputer(root, keyring());
  const f = fakeNative(root, {ask: true}), app = launch(root, keyring(), f.native);
  await tick();
  const response = await app.handle({url: 'fb://app/', method: 'GET'});
  const guards = require('../main/guards.cjs');
  assert.equal(response.headers.get('content-security-policy'), guards.CSP);
  assert.match(guards.CSP, /default-src 'none'|default-src 'self'/);
  assert.equal(response.headers.get('x-content-type-options'), 'nosniff');
  assert.equal(response.headers.get('cache-control'), 'no-store');
}));


test('a Stripe refresh panic recovers visibly and stops the background writes for this launch', () => fixture(async root => {
  const old = keyring(); oldComputer(root, old);
  const f = fakeNative(root, {invoicing: true});
  const app = launch(root, old, f.native);
  await tick();
  f.state.signedIn = true;
  app.windows[0].url = 'fb://app/c/00000000-0000-7000-8000-000000000001/invoices/';
  let attempts = 0;
  f.native.stripeRefreshAsync = async () => {
    attempts++;
    f.state.open = false;
    throw new Error('FB_PANIC '+JSON.stringify({closed: true, was_open: true, saved: 'unknown', op: 'stripe_refresh_async'}));
  };
  const timer = app.timers.find(t => t.fn.toString().includes('stripeRefreshAsync'));
  await timer.fn();
  assert.equal(attempts, 1);
  assert.equal(timer.cleared, true);
  assert.equal(f.state.open, true, 'normal startup recovery reopened the synthetic book');
  assert.equal(app.windows[0].url, 'fb://app/desktop/problem/');
  const response = await app.handle({url: app.windows[0].url, method: 'GET'});
  assert.match(await response.text(), /Sign in to carry on/);
}));

test('a panic while adopting a restored copy reaches the IPC recovery page', () => fixture(async root => {
  const old = keyring(); oldComputer(root, old);
  const f = fakeNative(root, {ask: true});
  const app = launch(root, old, f.native);
  await tick();
  const copy = copyWithSavedKey(root, old);
  await app.calls['app.chooseBook']({path: copy});
  await app.calls['app.unlock']({password: PASSWORD});
  f.native.run = () => JSON.stringify({outcome: 'done', data: {desktop_keys: 'use-restored'}});
  f.native.useRestoredCopy = () => {
    f.state.open = false;
    throw new Error('FB_PANIC '+JSON.stringify({closed: true, was_open: true, saved: 'unknown', op: 'use_restored_copy'}));
  };
  const frame = {url: 'fb://app/settings/restored-book/'};
  app.windows[0].webContents.mainFrame = frame;
  const result = await app.handlers['fb:books.run']({sender: app.windows[0].webContents, senderFrame: frame}, {type: 'SubmitForm', path: '/settings/restored-book/', token: 'synthetic', fields: [['action', 'use'], ['confirm', 'on']]});
  assert.equal(result.ok, true);
  assert.equal(result.value.location, '/desktop/problem/');
  assert.equal(app.windows[0].url, 'fb://app/desktop/problem/');
  assert.ok(!fs.existsSync(path.join(root, 'active-book.json')), 'the shell did not adopt a failed change');
}));


test('recurring timer requires a signed-in open book and stops after a write panic', () => fixture(async root => {
  const old = keyring(); oldComputer(root, old);
  const f = fakeNative(root);
  const app = launch(root, old, f.native);
  await tick();
  const timer = app.timers.find(t => t.fn.toString().includes('recurringBillsAsync'));
  assert.ok(timer);
  let attempts = 0;
  f.native.recurringBillsAsync = async () => {
    attempts++;
    f.state.open = false;
    throw new Error('FB_PANIC '+JSON.stringify({closed: true, was_open: true, saved: 'unknown', op: 'recurring_bills'}));
  };
  await timer.fn(); assert.equal(attempts, 0, 'signed-out books do not generate');
  f.state.signedIn = true;
  await timer.fn(); assert.equal(attempts, 1);
  assert.equal(timer.cleared, true);
  assert.equal(app.windows[0].url, 'fb://app/desktop/problem/');
  assert.equal(f.state.open, true, 'normal recovery reopens the synthetic book');
}));


test('recurring timer ignores both native and JavaScript busy errors', () => fixture(async root => {
  const old=keyring();oldComputer(root,old);
  const f=fakeNative(root);const app=launch(root,old,f.native);await tick();f.state.signedIn=true;
  const timer=app.timers.find(t=>t.fn.toString().includes('recurringBillsAsync'));
  for(const message of ['BUSY','The books are busy. Wait for the current operation to finish, then try again.']) {
    f.native.recurringBillsAsync=async()=>{throw new Error(message);};
    await timer.fn();
  }
  assert.equal(timer.cleared,false);
  const logs=path.join(root,'logs');
  if(fs.existsSync(logs)) for(const name of fs.readdirSync(logs)) assert.ok(!fs.readFileSync(path.join(logs,name),'utf8').includes('recurring-bills'));
}));

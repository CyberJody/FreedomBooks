'use strict';
// FreedomBooks desktop main process (ARCHITECTURE 8). It owns the window, the one native module (fb-node)
// and the key store. Desktop pages come from the in-process `fb://app/` scheme handler,
// which renders screens through fb-node and serves only the bundled static files.
const { app, BrowserWindow, Menu, ipcMain, protocol, session, dialog, powerMonitor, shell, clipboard, safeStorage } = require('electron');
const fs = require('node:fs');
const path = require('node:path');
const keys = require('./keys.cjs');
const guards = require('./guards.cjs');

const development = require('./development.cjs');
const native = require(development.nativeModule(app, __dirname));
// "Report a problem" shows these for copying into an issue (no other machine details).
native.setHost(app.getVersion(), require('./about.cjs').osLabel());
// Optional capabilities are compiled in: `simplefin` enables bank feeds; `plugins` also enables AI.
// The default build has neither and remains offline.
const PLUGINS = native.pluginsEnabled() === true;
const BANK_FEEDS = native.bankfeedsEnabled() === true;
// WAVE4: plugin network calls run in the separate fb-net process, spawned by fb-node. The path is
// set once here and fb-node refuses to change it; without the helper, bank feeds and ChatGPT say so.
if (PLUGINS || BANK_FEEDS) {
  const helper = development.netHelper(app, __dirname);
  if (fs.existsSync(helper)) native.configureNetwork(path.resolve(helper));
}
const STATIC_ROOT = path.join(__dirname, '..', 'renderer', 'static');
const ORIGIN = 'fb://app';
const IDLE_LOCK_SECONDS = 15 * 60;

protocol.registerSchemesAsPrivileged([
  { scheme: 'fb', privileges: { standard: true, secure: true, supportFetchAPI: false, corsEnabled: false, stream: false } },
]);
// WebRTC UDP bypasses CSP/webRequest. Apply before Chromium starts, also per WebContents below.
app.commandLine.appendSwitch('force-webrtc-ip-handling-policy', 'disable_non_proxied_udp');
app.commandLine.appendSwitch('webrtc-ip-handling-policy', 'disable_non_proxied_udp');
app.enableSandbox();
require('./identity.cjs').configure(app);
// Keep Electron settings/cache in their existing profile; books/keys use dataDir.
// Exit immediately: a secondary process must not continue into data migration.
if (!require('./identity.cjs').claimInstance(app, development.userData(app) || require('./identity.cjs').settingsDir(app))) app.exit(0);

const userData = keys.userDataDir(app);
const store = keys.store(userData);
native.configureBackups(userData);
const output = require('./save-file.cjs').create({dataDir:userData, bookPath:()=>JSON.parse(native.appState()).path || store.bookPath()});
const printing = require('./printing.cjs').controller(native, output, exclusive);

let win = null;
// One-shot page for the next GET of a path: a re-rendered invalid form, or the gate with its errors.
let pending = null;

// BACKUPS-POLISH: a backup, restore or copy runs on fb-node's worker pool, never on this thread, so the
// window keeps answering. One at a time: while it runs no other command reaches the books (fb-node's
// synchronous calls answer BUSY instead of waiting), page loads wait for it without blocking, and a lock
// or quit asked for meanwhile happens as soon as it finishes.
const BUSY_TEXT = 'The books are busy. Wait for the current operation to finish, then try again.';
const LONG_COMMAND = /^(?:\/desktop\/(?:sign-in|users|mfa)\/|\/c\/[0-9a-f-]{36}\/(?:backups\/create|bank-feeds\/connect|bank-feeds\/[0-9a-f-]{36}\/sync)\/|\/settings\/(?:book-protection|restore-backup)\/)$/;
const BACKUPS_PAGE = /^\/c\/[0-9a-f-]{36}\/backups\/$/;
let busy = null;
let lockWhenIdle = false;
const localAI = require('./ai-desktop.cjs').create({app,native,dialog,clipboard,safeStorage,root:userData,window:()=>win,exclusive,busy:()=>!!busy});
function exclusive(work) {
  if (busy) throw new Error(BUSY_TEXT);
  const capture = result => ({...result,generation:JSON.parse(native.appState()).session_generation});
  busy = Promise.resolve().then(work).then(value => capture({value}),error => capture({error})).finally(() => {
    busy = null;
    if (lockWhenIdle) {
      lockWhenIdle = false;
      lockNow();
    }
  }).then(result => {
    // Native completion may itself sign in/out. Bind its resulting session BEFORE the
    // deferred-lock finalizer, then suppress that output if the finalizer ended it.
    requireGeneration(result.generation);
    if(Object.hasOwn(result,'error')) throw result.error;
    return result.value;
  });
  return busy;
}
function requireGeneration(generation) {
  if (generation !== JSON.parse(native.appState()).session_generation) {
    throw Object.assign(new Error('The active user changed. Open the screen again.'), {code:'SESSION_CHANGED'});
  }
}
function sessionLocation() {
  return JSON.parse(native.appState()).open ? '/desktop/sign-in/' : '/';
}

function page(html, status = 200) {
  return new Response(html, {
    status,
    headers: { 'content-type': 'text/html; charset=utf-8', 'content-security-policy': guards.CSP, 'cache-control': 'no-store', 'x-content-type-options': 'nosniff' },
  });
}

function gate(errors = [], starterChart = 'standard') {
  const mode = store.exists() ? 'unlock' : 'setup';
  return native.renderGate(mode, true, '', JSON.stringify(errors), starterChart);
}

function syncBookKeys() {
  const state = JSON.parse(native.appState());
  if (!state.open || !state.path) return;
  store.remember(state.path, state.viewing_restored === true);
  const protection = JSON.parse(native.protectionState(state.path));
  if (!protection.known) return; // Preserve an older OS-only key until recovery setup finishes.
  if (!state.builtin && !protection.ask_password && store.osStoreUsable()) {
    // The books are already open: a keyring that cannot seal means asking for the password next time.
    try { store.sealBook(native.keyForSealing()); } catch { store.removeSealedKey(); }
  } else store.removeSealedKey();
  const item = Menu.getApplicationMenu()?.getMenuItemById('lock-books');
  if (item) item.enabled = state.can_lock || state.signed_in;
}

async function saveRecovery() {
  const {kit,ticket} = JSON.parse(native.recoveryExport());
  const selected = await dialog.showSaveDialog(win, {title:'Save recovery key away from this computer', defaultPath:'FreedomBooks Emergency Kit.txt', filters:[{name:'Emergency Kit',extensions:['txt']}], properties:['showOverwriteConfirmation']});
  if (selected.canceled || !selected.filePath) return {outcome:'done'};
  await exclusive(async () => {
    native.validateRecoveryExport(ticket);
    output.write(selected.filePath, kit);
    await native.recoverySavedAsync(ticket);
  });
  return {outcome:'redirect',location:'/settings/recovery-key/'};
}

async function handle(request) {
  const url = new URL(request.url);
  if (url.host !== 'app') return new Response('Not found', { status: 404 });
  if (request.method !== 'GET') return new Response('Method not allowed', { status: 405 });
  if (!guards.pluginAllowed(url.pathname, {plugins:PLUGINS,bankFeeds:BANK_FEEDS})) return new Response('Not found', { status: 404 });
  if (url.pathname.startsWith('/static/')) return guards.staticFile(STATIC_ROOT, url.pathname.slice('/static/'.length));
  const target = url.pathname + url.search;
  // Several page loads may be waiting on the same operation. Recheck admission after
  // every await so a newly admitted render cannot overlap another request.
  while (busy) await busy.catch(() => {});
  const logo = /^\/c\/([0-9a-f-]{36})\/document-logo\/$/.exec(url.pathname);
  if (logo) {
    try {
      if (!JSON.parse(native.appState()).open) return new Response('Not found', {status:404});
      const data = JSON.parse(native.query(JSON.stringify({type:'CompanyLogo',company_id:logo[1]})));
      if (!data || !['image/png','image/jpeg'].includes(data.mime)) return new Response('Not found', {status:404});
      return new Response(Buffer.from(data.base64,'base64'), {headers:{'content-type':data.mime,'cache-control':'no-store','x-content-type-options':'nosniff','content-security-policy':guards.CSP}});
    } catch { return new Response('Not found', {status:404}); }
  }
  if (pending && pending.generation !== undefined && pending.generation !== JSON.parse(native.appState()).session_generation) pending = null;
  if (pending && pending.path === target) {
    const { html, status } = pending;
    pending = null;
    return page(html, status);
  }
  if (!JSON.parse(native.appState()).open) return page(gate(), 200);
  try {
    const r = JSON.parse(BACKUPS_PAGE.test(url.pathname)
      ? await exclusive(() => native.renderAsync(target)) : native.render(target));
    // exclusive applies a deferred lock before resolving. Never deliver the completed
    // render's book contents after that lock has taken effect.
    if (!JSON.parse(native.appState()).open) return page(gate(), 200);
    if (r.status >= 300 && r.status < 400) {
      const location = new URL(guards.target({ target: r.html }), ORIGIN);
      if (location.protocol !== 'fb:' || location.host !== 'app') throw new Error('Invalid local redirect');
      return Response.redirect(location.href, r.status);
    }
    if (r.route === 'history' && url.searchParams.get('format') === 'csv' && r.html.startsWith('\uFEFF')) {
      return new Response(r.html, { status: 200, headers: { 'content-type': 'text/csv; charset=utf-8', 'content-disposition': 'attachment; filename="audit-history.csv"', 'content-security-policy': guards.CSP, 'cache-control': 'no-store' } });
    }
    // D8: direct report CSV requests must never parse user-controlled cells as HTML. QueryDict uses
    // the last value of a repeated key; retain that rule here as well as in fb-app's validators.
    if (r.status === 200 && ['profit_loss', 'balance_sheet', 'general_ledger'].includes(r.route) && url.searchParams.getAll('output').at(-1) === 'csv') {
      return new Response(r.html, { status: 200, headers: { 'content-type': 'text/csv; charset=utf-8', 'content-disposition': 'attachment; filename="report.csv"', 'content-security-policy': guards.CSP, 'cache-control': 'no-store', 'x-content-type-options': 'nosniff' } });
    }
    if (r.status === 200 && ['register', 'reconcile:report'].includes(r.route) && url.searchParams.getAll('output').at(-1) === 'csv') {
      return new Response(r.html, { status: 200, headers: { 'content-type': 'text/csv; charset=utf-8', 'content-disposition': 'attachment; filename="banking.csv"', 'content-security-policy': guards.CSP, 'cache-control': 'no-store', 'x-content-type-options': 'nosniff' } });
    }
    return page(r.html, r.status);
  } catch (e) {
    if (e.code === 'SESSION_CHANGED') {
      return JSON.parse(native.appState()).open ? Response.redirect(ORIGIN+'/desktop/sign-in/',303) : page(gate(),200);
    }
    if (String(e.message) === 'LOCKED') return page(gate(), 200);
    return page(native.renderGate('unlock', false, '', JSON.stringify([String(e.message)])), 404);
  }
}

function lockNow() {
  if (busy) {
    lockWhenIdle = true;
    return;
  }
  const state = JSON.parse(native.appState());
  if (!state.can_lock) {
    if (!state.open) return;
    // Also cancel a password-verified MFA challenge on OS/idle/manual lock.
    native.signOut();
    pending = null;
    if (win && !win.isDestroyed()) win.loadURL(`${ORIGIN}/desktop/sign-in/`);
    return;
  }
  native.lock();
  store.returnToMain();
  pending = null;
  if (win && !win.isDestroyed()) win.loadURL(`${ORIGIN}/`);
}

// The fixed call list (preload exposes exactly these). Each handler checks the sender, then the payload.
const calls = {
  'books.run': async (payload) => {
    if (busy) throw new Error(BUSY_TEXT);
    const command = guards.command(payload, { plugins: PLUGINS, bankFeeds: BANK_FEEDS });
    const long = command.type === 'SubmitForm' && LONG_COMMAND.test(command.path);
    let outcome;
    try { outcome = JSON.parse(long ? await exclusive(() => native.runAsync(JSON.stringify(command))) : native.run(JSON.stringify(command))); }
    catch(error) {
      if(error.code === 'SESSION_CHANGED') return {outcome:'redirect',location:sessionLocation()};
      throw error;
    }
    const generation = JSON.parse(native.appState()).session_generation;
    if (long && !JSON.parse(native.appState()).open) return {outcome:'redirect',location:'/'};
    syncBookKeys();
    if (outcome.outcome === 'done' && outcome.data?.desktop_keys === 'copy-recovery') {
      // The path-bound form and native authorization both have to allow this export.
      // Copying is not a durable backup and must not mark the recovery key as saved.
      try { clipboard.writeText(native.recoveryKey()); outcome.data.copied = true; }
      catch { outcome.data.copy_error = 'The recovery key could not be copied. Try again, or save it to a file.'; }
      return outcome;
    }
    if (outcome.outcome === 'done' && outcome.data?.desktop_keys === 'save-recovery') return saveRecovery();
    if (outcome.outcome === 'done' && outcome.data && typeof outcome.data.desktop === 'string') return backupDialog(outcome.data);
    if (outcome.outcome === 'done' && outcome.data && outcome.data.desktop_print === true) {
      try {
        const printed = await printing.choose(win, outcome.data.target);
        void printed;
      }
      catch (error) { outcome.data.print_error = String(error.message || error); }
      requireGeneration(generation);
      return outcome;
    }
    if (outcome.outcome === 'export') {
      const selected = await dialog.showSaveDialog(win, { defaultPath: outcome.filename, filters: [{ name: 'CSV', extensions: ['csv'] }], properties: ['showOverwriteConfirmation'] });
      if (selected.canceled || !selected.filePath) return { outcome: 'done', data: { saved: false } };
      requireGeneration(generation);
      output.write(selected.filePath, outcome.contents);
      return { outcome: 'done', data: { saved: true } };
    }
    if (outcome.outcome === 'external') {
      if (!PLUGINS) throw new Error('Refused: link not allowed.');
      // The ChatGPT sign-in: the system browser, never this window; then the page fb-app named.
      const url = guards.signIn(outcome.url, JSON.parse(native.externalOrigins()));
      const location = guards.target({ target: outcome.location });
      await shell.openExternal(url);
      requireGeneration(generation);
      return { outcome: 'redirect', location };
    }
    if (['invalid', 'page'].includes(outcome.outcome) && command.type === 'SubmitForm') {
      pending = { path: command.path, html: outcome.html, status: outcome.status || 400, generation };
      return { outcome: 'invalid', reload: command.path };
    }
    return outcome;
  },
  'app.createBook': async (payload) => {
    if (busy) throw new Error(BUSY_TEXT);
    const input = guards.setup(payload);
    if (store.exists()) throw new Error('A book already exists on this computer.');
    const errors = [];
    if (!input.owner_name.trim()) errors.push("Enter the owner's name.");
    if (!input.company_name.trim()) errors.push('Enter the company name.');
    const protect = input.protect === 'on';
    if (protect && input.lock_password !== input.lock_password2) errors.push('The two passwords differ.');
    if (errors.length) return failGate(errors, input.starter_chart);
    try {
      const created = JSON.parse(await exclusive(() => native.createProtectedBookAsync(store.bookPath(), input.owner_name, input.company_name, input.lock_password, protect, input.starter_chart)));
      syncBookKeys();
      return {outcome:'redirect',location:created.home};
    } catch (e) { return failGate([String(e.message)], input.starter_chart); }
  },
  'app.restoreBook': async (payload) => {
    if (busy) throw new Error(BUSY_TEXT);
    const input = guards.restoreBook(payload);
    const selected = await dialog.showOpenDialog(win, {title:'Choose a backup to restore', properties:['openFile'], filters:[{name:'FreedomBooks backups',extensions:['fbbackup']}]});
    if (selected.canceled || !selected.filePaths[0]) return {outcome:'done'};
    try {
      const opened = JSON.parse(await exclusive(() => native.restoreBookFromGateAsync(selected.filePaths[0], input.name || 'Recovered books', input.secret)));
      syncBookKeys();
      return {outcome:'redirect',location:opened.home || '/'};
    } catch (e) { return failGate([String(e.message)]); }
  },

  // The issue page opens in the system browser; only guards.EXTERNAL_URLS, matched exactly.
  'app.openExternal': async (payload) => {
    await shell.openExternal(guards.external(payload));
    return { outcome: 'done' };
  },
  'app.unlock': (payload) => {
    const input = guards.unlock(payload);
    try {
      const state = JSON.parse(native.protectionState(store.bookPath()));
      let opened;
      if (state.known) opened = JSON.parse(native.openProtectedBook(store.bookPath(), input.password || ''));
      else if (store.hasPasswordWrap()) opened = JSON.parse(native.openBookWithPassword(store.bookPath(), store.readWrap(), input.password || ''));
      else {
        opened = keys.openLegacyBook(native, store, input.password || '');
      }
      syncBookKeys();
      return { outcome: 'redirect', location: opened.home || '/' };
    } catch (e) {
      return failGate([String(e.message)]);
    }
  },
};

// DESKTOP-BACKUPS: a Backups action (or J-ATTACH: an Attach) that needs a native dialog. The one-time ticket stays in this
// process; Rust (fb-app) does every file operation with the path the user picked.
async function backupDialog({ desktop, ticket, folder, filename }) {
  let chosen = '';
  if (desktop === 'choose-backup-folder') {
    const r = await dialog.showOpenDialog(win, { title: 'Choose a folder for backups', defaultPath: folder, properties: ['openDirectory', 'createDirectory'] });
    chosen = r.canceled ? '' : r.filePaths[0] || '';
  } else if (desktop === 'pick-backup') {
    const r = await dialog.showOpenDialog(win, { title: 'Choose a backup to restore', defaultPath: folder, properties: ['openFile'], filters: [{ name: 'FreedomBooks backups', extensions: ['fbbackup'] }] });
    chosen = r.canceled ? '' : r.filePaths[0] || '';
  } else if (desktop === 'pick-attachment') {
    // J-ATTACH: the page never sends file bytes; Rust reads the picked file (size-limited) and checks its type.
    const r = await dialog.showOpenDialog(win, { title: 'Attach a file', properties: ['openFile'], filters: [{ name: 'PDF, PNG, JPEG or WebP', extensions: ['pdf', 'png', 'jpg', 'jpeg', 'webp'] }] });
    chosen = r.canceled ? '' : r.filePaths[0] || '';
  } else if (desktop === 'save-backup') {
    const r = await dialog.showSaveDialog(win, { title: 'Save a copy of this backup', defaultPath: filename, filters: [{ name: 'FreedomBooks backups', extensions: ['fbbackup'] }] });
    chosen = r.canceled ? '' : r.filePath || '';
  } else throw new Error('Unknown desktop action.');
  return JSON.parse(await exclusive(() => native.backupDialogAsync(String(ticket), chosen)));
}

function failGate(errors, starterChart) {
  pending = { path: '/', html: gate(errors, starterChart), status: 400 };
  return { outcome: 'invalid', reload: '/' };
}

for (const [name, fn] of Object.entries(calls)) {
  ipcMain.handle(`fb:${name}`, async (event, payload) => {
    guards.sender(event, win);
    try {
      return { ok: true, value: await fn(payload) };
    } catch (e) {
      const message = String(e && e.message ? e.message : e);
      return { ok: false, error: message === 'BUSY' ? BUSY_TEXT : message };
    }
  });
}

let startupHome = '';
function createWindow() {
  win = new BrowserWindow({
    width: 1440,
    height: 1000,
    show: false,
    title: 'FreedomBooks',
    backgroundColor: '#ffffff',
    webPreferences: {
      preload: path.join(__dirname, '..', 'preload', 'preload.cjs'),
      contextIsolation: true,
      sandbox: true,
      nodeIntegration: false,
      nodeIntegrationInWorker: false,
      nodeIntegrationInSubFrames: false,
      webSecurity: true,
      allowRunningInsecureContent: false,
      webviewTag: false,
      spellcheck: false,
      devTools: development.devTools(app),
    },
  });
  guards.window(win, ORIGIN);
  require('./edit-menu.cjs').attach(win, Menu);
  win.once('ready-to-show', () => win.show());
  const state = JSON.parse(native.appState());
  const companies = state.signed_in ? JSON.parse(native.query(JSON.stringify({type:'Companies'}))) : [];
  win.loadURL(`${ORIGIN}${startupHome || (companies.length ? '/c/'+companies[0].id+'/home/' : '/')}`);
}

app.whenReady().then(() => {
  guards.session(session.defaultSession);
  session.defaultSession.protocol.handle('fb', handle);
  Menu.setApplicationMenu(Menu.buildFromTemplate([
    { label: 'File', submenu: [...printing.menuItems, { type:'separator' }, { id:'lock-books', label:'Lock', enabled:false, accelerator:'CmdOrCtrl+L', click:lockNow }, { type: 'separator' }, { role: 'quit' }] },
    { label:'Settings', submenu:[...localAI.menuItems,{type:'separator'},{label:'Multi-factor authentication…',click:()=>{if(JSON.parse(native.appState()).open) win.loadURL(ORIGIN+'/desktop/mfa/');}},{label:'Users…',click:()=>{if(JSON.parse(native.appState()).open) win.loadURL(ORIGIN+'/desktop/users/');}},{label:'Switch user / sign out…',click:()=>{if(JSON.parse(native.appState()).open) win.loadURL(ORIGIN+'/desktop/session/');}},{label:'Book protection…',click:()=>{if(JSON.parse(native.appState()).open) win.loadURL(ORIGIN+'/settings/book-protection/');}}] },
    { label: 'Edit', submenu: [{ role: 'undo' }, { role: 'redo' }, { type: 'separator' }, { role: 'cut' }, { role: 'copy' }, { role: 'paste' }, { role: 'selectAll' }] },
    { label: 'View', submenu: [{ role: 'resetZoom' }, { role: 'zoomIn' }, { role: 'zoomOut' }] },
  ]));
  powerMonitor.on('suspend', lockNow);
  powerMonitor.on('lock-screen', lockNow);
  setInterval(() => {
    if (!busy && powerMonitor.getSystemIdleTime() >= IDLE_LOCK_SECONDS && JSON.parse(native.appState()).open) lockNow();
  }, 30 * 1000).unref();
  try {
    native.configureKeys(store.connectionKey(), store.osStoreUsable());
    if (store.exists()) {
      native.recoverKeyChange(store.bookPath());
      const state = JSON.parse(native.protectionState(store.bookPath()));
      if (state.known && state.ask_password) store.removeSealedKey();
      if (state.builtin) startupHome = JSON.parse(native.openProtectedBook(store.bookPath(), '')).home;
      else if (!state.known && !store.hasPasswordWrap()) {
        // A copied built-in-key file needs no local metadata or OS store to open.
        const opened = keys.openLegacyBook(native, store, '', store.osStoreUsable());
        if (opened) startupHome = opened.home;
      } else if (state.known && !state.ask_password && store.osStoreUsable()) {
        startupHome = JSON.parse(native.openBook(store.bookPath(), store.readSealedKey())).home;
      }
    }
  } catch (e) { pending = {path:'/',html:gate([String(e.message)]),status:400}; }
  createWindow();
  syncBookKeys();
});

app.on('second-instance', () => {
  if (win) {
    if (win.isMinimized()) win.restore();
    win.focus();
  }
});
app.on('window-all-closed', () => {
  if (!busy) native.lock();
  app.quit();
});
// DESKTOP-BACKUPS: app.quit() (File › Quit) skips window-all-closed; close the open book cleanly too.
// BACKUPS-POLISH: a running backup or restore finishes first, then the book closes and the app quits.
app.on('will-quit', (event) => {
  if (busy) {
    event.preventDefault();
    busy.catch(() => {}).then(() => app.quit());
    return;
  }
  native.lock();
});

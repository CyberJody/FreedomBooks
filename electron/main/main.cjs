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
const INVOICE_DELIVERY = native.invoiceDeliveryEnabled() === true;
// WAVE4: plugin network calls run in the separate fb-net process, spawned by fb-node. The path is
// set once here and fb-node refuses to change it; without the helper, bank feeds and ChatGPT say so.
if (PLUGINS || BANK_FEEDS || INVOICE_DELIVERY) {
  const helper = development.netHelper(app, __dirname);
  if (fs.existsSync(helper)) native.configureNetwork(path.resolve(helper));
}
// "Host these books": the same fb-remote helper, as the TLS listener. fb-node takes the path once.
{
  const helper = development.remoteHelper(app, __dirname);
  if (fs.existsSync(helper)) native.configureRemote(path.resolve(helper));
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
// CRASHLOG: the problem log (diagnostics.cjs). Development runs keep it inside the isolated profile.
const diagnostics = require('./diagnostics.cjs').create({native, dir: require('./diagnostics.cjs').logDir(app), shell, clipboard, dialog, window: () => win});
diagnostics.install();
native.configureBackups(userData);
// An interrupted "Use this as my books" is finished or undone before the saved choice of books is
// read. If that can't be decided, no book opens and the unlock screen says why (gateMode 'error').
let bookChangeError = null;
try { native.recoverBookChange(); } catch (e) { bookChangeError = e; }
const store = keys.store(userData);
const output = require('./save-file.cjs').create({dataDir:userData, bookPath:()=>JSON.parse(native.appState()).path || store.bookPath()});

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
let connectionKeysConfigured = false;
let busy = null;
let lockWhenIdle = false;
let scheduledBackupTimer;
let scheduledBackupDeferred = false;
let scheduledBackupRunning = false;
const localAI = require('./ai-desktop.cjs').create({app,native,dialog,clipboard,safeStorage,root:userData,window:()=>win,exclusive,busy:()=>!!busy,client:()=>remote.active()});
// Remote access, client mode (remote-mode.cjs): off until the person connects to a host. While
// connected, pages and form posts go to the host; the local books stay closed.
const remoteLink = require('./remote-client.cjs');
const remoteStore = remoteLink.store({dir: userData, safeStorage, writePrivate: keys.writePrivate});
let remoteGeneration = 0;
const remote = require('./remote-mode.cjs').create({
  client: remoteLink.child({helper: development.remoteHelper(app, __dirname), onLink: state => remote.setLink(state)}),
  store: remoteStore, window: () => win, output, dialog, choosePrint: target => printing.choose(win, target),
  show: (location, expected) => {
    if (!win || win.isDestroyed()) return;
    if (expected && win.webContents.getURL() !== ORIGIN + expected) return;
    return win.loadURL(ORIGIN + location).catch(() => {});
  },
  onChange: () => {
    remoteGeneration++;
    const menu = Menu.getApplicationMenu();
    const off = menu?.getMenuItemById('remote-disconnect');
    if (off) off.enabled = remote.active();
    for (const id of ['ai-connect','ai-manage','book-protection','host-books']) {
      const item = menu?.getMenuItemById(id);
      if (item) item.enabled = !remote.active();
    }
    const lockItem = menu?.getMenuItemById('lock-books');
    if (lockItem && remote.active()) lockItem.enabled = true;
  },
});
const printing = require('./printing.cjs').controller(native, output, exclusive, remote);
const REMOTE = require('./remote-mode.cjs');
// "Host these books" (hosting.cjs): Settings › Host these books… on the computer with the books.
const hosting = require('./hosting.cjs').create({
  native, exclusive, os: require('node:os'), loopback: development.hostingLoopback(app),
  confirm: async (message, detail, action) => {
    const {response} = await dialog.showMessageBox(win, {type:'question', message, detail, buttons:['Cancel', action], defaultId:0, cancelId:0});
    return response === 1 && !remote.active() && !win.isDestroyed() && win.webContents.getURL() === ORIGIN + hosting.PATH;
  },
  note: (op, e) => diagnostics.note(op, e),
  onNotice: () => { if (win && !win.isDestroyed() && new URL(win.webContents.getURL()).pathname === hosting.PATH) win.loadURL(ORIGIN + hosting.PATH); },
  onStatus: s => {
    const label = require('./hosting.cjs').statusLabel(s);
    if (label === hostingTitle) return;
    hostingTitle = label;
    if (win && !win.isDestroyed()) win.setTitle(windowTitle(win.webContents.getTitle()));
  },
});
// While hosting, the window title says so and how many computers are connected (review S5).
let hostingTitle = '';
function windowTitle(title) {
  return hostingTitle ? `${title} — ${hostingTitle}` : title;
}
function remoteSender(event, paths) {
  let pathname = '';
  try { pathname = new URL(event.senderFrame.url).pathname; } catch { /* refused below */ }
  if (!paths.includes(pathname)) throw new Error('Refused: unexpected sender.');
}
function remoteResult(r) {
  if (r.navigated) return {outcome: 'done'};
  if (r.errors) { remote.flash(r.errors); return {outcome: 'invalid', reload: '/remote/connect/'}; }
  return {outcome: 'redirect', location: r.location};
}
// Connecting closes this computer's own books first: one window shows one set of books.
function closeLocalForRemote() {
  if (busy) throw new Error(BUSY_TEXT);
  if (JSON.parse(native.appState()).open) { native.lock(); store.returnToMain(); }
  pending = null;
}
async function disconnectToBooks() {
  await remote.disconnect();
  if (remote.active() || JSON.parse(native.appState()).open) return '/';
  startupHome = '';
  gateReason = '';
  // Disconnect can be reached from a locked desktop. Only try the built-in key.
  startupOpen(false, false);
  if (startupHome) {
    try { syncBookKeys(); } catch (e) { diagnostics.note('sync-keys', e); }
    void hosting.resume();
  }
  return startupHome || '/';
}
async function remotePage(target, url, marker) {
  const r = await remote.render(target, marker);
  if (r.marker && !remote.accepts(r.marker)) {
    if (r.binary) return new Response('Not found', {status: 404});
    return Response.redirect(ORIGIN + remote.currentPage(), 303);
  }
  if (r.redirect) return Response.redirect(new URL(r.redirect, ORIGIN).href, 303);
  if (r.binary) return new Response(r.binary, {headers: {'content-type': r.mime, 'cache-control': 'no-store', 'x-content-type-options': 'nosniff', 'content-security-policy': guards.CSP}});
  return guards.csvExportResponse(r.route, url, r.status, r.html, 'export.csv') ?? page(r.html, r.status);
}
function exclusive(work, {bindSession=true} = {}) {
  if (busy) throw new Error(BUSY_TEXT);
  const capture = result => bindSession ? {...result,generation:JSON.parse(native.appState()).session_generation} : result;
  busy = Promise.resolve().then(work).then(value => capture({value}),error => capture({error})).finally(() => {
    busy = null;
    if (lockWhenIdle) {
      lockWhenIdle = false;
      lockNow('locked');
    }
    if (scheduledBackupDeferred) {
      scheduledBackupDeferred = false;
      setImmediate(() => wakeScheduledBackup({ allowDefer: false }));
    }
  }).then(result => {
    // Native completion may itself sign in/out. Bind its resulting session BEFORE the
    // deferred-lock finalizer, then suppress that output if the finalizer ended it.
    if(bindSession)requireGeneration(result.generation);
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
async function scheduledBackupTick({ allowDefer = true } = {}) {
  if (scheduledBackupRunning || lockWhenIdle || !win || win.isDestroyed() || remote.active()) return;
  if (busy) {
    if (allowDefer) scheduledBackupDeferred = true;
    return;
  }
  try {
    // An idle screen can be due for its existing lock callback at the same
    // instant as the backup timer. Give that callback first use of the books.
    if (powerMonitor.getSystemIdleTime() >= IDLE_LOCK_SECONDS || native.hostingActive()) return;
    const state = JSON.parse(native.appState());
    if (!state.open || !state.signed_in || state.viewing_restored || state.hosting) return;
    scheduledBackupRunning = true;
    await exclusive(async () => {
      requireGeneration(state.session_generation);
      if (remote.active() || lockWhenIdle || native.hostingActive()) return;
      try {
        await native.scheduledBackupAsync();
      } finally {
        // An automatic backup can finish after the existing idle timer became
        // due. Apply that lock in exclusive's finalizer before admitting work.
        if (powerMonitor.getSystemIdleTime() >= IDLE_LOCK_SECONDS) lockNow('idle');
      }
    });
  } catch (e) {
    if (diagnostics.panic(e)) {
      clearInterval(scheduledBackupTimer);
      scheduledBackupTimer = undefined;
      const url = win && !win.isDestroyed() ? win.webContents.getURL() : ORIGIN;
      showPanic(e, new URL(url || ORIGIN).pathname);
    } else if (e.code !== 'SESSION_CHANGED' && !['BUSY', BUSY_TEXT].includes(e.message)) {
      diagnostics.note('scheduled-backup', e);
    }
  } finally {
    scheduledBackupRunning = false;
  }
}
function wakeScheduledBackup(opts) {
  if (scheduledBackupTimer) void scheduledBackupTick(opts);
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

// Why the unlock screen is showing: '' (nothing special), 'missing' or 'unreadable' (the saved book key
// is not usable after a reinstall or move), 'keyring' (no working keyring this launch), 'ask' (the owner
// chose to be asked), 'locked' or 'idle' (locked by the user or after inactivity), 'choose' (pick a book).
let gateReason = '';

// fb-node refusals carry plain words written for the owner. Anything else (file system, keyring,
// Electron) is never shown: its text can be technical or name internals.
const GATE_FALLBACK = 'FreedomBooks could not finish opening these books. Nothing was changed. Try again, and if this keeps happening use “Report a problem”.';
function plainError(e) {
  const panic = diagnostics.panic(e);
  if (panic) return diagnostics.panicText(panic);
  const code = e && typeof e.code === 'string' ? e.code : '';
  return (code === 'GenericFailure' || code.startsWith('FB_')) && typeof e.message === 'string' && e.message ? e.message : GATE_FALLBACK;
}
const WRONG_UNLOCK = 'That didn’t unlock the books. Check Caps Lock, and remember this is the book password (or the recovery key from your Emergency Kit), not your sign-in password.';

// What the gate says about this computer: why it shows, which book file it opens and when that file last
// changed. Paths are shown to the owner (home folder as ~); nothing secret is included.
function gateContext() {
  const context = {reason: gateReason, book: '', books: [], home: require('node:os').homedir()};
  try {
    context.book = store.bookPath();
    context.books = store.books();
  } catch (e) { diagnostics.note('gate-context', e); /* the screen still works without the file details */ }
  return context;
}
// setup only when this computer is known to have no books at all; a saved choice that can't be
// followed, or a missing main book beside other books, asks the owner to choose instead of starting
// new books. Anything that can't be read fails closed: an error page, never first-time setup.
const BOOKS_UNREADABLE = 'FreedomBooks couldn’t read the list of books on this computer: the books folder, or a folder inside it, can’t be opened. Nothing was changed or deleted. Check that folder’s permissions, then start FreedomBooks again. If this keeps happening use “Report a problem”.';
function gateProblem() {
  if (bookChangeError) return plainError(bookChangeError);
  if (store.exists() && gateReason !== 'choose') return '';
  try { store.books(); return ''; } catch { return BOOKS_UNREADABLE; }
}
function gateMode() {
  if (gateProblem()) return 'error';
  if (store.exists() && gateReason !== 'choose') return 'unlock';
  return store.books().length ? 'choose' : 'setup';
}
function gate(errors = [], starterChart = 'standard') {
  const mode = gateMode();
  const shown = mode === 'error' ? [gateProblem(), ...errors.filter(e => e !== gateProblem())] : errors;
  return native.renderGate(mode, JSON.stringify(gateContext()), JSON.stringify(shown), starterChart);
}

// After any open: remember which book is open, and keep the saved book key in step with the owner's
// choice. A keyring that can't seal right now changes nothing on disk; the password is asked next time.
function syncBookKeys() {
  const state = JSON.parse(native.appState());
  if (!state.open || !state.path) return;
  gateReason = '';
  // Only the main book is the main book. Anything else opened (a restored copy, a set-aside book, or
  // any book while the saved choice is missing or damaged) is a labelled copy until the owner makes
  // it the main books with fb-app's confirmed, audited "Use this as my books".
  store.remember(state.path);
  const copy = !store.isMain(state.path);
  native.markCopy(copy);
  // A committed change of main books whose audit event a restart still owes is recorded now.
  try { native.finishBookChange(); } catch (e) { diagnostics.note('finish-book-change', e); /* tried again after the next command or open */ }
  const protection = JSON.parse(native.protectionState(state.path));
  if (protection.known) { // Otherwise preserve an older OS-only key until recovery setup finishes.
    if (state.builtin || protection.ask_password) {
      try { store.removeSealedKey(); } catch (e) { diagnostics.note('remove-sealed-key', e); /* the owner's prompt still applies: the record says ask */ }
    } else if (!copy && store.keyringWorks()) {
      // Only the main books open by themselves at start, so only their key is saved here. A copy's
      // saved key would only ever be a way to open it without its password (see app.chooseBook).
      try { store.sealBook(native.keyForSealing()); } catch (e) { diagnostics.note('seal-book-key', e); /* nothing removed; asked next time */ }
    }
  }
  const item = Menu.getApplicationMenu()?.getMenuItemById('lock-books');
  if (item) item.enabled = state.can_lock || state.signed_in;
  const use = Menu.getApplicationMenu()?.getMenuItemById('use-restored');
  if (use) use.enabled = copy;
}

// "Use this as my books", after the owner confirmed it on the restored-book page. fb-app does the
// whole change under a journal: it sets the previous main book aside (moved, never deleted), points
// the saved choice at the open book, and writes the audit event only once the change is committed.
// A refusal or failure says truthfully what is where; the shell then changes nothing.
function useRestored() {
  const state = JSON.parse(native.appState());
  if (!state.open || !state.path || state.path !== store.bookPath() || store.isMain(state.path)) {
    throw new Error('These are already your books.');
  }
  const main = store.mainBook();
  const previous = main && main !== state.path && fs.existsSync(main) ? main : '';
  const made = JSON.parse(native.useRestoredCopy(previous));
  store.adoptMain(made.main);
  native.markCopy(false);
  try { syncBookKeys(); } catch (e) {
    if (diagnostics.panic(e)) throw e;
    diagnostics.note('sync-keys', e); // the change is made; save the key after the next unlock
  }
  return {outcome:'redirect', location:'/settings/restored-book/?done=1'};
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
  if (url.pathname === diagnostics.PROBLEM_PATH) return diagnostics.lastPage() ? page(diagnostics.lastPage(), 500) : Response.redirect(`${ORIGIN}/`, 303);
  const marker = url.searchParams.get(REMOTE.PAGE_KEY);
  const query = marker === null ? url.search : '?' + url.search.slice(1).split('&').filter(part => !new URLSearchParams(part).has(REMOTE.PAGE_KEY)).join('&');
  const target = url.pathname + (query === '?' ? '' : query);
  const forRemote = () => marker !== null || remote.active() || remote.handles(url.pathname);
  const renderRemote = async () => {
    try { return await remotePage(target, url, marker); }
    catch (e) { diagnostics.unexpected('remote-page', e); return page(require('./remote-mode.cjs').noticePage('Something went wrong', 'The page could not be shown. Try again.'), 500); }
  };
  if (forRemote()) return renderRemote();
  // Several page loads may be waiting on the same operation. Recheck admission after
  // every await so a newly admitted render cannot overlap another request.
  const waitedForWork = !!busy;
  while (busy) await busy.catch(() => {});
  if (forRemote()) return Response.redirect(ORIGIN + remote.currentPage(), 303);
  const logo = /^\/c\/([0-9a-f-]{36})\/document-logo\/$/.exec(url.pathname);
  if (logo) {
    try {
      if (!JSON.parse(native.appState()).open) return new Response('Not found', {status:404});
      const data = JSON.parse(native.query(JSON.stringify({type:'CompanyLogo',company_id:logo[1]})));
      if (!data || !['image/png','image/jpeg'].includes(data.mime)) return new Response('Not found', {status:404});
      return new Response(Buffer.from(data.base64,'base64'), {headers:{'content-type':data.mime,'cache-control':'no-store','x-content-type-options':'nosniff','content-security-policy':guards.CSP}});
    } catch (e) { diagnostics.note('logo', e); return new Response('Not found', {status:404}); }
  }
  if (pending && pending.generation !== undefined && pending.generation !== JSON.parse(native.appState()).session_generation) pending = null;
  if (pending && pending.path === target) {
    const { html, status } = pending;
    pending = null;
    return page(html, status);
  }
  if (!JSON.parse(native.appState()).open) return page(gate(), 200);
  if (hosting.handles(url.pathname)) {
    try { return page(hosting.render()); }
    catch (e) { diagnostics.unexpected('hosting-page', e); return page(gate([plainError(e)]), 500); }
  }
  try {
    const generationBeforeRender = remoteGeneration;
    // Automation has its own write-classified native boundary, separate from page reads.
    // The timer also catches up while the user leaves a page open across a due date.
    if (!waitedForWork && /^\/c\/[0-9a-f-]{36}\//.test(url.pathname) && JSON.parse(native.appState()).signed_in) {
      const generation = JSON.parse(native.appState()).session_generation;
      await exclusive(() => native.recurringBillsAsync());
      requireGeneration(generation);
    }
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
    if (r.status === 200 && url.pathname === '/desktop/users/') r.html = hosting.usersPage(r.html);
    // The remembered approval cannot act until native rendering has authenticated this company.
    const aiCompany = /^\/c\/([0-9a-f-]{36})\//.exec(url.pathname)?.[1];
    if (r.status === 200 && aiCompany) {
      const generation = JSON.parse(native.appState()).session_generation;
      const aiGeneration = native.aiGeneration();
      await localAI.resume(aiCompany);
      requireGeneration(generation);
      if (aiGeneration !== native.aiGeneration()) throw Object.assign(new Error('The active company changed. Open the screen again.'), {code:'SESSION_CHANGED'});
    }
    if (remoteGeneration !== generationBeforeRender) return Response.redirect(ORIGIN + remote.currentPage(), 303);
    // Direct CSV requests must never parse user-controlled cells as HTML. QueryDict uses
    // the last value of a repeated key; guards.csvExportFilename enforces that rule for
    // both local and connected-host pages.
    return guards.csvExportResponse(r.route, url, r.status, r.html) ?? page(r.html, r.status);
  } catch (e) {
    if (remote.active()) return Response.redirect(ORIGIN + remote.currentPage(), 303);
    if (e.code === 'SESSION_CHANGED') {
      return JSON.parse(native.appState()).open ? Response.redirect(ORIGIN+'/desktop/sign-in/',303) : page(gate(),200);
    }
    if (String(e.message) === 'LOCKED') return page(gate(), 200);
    const panic = diagnostics.panic(e);
    if (panic) return page(afterPanic(panic, url.pathname), 500);
    diagnostics.unexpected('page', e);
    return page(gate([plainError(e)]), 404);
  }
}

// CRASHLOG: fb-node caught a panic and turned it into an error (see diagnostics.cjs). When it closed
// the books (after anything that could have changed them), open them again the way startup does if
// this computer can, so the owner signs in or unlocks as usual; then say what happened.
function afterPanic(info, pathname) {
  if (info.closed) {
    pending = null;
    gateReason = '';
    startupHome = '';
    if (!busy && info.was_open) {
      try { startupOpen(true); } catch (e) { diagnostics.note('reopen', e); }
      try { syncBookKeys(); } catch (e) { diagnostics.note('sync-keys', e); }
    }
  }
  let state = {open: false};
  try { state = JSON.parse(native.appState()); } catch (e) { diagnostics.note('app-state', e); }
  let version = '';
  try { version = JSON.parse(native.buildInfo()).version; } catch (e) { diagnostics.note('build-info', e); }
  return diagnostics.remember(info, {open: state.open === true, version}, pathname);
}

function showPanic(error, pathname = '') {
  const info = diagnostics.panic(error);
  if (!info) return false;
  afterPanic(info, pathname);
  if (win && !win.isDestroyed()) win.loadURL(`${ORIGIN}${diagnostics.PROBLEM_PATH}`);
  return true;
}

function lockNow(reason = 'locked') {
  // Client mode owns the lock page and any reconnection it needs.
  if (remote.active()) return remote.lock().catch(() => {});
  // Hosting: the host person's lock (Ctrl+L, idle, suspend, screen lock) signs out only them; the
  // books stay open for the people connected. Closing the books or quitting stops hosting. It never
  // waits for the books (a connected person's request may hold them, or a desktop task may be
  // running): fb-node records the Lock at once and applies it before anything else runs for this
  // window, and the sign-in screen replaces the page now (review S1).
  let hostingNow = false;
  try { hostingNow = native.hostingActive(); } catch (e) { diagnostics.note('hosting-lock', e); }
  if (hostingNow) {
    try { native.signOut(); } catch (e) { diagnostics.note('hosting-lock', e); }
    lockWhenIdle = false;
    pending = null;
    if (win && !win.isDestroyed()) win.loadURL(`${ORIGIN}/desktop/sign-in/`);
    return;
  }
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
  gateReason = reason === 'idle' ? 'idle' : 'locked';
  pending = null;
  if (win && !win.isDestroyed()) win.loadURL(`${ORIGIN}/`);
}

// The fixed call list (preload exposes exactly these). Each handler checks the sender, then the payload.
const calls = {
  'books.run': async (payload, _event, marker = null) => {
    const refused = remote.admit(marker);
    if (refused) return refused;
    if (busy) throw new Error(BUSY_TEXT);
    const command = guards.command(payload, { plugins: PLUGINS, bankFeeds: BANK_FEEDS });
    if (marker !== null) return remote.run(command, marker);
    const managedDownload = command.type === 'ExportBackup' && typeof command.name === 'string' && command.name.startsWith('scheduled.');
    const long = managedDownload || (command.type === 'SubmitForm' && (
      LONG_COMMAND.test(command.path) ||
      (BACKUPS_PAGE.test(command.path) && command.fields.some(([key,value]) => key === 'scheduled_archive' && value)) ||
      (INVOICE_DELIVERY && command.fields.some(([k,v]) => k === 'action' && (v.startsWith('gmail_') || v.startsWith('stripe_'))))
    ));
    if (!connectionKeysConfigured && command.type === 'SubmitForm') {
      const state = JSON.parse(native.appState());
      if (state.signed_in && needsConnectionKey(command, state.builtin)) configureConnectionKeys();
      else if (command.path === '/desktop/sign-in/') configureForBook();
    }
    let outcome;
    try { outcome = JSON.parse(long ? await exclusive(() => native.runAsync(JSON.stringify(command))) : native.run(JSON.stringify(command))); }
    catch(error) {
      if(error.code === 'SESSION_CHANGED') return {outcome:'redirect',location:sessionLocation()};
      throw error;
    }
    const generation = JSON.parse(native.appState()).session_generation;
    if (long && !JSON.parse(native.appState()).open) return {outcome:'redirect',location:'/'};
    try { syncBookKeys(); } catch (e) { diagnostics.note('sync-keys', e); /* the command finished; the key is saved after the next unlock */ }
    if (outcome.outcome === 'done' && outcome.data?.desktop_keys === 'copy-recovery') {
      // The path-bound form and native authorization both have to allow this export.
      // Copying is not a durable backup and must not mark the recovery key as saved.
      try { clipboard.writeText(native.recoveryKey()); outcome.data.copied = true; }
      catch (e) { diagnostics.note('copy-recovery', e); outcome.data.copy_error = 'The recovery key could not be copied. Try again, or save it to a file.'; }
      return outcome;
    }
    if (outcome.outcome === 'done' && typeof outcome.data?.invoice_email_pdf === 'string') {
      if (!INVOICE_DELIVERY) throw new Error('Invoice email is unavailable.');
      return require('./invoice-email.cjs').controller(native,STATIC_ROOT,exclusive)(outcome.data.invoice_email_pdf);
    }
    if (outcome.outcome === 'done' && outcome.data?.desktop_keys === 'save-recovery') return saveRecovery();
    if (outcome.outcome === 'done' && outcome.data?.desktop_keys === 'use-restored') return useRestored();
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
      if (!PLUGINS && !INVOICE_DELIVERY) throw new Error('Refused: link not allowed.');
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
    if (scheduledBackupTimer && command.type === 'SubmitForm' && outcome.outcome === 'redirect' &&
        ['/desktop/sign-in/', '/desktop/mfa/'].includes(command.path)) setImmediate(wakeScheduledBackup);
    return outcome;
  },
  // Only this computer's own connect and disconnect screens may pair, connect, forget or
  // disconnect: never a page whose content came from a host.
  'remote.pair': async (payload, event) => {
    remoteSender(event, [REMOTE.CONNECT]);
    const input = guards.remotePair(payload);
    closeLocalForRemote();
    return remoteResult(await remote.pair(input));
  },
  'remote.connect': async (_payload, event) => {
    remoteSender(event, [REMOTE.CONNECT]);
    closeLocalForRemote();
    return remoteResult(await remote.connectSaved());
  },
  'remote.disconnect': async (_payload, event) => { remoteSender(event, [REMOTE.CONNECT, REMOTE.CONNECTING, REMOTE.DISCONNECT]); return {outcome: 'redirect', location: await disconnectToBooks()}; },
  'remote.forget': async (_payload, event) => { remoteSender(event, [REMOTE.CONNECT]); await remote.forget(); return {outcome: 'redirect', location: '/remote/connect/'}; },
  // Hosting controls: only from this computer's own Hosting screen; fb-node checks the Administrator.
  'hosting.on': async (payload, event) => { remoteSender(event, [hosting.PATH]); if (remote.active()) throw new Error('Disconnect from the host first.'); return hosting.on(guards.hostingOn(payload)); },
  'hosting.poll': (_payload, event) => {
    remoteSender(event, [hosting.PATH]);
    return busy || remote.active() ? null : hosting.poll();
  },
  'hosting.off': async (_payload, event) => { remoteSender(event, [hosting.PATH]); return hosting.off(); },
  'hosting.pair': async (_payload, event) => { remoteSender(event, [hosting.PATH]); return hosting.pair(); },
  'hosting.cancel': async (_payload, event) => { remoteSender(event, [hosting.PATH]); return hosting.cancel(); },
  'hosting.revoke': async (payload, event) => { remoteSender(event, [hosting.PATH]); return hosting.revoke(guards.hostingRevoke(payload)); },
  'hosting.repair': async (payload, event) => { remoteSender(event, [hosting.PATH]); return hosting.repair(guards.hostingRepair(payload)); },
  'hosting.restart': async (_payload, event) => { remoteSender(event, [hosting.PATH]); return hosting.restart(); },
  'app.createBook': async (payload) => {
    if (busy) throw new Error(BUSY_TEXT);
    const input = guards.setup(payload);
    // Only first-time setup creates books: never beside books this computer has (or can't list).
    if (store.exists() || gateMode() !== 'setup') throw new Error('Books already exist on this computer. Choose them on the unlock screen, or restore a backup as a copy.');
    const errors = [];
    if (!input.owner_name.trim()) errors.push("Enter the owner's name.");
    if (!input.company_name.trim()) errors.push('Enter the company name.');
    const protect = input.protect === 'on';
    if (protect && input.lock_password !== input.lock_password2) errors.push('The two passwords differ.');
    if (errors.length) return failGate(errors, input.starter_chart);
    try {
      if (protect) configureConnectionKeys();
      const created = JSON.parse(await exclusive(() => native.createProtectedBookAsync(store.bookPath(), input.owner_name, input.company_name, input.lock_password, protect, input.starter_chart)));
      try { syncBookKeys(); } catch (e) { diagnostics.note('sync-keys', e); /* the new book is open; its key is saved after the next unlock */ }
      return {outcome:'redirect',location:created.home};
    } catch (e) { return failGate([plainError(e)], input.starter_chart); }
  },
  'app.restoreBook': async (payload) => {
    if (busy) throw new Error(BUSY_TEXT);
    const input = guards.restoreBook(payload);
    if (gateMode() === 'error') return failGate([]);
    const selected = await dialog.showOpenDialog(win, {title:'Choose a backup to restore', properties:['openFile'], filters:[{name:'FreedomBooks backups',extensions:['fbbackup']}]});
    if (selected.canceled || !selected.filePaths[0]) return {outcome:'done'};
    let opened;
    try { opened = JSON.parse(await exclusive(() => native.restoreBookFromGateAsync(selected.filePaths[0], input.name || 'Recovered books', input.secret))); }
    catch (e) { return failGate([plainError(e)]); }
    configureForBook();
    try { syncBookKeys(); } catch (e) { diagnostics.note('sync-keys', e); /* the restored book is open; its key is saved after the next unlock */ }
    return {outcome:'redirect',location:opened.home || '/'};
  },

  // The issue page opens in the system browser; only guards.EXTERNAL_URLS, matched exactly.
  'app.openExternal': async (payload) => {
    await shell.openExternal(guards.external(payload));
    return { outcome: 'done' };
  },
  // The unlock screen's list of books only points the unlock screen at another book. It never opens
  // one through a saved key, so a lock (by the owner or after inactivity) or "ask every time" can't be
  // stepped around by choosing a different book: the chosen book asks for its own book password.
  // Only a built-in-key book, which has no book password, opens straight away (sign-in still applies).
  'app.chooseBook': (payload) => {
    if (busy) throw new Error(BUSY_TEXT);
    const input = guards.chooseBook(payload);
    if (JSON.parse(native.appState()).open) throw new Error('Lock the open books first.');
    if (gateMode() === 'error') return failGate([]);
    try { store.select(input.path); }
    catch (e) { return failGate([plainError(e)]); }
    const locked = ['locked', 'idle'].includes(gateReason) ? gateReason : '';
    gateReason = '';
    pending = null;
    startupHome = '';
    startupOpen(true, false);
    if (!startupHome && locked && gateReason === 'password') gateReason = locked;
    if (startupHome) {
      try { syncBookKeys(); } catch (e) { diagnostics.note('sync-keys', e); /* opened; the key is saved after the next unlock */ }
      void hosting.resume();
      return {outcome:'redirect', location:startupHome};
    }
    return {outcome:'redirect', location:'/'};
  },
  'app.unlock': (payload) => {
    const input = guards.unlock(payload);
    if (gateMode() === 'error') return failGate([]);
    let opened;
    try {
      // An interrupted key change is recovered before every open, not only at startup.
      native.recoverKeyChange(store.bookPath());
      const state = JSON.parse(native.protectionState(store.bookPath()));
      if (state.known) opened = JSON.parse(native.openProtectedBook(store.bookPath(), input.password || ''));
      else if (store.hasPasswordWrap()) opened = JSON.parse(native.openBookWithPassword(store.bookPath(), store.readWrap(), input.password || ''));
      else {
        opened = keys.openLegacyBook(native, store, input.password || '');
      }
    } catch (e) {
      const wrongKey = gateReason === 'builtin' ? 'The built-in key could not open these books.' : WRONG_UNLOCK;
      return failGate([e && e.code === 'FB_BOOK_WRONG_KEY' ? wrongKey : plainError(e)]);
    }
    // The books are open: saving the key for next time must never turn into an unlock failure.
    try { syncBookKeys(); } catch (e) { diagnostics.note('sync-keys', e); /* asked again next time; nothing removed */ }
    void hosting.resume();
    if (scheduledBackupTimer) setImmediate(wakeScheduledBackup);
    return { outcome: 'redirect', location: opened.home || '/' };
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
  } else if (desktop === 'pick-bank-rules') {
    const r = await dialog.showOpenDialog(win, { title: 'Import QuickBooks bank rules', properties: ['openFile'], filters: [{ name: 'QuickBooks rule export', extensions: ['xls'] }] });
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

// While connected to a host, this computer's own books stay closed: a stale gate page cannot open them.
for (const name of ['app.createBook', 'app.restoreBook', 'app.chooseBook', 'app.unlock']) {
  const local = calls[name];
  calls[name] = (payload) => {
    if (remote.active()) throw new Error('Disconnect from the host first (File › Disconnect from host).');
    return local(payload);
  };
}

// CRASHLOG: Report a problem's two buttons (open the log folder, copy the diagnostic summary).
Object.assign(calls, diagnostics.calls());
// Back, Forward, Home, Find and the Go menu (navigation.cjs). The title bar's Back and Forward buttons
// come through this one call; the preload sends only {to:'back'|'forward'}.
// While connected to a host, navigation follows the remote connection (the host enforces sign-in).
const navigationState = { appState: () => remote.active() ? JSON.stringify({open: true, signed_in: true, session_generation: 1e9 + remoteGeneration}) : native.appState() };
const navigation = require('./navigation.cjs').controller({ native: navigationState, window: () => win, menu: () => Menu.getApplicationMenu() });
calls['app.navigate'] = (payload) => navigation.navigate(payload);

for (const [name, fn] of Object.entries(calls)) {
  ipcMain.handle(`fb:${name}`, async (event, payload, marker) => {
    guards.sender(event, win);
    try {
      if (marker && !['books.run', 'app.navigate', 'app.openExternal'].includes(name)) throw new Error('This can only be done on the host computer.');
      return { ok: true, value: await fn(payload, event, marker) };
    } catch (e) {
      if (showPanic(e, payload && typeof payload.path === 'string' ? payload.path : '')) {
        // The window shows what happened; the form's own handler follows the same address.
        return { ok: true, value: { outcome: 'redirect', location: diagnostics.PROBLEM_PATH } };
      }
      diagnostics.unexpected(name, e);
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
  navigation.attach(win);
  require('./edit-menu.cjs').attach(win, Menu);
  win.once('ready-to-show', () => win.show());
  // The window title says which host's books are showing.
  win.on('page-title-updated', (event, title) => {
    if (!hostingTitle) return;
    event.preventDefault();
    win.setTitle(windowTitle(title));
  });
  const state = JSON.parse(native.appState());
  const companies = state.signed_in ? JSON.parse(native.query(JSON.stringify({type:'Companies'}))) : [];
  // A computer with no books of its own that is paired with a host starts connected to it.
  let remoteStart = false;
  try { remoteStart = !state.open && !startupHome && gateMode() === 'setup' && !!remoteStore.summary(); } catch { /* local start */ }
  win.loadURL(`${ORIGIN}${remoteStart ? '/remote/connect/?auto=1' : startupHome || (companies.length ? '/c/'+companies[0].id+'/home/' : '/')}`);
}

app.whenReady().then(() => {
  guards.session(session.defaultSession);
  session.defaultSession.protocol.handle('fb', handle);
  // macOS shows the first menu under the app's name: give it the standard app menu (Hide, Quit).
  const mac = require('node:os').platform() === 'darwin';
  Menu.setApplicationMenu(Menu.buildFromTemplate([
    ...(mac ? [{ role: 'appMenu' }] : []),
    { label: 'File', submenu: [{ id:'remote-connect', label:'Connect to a FreedomBooks host…', click:()=>{ if (win && !win.isDestroyed()) win.loadURL(ORIGIN+'/remote/connect/'); } }, { id:'remote-disconnect', label:'Disconnect from host', enabled:false, click:async()=>{ const location = await disconnectToBooks(); if (win && !win.isDestroyed()) win.loadURL(ORIGIN+location); } }, { type:'separator' }, ...printing.menuItems, { type:'separator' }, { id:'lock-books', label:'Lock', enabled:false, accelerator:'CmdOrCtrl+L', click:()=>lockNow('locked') }, { id:'use-restored', label:'Use this copy as my books…', enabled:false, click:()=>{if(JSON.parse(native.appState()).open) win.loadURL(ORIGIN+'/settings/restored-book/');} }, ...(mac ? [] : [{ type: 'separator' }, { role: 'quit' }])] },
    { label:'Settings', submenu:[...localAI.menuItems,{type:'separator'},{label:'Multi-factor authentication…',click:()=>{if(remote.active() || JSON.parse(native.appState()).open) win.loadURL(ORIGIN+'/desktop/mfa/');}},{label:'Users…',click:()=>{if(remote.active() || JSON.parse(native.appState()).open) win.loadURL(ORIGIN+'/desktop/users/');}},{label:'Switch user / sign out…',click:()=>{if(remote.active() || JSON.parse(native.appState()).open) win.loadURL(ORIGIN+'/desktop/session/');}},{id:'book-protection',label:'Book protection…',click:()=>{if(JSON.parse(native.appState()).open) win.loadURL(ORIGIN+'/settings/book-protection/');}},{type:'separator'},{id:'host-books',label:'Host these books…',click:()=>{if(!remote.active() && JSON.parse(native.appState()).open) win.loadURL(ORIGIN+hosting.PATH);}}] },
    { label: 'Edit', submenu: [{ role: 'undo' }, { role: 'redo' }, { type: 'separator' }, { role: 'cut' }, { role: 'copy' }, { role: 'paste' }, { role: 'selectAll' }] },
    { label: 'View', submenu: [{ role: 'resetZoom' }, { role: 'zoomIn' }, { role: 'zoomOut' }] },
    navigation.go,
    require('./navigation.cjs').helpMenu({ app, dialog, native, window: () => win }),
  ]));
  const recurringTimer = setInterval(async () => {
    if (busy || !win || win.isDestroyed()) return;
    try {
      const state = JSON.parse(native.appState());
      if (state.open && state.signed_in) await exclusive(() => native.recurringBillsAsync());
    } catch (e) {
      if (diagnostics.panic(e)) { clearInterval(recurringTimer); showPanic(e, new URL(win.webContents.getURL()).pathname); }
      else if (e.code !== 'SESSION_CHANGED' && !['BUSY', BUSY_TEXT].includes(e.message)) diagnostics.note('recurring-bills', e);
    }
  }, 30 * 1000);
  recurringTimer.unref();
  if (INVOICE_DELIVERY) {
    const timer=setInterval(async () => {
      if (busy || !win || win.isDestroyed()) return;
      const pathname=new URL(win.webContents.getURL()).pathname;
      try {
        const state=JSON.parse(native.appState()); if(!state.open || !state.signed_in) return;
        const match=pathname.match(/^\/c\/([0-9a-f-]{36})\//);
        if(match) await exclusive(()=>native.stripeRefreshAsync(match[1]));
      } catch (e) {
        if (diagnostics.panic(e)) {
          // Do not repeat a failing background write every thirty seconds this launch.
          clearInterval(timer);
          showPanic(e, pathname);
        } else if (e.code !== 'SESSION_CHANGED' && e.message !== 'BUSY') diagnostics.note('stripe-refresh',e);
      }
    }, 30 * 1000); timer.unref();
  }
  startupOpen();
  void localAI.start().catch(error=>diagnostics.note('ai-start',error));
  createWindow();
  require('./power-lock.cjs').watch({powerMonitor, window: win, remote, native, busy: () => !!busy, lockNow, idleSeconds: IDLE_LOCK_SECONDS});
  if (typeof native.scheduledBackupAsync === 'function') {
    scheduledBackupTimer = setInterval(wakeScheduledBackup, 30_000);
    scheduledBackupTimer.unref();
    powerMonitor.on('resume', wakeScheduledBackup);
    powerMonitor.on('unlock-screen', wakeScheduledBackup);
  }
  try { syncBookKeys(); } catch (e) { diagnostics.note('sync-keys', e); /* the window still opens; the key is saved after the next unlock */ }
  // Hosting left on by an Administrator starts again once the books are open (it says why if not),
  // and stops if its address leaves this computer.
  if (JSON.parse(native.appState()).open) void hosting.resume();
  hosting.watch();
});

// Startup, in an order where no step can skip another:
//  1. finish or roll back an interrupted protection change, before anything opens the book;
//  2. open the book if this computer can (built-in key, or a saved key the keyring can read);
//     otherwise record why the unlock screen is showing;
//  3. prepare saved connections for sign-in. Offline built-in books defer the keyring.
// `savedKey` false (the unlock screen's list of books): a saved key is never used; the book asks.
function startupOpen(chosen = false, savedKey = true) {
  const errors = [];
  try {
    if (bookChangeError) return; // nothing opens on a guess; the unlock screen says why
    if (!chosen && ['missing', 'unreadable'].includes(store.activeChoice())) { gateReason = 'choose'; return; }
    if (!store.exists()) return;
    try { native.recoverKeyChange(store.bookPath()); }
    catch (e) { errors.push(plainError(e)); return; }
    const state = JSON.parse(native.protectionState(store.bookPath()));
    if (state.known && state.ask_password) {
      try { store.removeSealedKey(); } catch (e) { diagnostics.note('remove-sealed-key', e); /* the record still asks every time */ }
    }
    if (state.builtin) {
      gateReason = 'builtin';
      try { startupHome = JSON.parse(native.openProtectedBook(store.bookPath(), '')).home; }
      catch (e) { if (e.code !== 'FB_BOOK_WRONG_KEY') errors.push(plainError(e)); }
      return;
    }
    if (!state.known && !store.hasPasswordWrap()) {
      // A copied built-in-key file needs no local metadata or OS store to open.
      try {
        const opened = keys.openLegacyBook(native, store, '', savedKey);
        if (opened) startupHome = opened.home;
      } catch (e) {
        if (e && e.code === 'FB_BOOK_WRONG_KEY') gateReason = !savedKey ? '' : store.keyringWorks() ? 'missing' : 'keyring';
        else errors.push(plainError(e));
      }
      return;
    }
    if (!state.known) { gateReason = 'ask'; return; }
    if (!savedKey) { gateReason = state.ask_password ? 'ask' : 'password'; return; }
    const status = store.bookKeyStatus(state.ask_password);
    if (status === 'unreadable') {
      // A saved key set aside earlier (the keyring may have answered differently one launch) that
      // this keyring opens and that opens these books is put back; anything else still asks.
      for (const candidate of store.bookKeyCandidates()) {
        try { startupHome = JSON.parse(native.openBook(store.bookPath(), candidate.key)).home; }
        catch (e) { if (e && e.code === 'FB_BOOK_WRONG_KEY') continue; errors.push(plainError(e)); return; }
        try { store.restoreBookKey(candidate.file); } catch { /* opened; the key is saved again after this open */ }
        return;
      }
    }
    if (status !== 'present') { gateReason = status; return; }
    try { startupHome = JSON.parse(native.openBook(store.bookPath(), store.readSealedKey())).home; }
    catch (e) {
      if (e && e.code === 'FB_BOOK_WRONG_KEY') gateReason = 'unreadable';
      else if (e && e.message === keys.SEALED_UNREADABLE) gateReason = 'unreadable';
      else errors.push(plainError(e));
    }
  } catch (e) {
    errors.push(plainError(e));
  } finally {
    configureForBook();
    if (errors.length) pending = {path:'/', html:gate(errors), status:400};
  }
}
// A leftover installation key alone says nothing about the open book. Older keys stay on
// disk for the next book or connection that actually uses them.
function configureForBook() {
  if (connectionKeysConfigured) return;
  try {
    const state = JSON.parse(native.appState());
    if (state.open ? !state.builtin || native.hasCredentials()
        : gateReason !== 'builtin' && (store.exists() || store.hasConnectionKeyMaterial())) configureConnectionKeys();
  } catch (e) { diagnostics.note('connection-setup', e); }
}

function needsConnectionKey(command, builtin) {
  const action = command.fields.findLast(([name]) => name === 'action')?.[1];
  if (command.path === '/settings/book-protection/') return ['add', 'adopt', 'ask'].includes(action);
  if (PLUGINS && command.path === '/auth/openai/start') return !builtin;
  const section = /^\/c\/[0-9a-f-]{36}\/(.*)$/.exec(command.path)?.[1];
  if (!section) return false;
  if (BANK_FEEDS && section === 'bank-feeds/connect/') return !builtin;
  if (BANK_FEEDS && /^bank-feeds\/[0-9a-f-]{36}\/sync\/$/.test(section)) return true;
  return INVOICE_DELIVERY && section === 'edit/' &&
    ['gmail_connect', 'gmail_poll', 'stripe_connect'].includes(action);
}

function configureConnectionKeys() {
  if (connectionKeysConfigured) return;
  let found;
  try { found = store.connectionKey(); }
  catch (e) { diagnostics.note('connection-key', e); found = {key:null, state:keys.CONNECTION.UNAVAILABLE}; }
  native.configureKeys(found.key || '', found.state, store.keyringWorks(), JSON.stringify(found.alternates || []));
  connectionKeysConfigured = found.state !== keys.CONNECTION.UNAVAILABLE;
}

app.on('second-instance', () => {
  if (win) {
    if (win.isMinimized()) win.restore();
    win.focus();
  }
});
app.on('window-all-closed', () => {
  // Connected: sign out on the host first (waited for briefly), so it does not show this person
  // signed in until the idle limit.
  const done = () => { if (!busy) native.lock(); app.quit(); };
  if (remote.active()) remote.disconnect().then(done, done); else done();
});
// DESKTOP-BACKUPS: app.quit() (File › Quit) skips window-all-closed; close the open book cleanly too.
// BACKUPS-POLISH: a running backup or restore finishes first, then the book closes and the app quits.
app.on('will-quit', (event) => {
  if (busy) {
    event.preventDefault();
    busy.catch(() => {}).then(() => app.quit());
    return;
  }
  void localAI.stop();
  native.lock();
});

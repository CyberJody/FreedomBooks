'use strict';
// CRASHLOG: the main process's side of the problem log. fb-node (fb-diag) owns the file: it writes
// Rust's entries and these, scrubs every field the same way, rotates and keeps it owner-only. This
// module picks the folder, forwards main-process failures (swallowed errors, uncaught exceptions,
// unhandled rejections), recognises fb-node's FB_PANIC errors and builds the page shown after one.
const path = require('node:path');
const os = require('node:os');

const PANIC = 'FB_PANIC ';
const PROBLEM_PATH = '/desktop/problem/';
// The pages whose buttons may open the log folder or copy the summary.
const LOG_PAGES = Object.freeze(['/help/report/', PROBLEM_PATH]);

/** The log folder: the isolated profile in development (FB_DESKTOP_USER_DATA), else the state folder. */
function logDir(app, {env = process.env, platform = process.platform, development = require('./development.cjs')} = {}) {
  const profile = development.userData(app, env);
  if (profile) {
    if (!path.isAbsolute(profile)) throw new Error('FB_DESKTOP_USER_DATA must be an absolute path');
    return path.join(profile, 'logs');
  }
  if (platform === 'linux') {
    const base = env.XDG_STATE_HOME && path.isAbsolute(env.XDG_STATE_HOME) ? env.XDG_STATE_HOME : path.join(os.homedir(), '.local', 'state');
    return path.join(base, 'FreedomBooks', 'logs');
  }
  return app.getPath('logs');
}

/** The folder as shown to the owner: the home folder as ~. */
function shown(dir, home = os.homedir()) {
  if (home && (dir === home || dir.startsWith(home + path.sep))) return '~' + dir.slice(home.length);
  return dir;
}

/** fb-node's panic error, parsed: {closed, was_open, saved, op}; null for any other error. */
function panicInfo(e) {
  const message = e && typeof e.message === 'string' ? e.message : '';
  if (!message.startsWith(PANIC)) return null;
  try {
    const info = JSON.parse(message.slice(PANIC.length));
    return {closed: info.closed === true, was_open: info.was_open === true, saved: ['none', 'partial', 'unknown'].includes(info.saved) ? info.saved : 'unknown', op: typeof info.op === 'string' ? info.op : ''};
  } catch { return {closed: false, was_open: false, saved: 'unknown', op: ''}; }
}

const OPENING = new Set(['open_book', 'open_book_with_password', 'open_protected_book', 'recover_key_change', 'create_book', 'create_protected_book', 'create_protected_book_async', 'restore_book_from_gate', 'restore_book_from_gate_async']);
function doing(op) {
  if (op === 'render' || op === 'render_async' || op === 'query') return 'opening that page';
  if (OPENING.has(op)) return 'opening your books';
  return 'doing that';
}
function savedText(saved) {
  if (saved === 'none') return 'Your books are safe: nothing from this was saved.';
  if (saved === 'partial') return 'Your books are safe, but part of this may have been saved before the error. Check the page before you do it again.';
  return 'Your books are safe. Check the page before you do it again, in case part of it was saved.';
}

/** One line for the unlock or setup screen's error list. */
function panicText(info) {
  return `Something went wrong inside FreedomBooks while ${doing(info.op)}. ${savedText(info.saved)} The details were recorded in the problem log.`;
}

const escape = text => String(text).replace(/[&<>"']/g, c => ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'})[c]);

/** Where Continue goes after a problem on `pathname`: that company's Home (unless Home itself failed
 * or the books were closed), otherwise the start page. */
function continueFrom(pathname, info) {
  const company = /^\/c\/([0-9a-f-]{36})\//.exec(String(pathname || ''));
  if (!company || info.closed) return '/';
  const home = `/c/${company[1]}/home/`;
  return pathname === home ? '/' : home;
}

/** The page shown after a panic. `open`: the books are open after recovery; `folder`: shown log folder;
 * `back`: where Continue goes (see continueFrom). */
function problemPage(info, {open, folder, version, back = '/'}) {
  const after = !info.closed
    ? 'You can carry on using the rest of FreedomBooks.'
    : open ? 'To be safe, FreedomBooks closed your books and opened them again. Sign in to carry on.'
      : 'To be safe, FreedomBooks closed your books. Unlock them to carry on.';
  const log = folder
    ? `<p>The details were recorded in the problem log on this computer: <code>${escape(folder)}</code>. It holds no names, amounts, passwords or keys.</p>`
    : '<p>The details could not be recorded in the problem log.</p>';
  const report = open && !info.closed ? ' <a href="/help/report/">Report a problem</a>' : '';
  const button = folder ? ' <button type="button" class="secondary" id="report-open-log">Open log folder</button>' : '';
  return `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>FreedomBooks · Something went wrong</title><link rel="stylesheet" href="/static/ledger/app.css"><link rel="stylesheet" href="/static/ledger/hybrid.css"><link rel="stylesheet" href="/static/desktop/gate.css"></head>
<body class="shell gate-page"><header class="titlebar"><span class="brand">FreedomBooks <small>${escape(version || '')}</small></span><div class="operator">Something went wrong</div></header>
<main id="main" class="gate"><div class="gate-card problem-card">
<h1>Something went wrong</h1>
<p>FreedomBooks hit an internal error while ${doing(info.op)}. ${escape(savedText(info.saved))}</p>
<p>${escape(after)}</p>
${log}
<p class="problem-actions"><a class="button" href="${escape(back)}">Continue</a>${button}${report}</p>
<p class="help">If this keeps happening, tell us with Report a problem and say what you were doing just before.</p>
</div></main></body></html>`;
}

function refusal(e) {
  const code = e && typeof e.code === 'string' ? e.code : '';
  return code === 'GenericFailure' || code.startsWith('FB_');
}

// Only engine-shaped coordinates within this app cross the native log boundary.
// Drop the message, function names and every external path before serialization.
function appFrames(stack) {
  const root = path.resolve(__dirname, '..').replace(/\\/g, '/') + '/';
  return String(stack || '').split('\n').slice(1).flatMap(line => {
    const frame = /^\s+at (?:[^()]+ \()?(.+):(\d+):(\d+)\)?$/.exec(line);
    if (!frame) return [];
    const file = frame[1].replace(/\\/g, '/');
    if (!file.startsWith(root)) return [];
    const relative = file.slice(root.length);
    if (!/^(main|preload)\/[a-z-]{1,30}\.(cjs|js|mjs)$/.test(relative)) return [];
    const row = Number(frame[2]), col = Number(frame[3]);
    return row > 0 && row <= 1000000 && col <= 1000000 ? [{file: relative, line: row, col}] : [];
  }).slice(0, 24);
}

function create({native, dir, shell, clipboard, dialog, window, processRef = process}) {
  let folder = '';
  try {
    native.configureLog(dir);
    folder = shown(dir);
  } catch { /* no log this launch; the app still runs and says so on the problem page */ }

  function send(event) {
    try {
      if (typeof native.logEvent === 'function') native.logEvent(JSON.stringify(event));
    } catch { /* the log must never become a second failure */ }
  }
  function entry(op, kind, e, extra = {}) {
    const error = e instanceof Error ? e : null;
    send({op, kind, name: error ? error.name : typeof e, code: error && typeof error.code === 'string' ? error.code : undefined,
      frames: error ? appFrames(error.stack) : [], ...extra});
  }

  let last = null;
  return {
    PROBLEM_PATH,
    folder: () => folder,
    panic: panicInfo,
    panicText,
    /** A main-process error that is handled (swallowed) where it happens: record it, change nothing.
     * fb-node's own refusals (code GenericFailure or FB_*) are the app answering, not failures. */
    note(op, e) { if (!panicInfo(e) && !refusal(e)) entry(op, 'caught', e); },
    /** An error on its way to the page: recorded only when it is not one of our plain refusals
     * (`new Error('…')` with words for the owner), e.g. a TypeError or a file-system error. */
    unexpected(op, e) {
      if (panicInfo(e) || refusal(e) || (e instanceof Error && e.name === 'Error' && typeof e.code !== 'string')) return;
      entry(op, 'error', e);
    },
    /** The panic page for `info`, kept for every later load of PROBLEM_PATH until the next one. */
    remember(info, state, pathname) { last = problemPage(info, {...state, folder, back: continueFrom(pathname, info)}); return last; },
    lastPage: () => last,
    install() {
      processRef.on('uncaughtException', e => {
        entry('main', 'uncaughtException', e);
        try { dialog.showErrorBox('FreedomBooks', `Something went wrong inside FreedomBooks. Your books are safe. The details were recorded in the problem log${folder ? ` (${folder})` : ''}.`); }
        catch { /* nothing more to do */ }
      });
      processRef.on('unhandledRejection', e => entry('main', 'unhandledRejection', e));
    },
    /** The two renderer calls behind the buttons on Report a problem and the problem page. */
    calls() {
      const onLogPage = payload => {
        if (payload === null || typeof payload !== 'object' || Array.isArray(payload) || Object.getPrototypeOf(payload) !== Object.prototype || Object.keys(payload).length) {
          throw new Error('Invalid request.');
        }
        const win = window();
        if (!win || win.isDestroyed() || !LOG_PAGES.includes(new URL(win.webContents.getURL()).pathname)) throw new Error('Refused: not available on this page.');
        if (!folder) throw new Error('This copy of FreedomBooks has no problem log.');
      };
      return {
        'app.openLogFolder': async payload => {
          onLogPage(payload);
          const failed = await shell.openPath(dir);
          if (failed) throw new Error(`The log folder could not be opened. It is ${folder}.`);
          return {outcome: 'done'};
        },
        'app.copyDiagnostics': payload => {
          onLogPage(payload);
          clipboard.writeText(native.diagnosticSummary());
          return {outcome: 'done', data: {copied: true}};
        },
      };
    },
  };
}

module.exports = {appFrames, create, logDir, shown, panicInfo, panicText, problemPage, continueFrom, PROBLEM_PATH, LOG_PAGES};

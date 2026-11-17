'use strict';
// Back, Forward, Home, Find and Companies for the app window. One controller serves the Go menu, the
// keyboard (Alt+Left, Alt+Right, Ctrl+Shift+H, Ctrl+F, Ctrl+K), the mouse's back and forward buttons
// (the window's `app-command`) and the title bar's Back and Forward buttons (the preload's
// `app.navigate` call), so they all behave the same.
//
// What Back may reach. History holds only GET pages: every POST form goes through the preload as a
// typed command and the scheme handler refuses any other method, so going back never re-submits a
// form. Pages are `no-store` and re-rendered on every visit, with fresh single-use form tokens and
// without the one-shot messages and re-rendered forms of the visit that showed them; the preload
// reloads a page Chromium restores from its back-forward cache. History is cleared when the session
// changes (lock, unlock, sign-in, sign-out, switch user): Back and Forward never lead into the pages
// of an earlier session, and every action re-checks the session before it moves.
const ORIGIN = 'fb://app';
const COMPANY = /^\/c\/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\//;

/** The session the window's history belongs to: book open, someone signed in, and which session. */
function sessionKey(state) {
  return `${state.open ? 1 : 0}:${state.signed_in ? 1 : 0}:${Number(state.session_generation)}`;
}

/** The company of an app page, or null. */
function companyOf(url) {
  try {
    const u = new URL(url);
    if (u.protocol !== 'fb:' || u.host !== 'app') return null;
    const m = COMPANY.exec(u.pathname);
    return m ? m[1] : null;
  } catch { return null; }
}

/** The action a key press stands for, or null. `mac` uses Cmd where others use Ctrl. */
function shortcut(input, mac = process.platform === 'darwin') {
  if (!input || input.type !== 'keyDown') return null;
  const command = mac ? input.meta && !input.control : input.control && !input.meta;
  const key = String(input.key || '');
  if (input.alt && !input.control && !input.meta && !input.shift) {
    if (key === 'ArrowLeft') return 'back';
    if (key === 'ArrowRight') return 'forward';
  }
  if (command && !input.alt) {
    const k = key.toLowerCase();
    if (!input.shift && (k === 'f' || k === 'k')) return 'find';
    if (input.shift && k === 'h') return 'home';
  }
  return null;
}

/** The title bar's request: exactly `{to: 'back'|'forward'}`. */
function request(payload) {
  if (payload === null || typeof payload !== 'object' || Array.isArray(payload) || Object.getPrototypeOf(payload) !== Object.prototype
    || Object.keys(payload).length !== 1 || !['back', 'forward'].includes(payload.to)) throw new Error('Invalid navigation.');
  return payload.to;
}

/**
 * `native` gives appState(); `window()` the app window (or null); `menu()` the application menu (or
 * null), whose Back and Forward items follow the history.
 */
function controller({ native, window, menu = () => null, origin = ORIGIN }) {
  let key = null;
  // The book's state, or null while a long operation (a backup, an AI request) holds the books: the
  // native call answers BUSY then. Nothing moves or is cleared on a state that could not be read.
  const state = () => {
    try { return JSON.parse(native.appState()); } catch { return null; }
  };
  const contents = () => {
    const w = window();
    return w && !w.isDestroyed() ? w.webContents : null;
  };
  // Entries made under another session are dropped before anything moves or is shown.
  function guard(wc) {
    const s = state();
    if (!s) return false;
    const now = sessionKey(s);
    if (key !== null && key !== now) wc.navigationHistory.clear();
    key = now;
    return true;
  }
  function publish(wc) {
    const h = wc.navigationHistory;
    const signedIn = key !== null && key.startsWith('1:1:');
    const s = { back: signedIn && h.canGoBack(), forward: signedIn && h.canGoForward() };
    const m = menu();
    for (const [id, on] of [['go-back', s.back], ['go-forward', s.forward]]) {
      const item = m && m.getMenuItemById(id);
      if (item) item.enabled = on;
    }
    if (!wc.isDestroyed()) wc.send('fb:history', s);
    return s;
  }
  function step(direction) {
    const wc = contents();
    if (!wc || !guard(wc)) return false;
    const h = wc.navigationHistory;
    // Only a signed-in session walks its own history; the unlock and sign-in screens have none.
    if (!key.startsWith('1:1:')) { publish(wc); return false; }
    if (direction === 'back' ? !h.canGoBack() : !h.canGoForward()) return false;
    if (direction === 'back') h.goBack(); else h.goForward();
    return true;
  }
  const actions = {
    back: () => step('back'),
    forward: () => step('forward'),
    home() {
      const wc = contents();
      const s = state();
      if (!wc || !s || !s.open || !s.signed_in) return false;
      const company = companyOf(wc.getURL());
      wc.loadURL(company ? `${origin}/c/${company}/home/` : `${origin}/`);
      return true;
    },
    companies() {
      const wc = contents();
      const s = state();
      if (!wc || !s || !s.open || !s.signed_in) return false;
      wc.loadURL(`${origin}/`);
      return true;
    },
    // The page's own Find field (company pages); the preload focuses it.
    find() {
      const wc = contents();
      const s = state();
      if (!wc || !s || !s.signed_in || !companyOf(wc.getURL())) return false;
      wc.send('fb:find');
      return true;
    },
  };
  function attach(win) {
    const wc = win.webContents;
    const navigated = () => { if (guard(wc)) publish(wc); };
    wc.on('did-navigate', navigated);
    wc.on('did-navigate-in-page', navigated);
    wc.on('dom-ready', navigated);
    // Calling preventDefault keeps the key from the page and from menu accelerators: one path only.
    wc.on('before-input-event', (event, input) => {
      const action = shortcut(input);
      if (!action) return;
      event.preventDefault();
      actions[action]();
    });
    win.on('app-command', (_event, command) => {
      if (command === 'browser-backward') actions.back();
      else if (command === 'browser-forward') actions.forward();
    });
  }
  // Accelerators are shown in the menu but handled by `before-input-event` above.
  const show = accelerator => ({ accelerator, registerAccelerator: false });
  const go = {
    label: 'Go',
    submenu: [
      { id: 'go-back', label: 'Back', enabled: false, ...show('Alt+Left'), click: () => actions.back() },
      { id: 'go-forward', label: 'Forward', enabled: false, ...show('Alt+Right'), click: () => actions.forward() },
      { type: 'separator' },
      { label: 'Home', ...show('CmdOrCtrl+Shift+H'), click: () => actions.home() },
      { label: 'Find…', ...show('CmdOrCtrl+F'), click: () => actions.find() },
      { type: 'separator' },
      { label: 'Companies…', click: () => actions.companies() },
    ],
  };
  return { attach, actions, go, navigate: payload => ({ moved: actions[request(payload)]() }) };
}

/** The Help menu: Report a problem (the app's own page) and About, with this build's version. */
function helpMenu({ app, dialog, native, window, origin = ORIGIN }) {
  return {
    label: 'Help',
    submenu: [
      {
        label: 'Report a problem…',
        click: () => {
          const w = window();
          if (w && !w.isDestroyed() && JSON.parse(native.appState()).open) w.loadURL(`${origin}/help/report/`);
        },
      },
      { type: 'separator' },
      {
        label: 'About FreedomBooks',
        click: () => {
          const w = window();
          const options = { type: 'info', title: 'About FreedomBooks', message: `FreedomBooks ${app.getVersion()}`, detail: 'Accounting for small businesses. Your books stay on this computer.', buttons: ['OK'], noLink: true };
          return w && !w.isDestroyed() ? dialog.showMessageBox(w, options) : dialog.showMessageBox(options);
        },
      },
    ],
  };
}

module.exports = { controller, helpMenu, shortcut, request, sessionKey, companyOf };

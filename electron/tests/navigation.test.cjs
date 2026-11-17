'use strict';
// GO: Back, Forward, Home and Find. One controller for the menu, keys, mouse and title bar; history never
// crosses a lock, sign-in, sign-out or user switch.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { controller, helpMenu, shortcut, request, companyOf } = require('../main/navigation.cjs');

const C = '0195b4a4-55af-719e-96cb-a2f946b067bf';

/** A window whose history behaves like Chromium's: entries, an index, and clear() keeping the current one. */
function fixture() {
  const session = { open: true, signed_in: true, session_generation: 3 };
  const handlers = {}, windowHandlers = {}, sent = [], items = { 'go-back': { enabled: false }, 'go-forward': { enabled: false } };
  const h = { entries: [], index: -1 };
  const wc = {
    isDestroyed: () => false,
    on: (event, fn) => { handlers[event] = fn; },
    send: (channel, value) => sent.push([channel, value]),
    getURL: () => h.entries[h.index] || '',
    loadURL: (url) => visit(url),
    navigationHistory: {
      canGoBack: () => h.index > 0,
      canGoForward: () => h.index < h.entries.length - 1,
      goBack: () => { h.index -= 1; handlers['did-navigate'](); },
      goForward: () => { h.index += 1; handlers['did-navigate'](); },
      clear: () => { h.entries = [h.entries[h.index]]; h.index = 0; },
    },
  };
  function visit(url) {
    h.entries = h.entries.slice(0, h.index + 1).concat(url);
    h.index = h.entries.length - 1;
    handlers['did-navigate']();
  }
  const win = { isDestroyed: () => false, webContents: wc, on: (event, fn) => { windowHandlers[event] = fn; } };
  const nav = controller({ native: { appState: () => { if (session.busy) throw new Error('BUSY'); return JSON.stringify(session); } }, window: () => win, menu: () => ({ getMenuItemById: id => items[id] }) });
  nav.attach(win);
  const key = (input) => { let prevented = false; handlers['before-input-event']({ preventDefault: () => { prevented = true; } }, { type: 'keyDown', alt: false, control: false, shift: false, meta: false, ...input }); return prevented; };
  return { session, h, nav, visit, sent, items, key, windowHandlers, url: () => wc.getURL(), last: () => sent.filter(([c]) => c === 'fb:history').at(-1)[1] };
}

test('Back and Forward walk this session\'s pages and report where they can go', () => {
  const f = fixture();
  f.visit(`fb://app/c/${C}/home/`);
  assert.deepEqual(f.last(), { back: false, forward: false });
  assert.equal(f.items['go-back'].enabled, false);
  f.visit(`fb://app/c/${C}/invoices/`);
  f.visit(`fb://app/c/${C}/invoices/new/`);
  assert.deepEqual(f.last(), { back: true, forward: false });
  assert.equal(f.items['go-back'].enabled, true);
  assert.equal(f.nav.actions.back(), true);
  assert.equal(f.url(), `fb://app/c/${C}/invoices/`);
  assert.deepEqual(f.last(), { back: true, forward: true });
  assert.equal(f.items['go-forward'].enabled, true);
  assert.equal(f.nav.navigate({ to: 'forward' }).moved, true);
  assert.equal(f.url(), `fb://app/c/${C}/invoices/new/`);
  assert.equal(f.nav.actions.forward(), false, 'nothing ahead');
});

test('a lock, unlock, sign-in or user switch clears history: Back never reaches another session\'s pages', () => {
  const f = fixture();
  f.visit(`fb://app/c/${C}/home/`);
  f.visit(`fb://app/c/${C}/bills/`);
  // Lock: the book closes and the gate loads.
  Object.assign(f.session, { open: false, signed_in: false, session_generation: 4 });
  f.visit('fb://app/');
  assert.deepEqual(f.h.entries, ['fb://app/']);
  assert.deepEqual(f.last(), { back: false, forward: false });
  assert.equal(f.nav.actions.back(), false);
  // Unlock, then sign in as someone else.
  Object.assign(f.session, { open: true });
  f.visit('fb://app/desktop/sign-in/');
  assert.deepEqual(f.h.entries, ['fb://app/desktop/sign-in/']);
  assert.equal(f.nav.actions.back(), false, 'the sign-in screen has no history');
  Object.assign(f.session, { signed_in: true, session_generation: 5 });
  f.visit(`fb://app/c/${C}/home/`);
  assert.deepEqual(f.h.entries, [`fb://app/c/${C}/home/`]);
  assert.deepEqual(f.last(), { back: false, forward: false });
  // A session change seen first by Back itself (no page loaded since) also clears before moving.
  f.visit(`fb://app/c/${C}/checks/`);
  f.session.session_generation = 6;
  assert.equal(f.nav.actions.back(), false);
  assert.deepEqual(f.h.entries, [`fb://app/c/${C}/checks/`]);
  // And Forward after a sign-out.
  f.visit(`fb://app/c/${C}/home/`);
  f.nav.actions.back();
  Object.assign(f.session, { signed_in: false, session_generation: 7 });
  assert.equal(f.nav.actions.forward(), false);
  assert.equal(f.h.entries.length, 1);
});

test('while the books are busy nothing moves, nothing throws, and history is kept', () => {
  const f = fixture();
  f.visit(`fb://app/c/${C}/home/`);
  f.visit(`fb://app/c/${C}/backups/`);
  f.session.busy = true;
  assert.equal(f.nav.actions.back(), false);
  assert.equal(f.key({ key: 'ArrowLeft', alt: true }), true);
  assert.equal(f.nav.actions.home(), false);
  assert.equal(f.nav.actions.find(), false);
  assert.doesNotThrow(() => f.visit(`fb://app/c/${C}/backups/?x=1`));
  assert.equal(f.h.entries.length, 3, 'nothing cleared on an unreadable state');
  f.session.busy = false;
  assert.equal(f.nav.actions.back(), true);
  assert.equal(f.url(), `fb://app/c/${C}/backups/`);
});

test('keys, mouse buttons, the menu and the title bar drive the same actions', () => {
  const f = fixture();
  f.visit(`fb://app/c/${C}/home/`);
  f.visit(`fb://app/c/${C}/deposits/`);
  assert.equal(f.key({ key: 'ArrowLeft', alt: true }), true, 'Alt+Left is taken from the page and menu');
  assert.equal(f.url(), `fb://app/c/${C}/home/`);
  f.windowHandlers['app-command']({}, 'browser-forward');
  assert.equal(f.url(), `fb://app/c/${C}/deposits/`);
  f.windowHandlers['app-command']({}, 'browser-backward');
  assert.equal(f.url(), `fb://app/c/${C}/home/`);
  f.nav.go.submenu.find(i => i.id === 'go-forward').click();
  assert.equal(f.url(), `fb://app/c/${C}/deposits/`);
  // Find focuses the page's field on company pages only.
  assert.equal(f.key({ key: 'k', control: true }), true);
  assert.equal(f.key({ key: 'F', control: true }), true);
  assert.equal(f.sent.filter(([c]) => c === 'fb:find').length, 2);
  // Home goes to this company's home page.
  f.visit(`fb://app/c/${C}/bills/new/`);
  assert.equal(f.key({ key: 'H', control: true, shift: true }), true);
  assert.equal(f.url(), `fb://app/c/${C}/home/`);
  f.nav.go.submenu.find(i => i.label === 'Companies…').click();
  assert.equal(f.url(), 'fb://app/');
  assert.equal(f.nav.actions.find(), false, 'no Find field outside a company');
  // Other keys pass through to the page.
  assert.equal(f.key({ key: 'ArrowLeft' }), false);
  assert.equal(f.key({ key: 'ArrowLeft', alt: true, shift: true }), false);
  assert.equal(f.key({ key: 'f', control: true, alt: true }), false);
  // Signed out, Home and Companies do nothing.
  Object.assign(f.session, { signed_in: false, session_generation: 9 });
  assert.equal(f.nav.actions.home(), false);
  assert.equal(f.nav.actions.companies(), false);
  assert.equal(f.nav.actions.find(), false);
});

test('shortcuts, the title bar request and company addresses are parsed strictly', () => {
  assert.equal(shortcut({ type: 'keyDown', key: 'ArrowLeft', alt: true }, false), 'back');
  assert.equal(shortcut({ type: 'keyUp', key: 'ArrowLeft', alt: true }, false), null);
  assert.equal(shortcut({ type: 'keyDown', key: 'k', meta: true }, true), 'find');
  assert.equal(shortcut({ type: 'keyDown', key: 'k', meta: true }, false), null);
  assert.equal(shortcut({ type: 'keyDown', key: 'h', control: true, shift: true }, false), 'home');
  assert.equal(request({ to: 'back' }), 'back');
  for (const bad of [null, [], 'back', { to: 'home' }, { to: 'back', extra: 1 }, { to: 'back', __proto__: { x: 1 } }, Object.create(null)]) {
    assert.throws(() => request(bad), /Invalid navigation/);
  }
  assert.equal(companyOf(`fb://app/c/${C}/invoices/`), C);
  assert.equal(companyOf(`https://app/c/${C}/invoices/`), null);
  assert.equal(companyOf('fb://app/c/not-a-company/home/'), null);
});

test('the Go and Help menus: Back, Forward, Home, Find, Companies; Report a problem and About', async () => {
  const f = fixture();
  assert.deepEqual(f.nav.go.submenu.filter(i => i.label).map(i => [i.label, i.accelerator || '']), [
    ['Back', 'Alt+Left'], ['Forward', 'Alt+Right'], ['Home', 'CmdOrCtrl+Shift+H'], ['Find…', 'CmdOrCtrl+F'], ['Companies…', ''],
  ]);
  assert.ok(f.nav.go.submenu.filter(i => i.accelerator).every(i => i.registerAccelerator === false), 'keys are handled once, by before-input-event');
  let loaded = null, shown = null;
  const win = { isDestroyed: () => false, loadURL: url => { loaded = url; } };
  const help = helpMenu({ app: { getVersion: () => '0.1.0-d4' }, dialog: { showMessageBox: (_w, o) => { shown = o; } }, native: { appState: () => '{"open":true}' }, window: () => win });
  assert.deepEqual(help.submenu.filter(i => i.label).map(i => i.label), ['Report a problem…', 'About FreedomBooks']);
  help.submenu[0].click();
  assert.equal(loaded, 'fb://app/help/report/');
  help.submenu[2].click();
  assert.equal(shown.message, 'FreedomBooks 0.1.0-d4');
});

test('the main process wires the menu and the window, and the renderer calls are exactly the known set', () => {
  const main = fs.readFileSync(path.join(__dirname, '..', 'main', 'main.cjs'), 'utf8');
  assert.match(main, /navigation\.attach\(win\);/);
  assert.match(main, /navigation\.go,\n\s+require\('\.\/navigation\.cjs'\)\.helpMenu/);
  assert.match(main, /calls\['app\.navigate'\] = \(payload\) => navigation\.navigate\(payload\);/);
  const preload = fs.readFileSync(path.join(__dirname, '..', 'preload', 'preload.cjs'), 'utf8');
  assert.deepEqual([...new Set([...preload.matchAll(/call\('([a-zA-Z.]+)'/g)].map(m => m[1]))].sort(), ['app.chooseBook', 'app.copyDiagnostics', 'app.createBook', 'app.navigate', 'app.openExternal', 'app.openLogFolder', 'app.restoreBook', 'app.unlock', 'books.run', 'hosting.on', 'hosting.repair', 'hosting.revoke', 'remote.connect', 'remote.disconnect', 'remote.forget', 'remote.pair']);
  assert.match(preload, /if \(event\.persisted\) window\.location\.reload\(\);/, 'a page from the back-forward cache is loaded fresh');
  assert.match(preload, /form\.setAttribute\('autocomplete', 'off'\);\n\s+if \(fromHistory\) form\.reset\(\);/, 'Back never refills a saving form with what was typed before');
});

"""DESKTOP-JODY1 item 1, Electron side: the issue page opens in the system browser from the main process.

One new fixed IPC call (`fb:app.openExternal`) accepts only an exact allowlisted URL; the renderer
never navigates away (will-navigate and the window-open handler still deny everything outside fb://app).
"""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def replace(path, old, new):
    p = ROOT / path
    t = p.read_text()
    assert t.count(old) == 1, (path, old)
    p.write_text(t.replace(old, new))


replace('electron/main/guards.cjs', '''module.exports.rendererCommands = Object.freeze([...RENDERER_COMMANDS]);''', '''module.exports.rendererCommands = Object.freeze([...RENDERER_COMMANDS]);

// DESKTOP-JODY1: the only outside addresses the app opens, in the system browser, from the main
// process. Exact string match: no parsing, no prefix, no case folding, no other scheme or repository.
const EXTERNAL_URLS = Object.freeze(['https://github.com/CyberJody/FreedomBooks/issues/new/choose']);
module.exports.EXTERNAL_URLS = EXTERNAL_URLS;
module.exports.external = payload => {
  plain(payload, 'link');
  if (Object.keys(payload).length !== 1 || typeof payload.url !== 'string' || !EXTERNAL_URLS.includes(payload.url)) throw new Error('Refused: link not allowed.');
  return payload.url;
};''')

replace('electron/main/main.cjs', '''const { app, BrowserWindow, Menu, ipcMain, protocol, session, dialog, powerMonitor } = require('electron');''',
        '''const { app, BrowserWindow, Menu, ipcMain, protocol, session, dialog, powerMonitor, shell } = require('electron');''')
replace('electron/main/main.cjs', '''const native = require(development.nativeModule(app, __dirname));''', '''const native = require(development.nativeModule(app, __dirname));
// "Report a problem" shows these for copying into an issue (no other machine details).
native.setHost(app.getVersion(), require('./about.cjs').osLabel());''')
replace('electron/main/main.cjs', '''  'app.unlock': (payload) => {''', '''  // The issue page opens in the system browser; only guards.EXTERNAL_URLS, matched exactly.
  'app.openExternal': async (payload) => {
    await shell.openExternal(guards.external(payload));
    return { outcome: 'done' };
  },
  'app.unlock': (payload) => {''')

replace('electron/preload/preload.cjs', '''// CSV links retain Django's URLs;''', '''// Outside links never navigate the window. The issue page (exactly this address; the main process
// re-checks its own allowlist) opens in the system browser; any other outside link does nothing.
const ISSUE_URL = 'https://github.com/CyberJody/FreedomBooks/issues/new/choose';
document.addEventListener('click', async (event) => {
  const anchor = event.target.closest && event.target.closest('a[href]');
  if (!anchor || event.defaultPrevented) return;
  const href = anchor.getAttribute('href');
  if (!/^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith('fb://app/')) return;
  event.preventDefault();
  if (href !== ISSUE_URL || event.button !== 0) return;
  try {
    await call('app.openExternal', { url: href });
  } catch (e) { window.alert(String(e.message || e)); }
}, true);

// CSV links retain Django's URLs;''')
print('ok')

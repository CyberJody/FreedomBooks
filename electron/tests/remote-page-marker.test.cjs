'use strict';
// Client mode marks each host page with the session that rendered it, and a host GET with a query
// must carry that marker (remote-mode.cjs). The preload adds it, in a real sandboxed Electron window
// with the real preload here. What matters is that the page's own scripts keep control:
//   - a GET submit the page cancels stays cancelled (Review transactions asks before dropping drafts,
//     Pay bills filters in place, the Bills list refuses "Pay selected" with nothing ticked);
//   - a link the page cancels is not followed;
//   - a link added or rewritten after the page loaded still carries the marker when followed;
//   - a page without a marker (the standalone desktop, hosting) is left exactly as it was.
// Hidden window, headless platform: no display is used.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawn } = require('node:child_process');

const root = path.resolve(__dirname, '..');
const MARKER = '11111111-2222-4333-8444-555555555555';

const page = marker => `<!doctype html><html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'self'; style-src 'self'">
${marker ? `<meta name="fb-remote-page" content="${marker}">` : ''}
<style>@page { size: landscape; }</style>
<style>@page { size: letter portrait; margin: .4in 12pt; margin-bottom: 1in; }</style>
<style>@page { margin: 1cm; } p { color: blue; }</style>
<style>@page { @bottom-center { content: "additional rule"; } }</style>
<style>@page { margin: url(example); }</style>
<style>@page { margin-left: 1cm 2cm; }</style>
<style>@page cover { margin: 0; }</style>
</head><body><button id="print-now">Print</button>
<form method="get" id="cancelled"><input name="due" value="2026-11-30"><button type="submit">Filter</button></form>
<form method="get" id="plain"><input name="q" value="1">${marker ? '<input type="hidden" name="__fb_remote_page" value="old"><input type="hidden" name="__fb_remote_page" value="duplicate">' : ''}<button type="submit">Go</button></form>
<form method="post" id="post" action="/c/x/save/"><input name="n" value="1"></form>
<a id="static" href="?x=1">static</a>
<a id="rewritten" href="?status=ALL">rewritten</a>
<a id="refused" href="?no=1">refused</a>
<a id="bare" href="#top">bare</a>
<script src="page.js"></script>
</body></html>`;

// The page's own script: cancels one submit and one link, rewrites one link on click, adds one late.
const PAGE_JS = `
document.getElementById('cancelled').addEventListener('submit', event => { event.preventDefault(); window.__cancelled = (window.__cancelled || 0) + 1; });
document.getElementById('refused').addEventListener('click', event => { event.preventDefault(); });
document.getElementById('rewritten').addEventListener('click', event => { event.currentTarget.setAttribute('href', '?status=ATTENTION'); });
document.addEventListener('DOMContentLoaded', () => {
  const late = document.createElement('a');
  late.id = 'late'; late.textContent = 'late'; late.setAttribute('href', '?late=1');
  document.body.append(late);
});
`;

const MAIN = `
const { app, BrowserWindow, protocol } = require('electron');
const fs = require('node:fs');
const path = require('node:path');
const dir = process.env.FB_TEST_DIR, preload = process.env.FB_TEST_PRELOAD;
const out = (v) => { process.stdout.write('RESULT ' + JSON.stringify(v) + '\\n'); app.exit(0); };
// The app's own origin, fb://app: the preload treats any other scheme as an outside link.
protocol.registerSchemesAsPrivileged([{ scheme: 'fb', privileges: { standard: true, secure: true } }]);
app.whenReady().then(async () => {
  protocol.handle('fb', request => {
    const name = path.basename(new URL(request.url).pathname);
    if (!['marked.html', 'duplicate.html', 'plain.html', 'page.js'].includes(name)) return new Response('', { status: 404 });
    return new Response(fs.readFileSync(path.join(dir, name)), { headers: { 'content-type': name.endsWith('.js') ? 'text/javascript' : 'text/html; charset=utf-8' } });
  });
  const win = new BrowserWindow({ show: false, webPreferences: { preload, sandbox: true, contextIsolation: true, nodeIntegration: false } });
  const js = (code) => win.webContents.executeJavaScript(code);
  const settle = () => new Promise(r => setTimeout(r, 300));
  let loads = 0;
  win.webContents.on('did-finish-load', () => { loads++; });
  // Click, then report whether the window loaded a page again and where it is.
  const act = async (code) => {
    const before = loads;
    await js(code + '; true');
    for (let i = 0; i < 40 && loads === before; i++) await new Promise(r => setTimeout(r, 25));
    await settle();
    return { navigated: loads !== before, search: await js('window.location.search'), hash: await js('window.location.hash') };
  };
  const results = {};
  try {
    for (const name of ['marked', 'plain']) {
      const r = results[name] = {};
      await win.loadURL('fb://app/' + name + '.html');
      r.styles = await js('document.adoptedStyleSheets.flatMap(s => Array.from(s.cssRules, rule => rule.cssText))');
      r.stamped = await js(\`({
        static: document.getElementById('static').getAttribute('href'),
        late: document.getElementById('late').getAttribute('href'),
        getField: document.querySelectorAll('#plain input[type=hidden]').length,
        postField: document.querySelectorAll('#post input[type=hidden]').length,
      })\`);
      r.cancelled = await act("document.querySelector('#cancelled button').click()");
      r.cancelledCount = await js('window.__cancelled || 0');
      r.refused = await act("document.getElementById('refused').click()");
      r.bare = await act("document.getElementById('bare').click()");
      r.late = await act("document.getElementById('late').click()");
      r.rewritten = await act("document.getElementById('rewritten').click()");
      r.static = await act("document.getElementById('static').click()");
      r.form = await act("document.querySelector('#plain button').click()");
    }
    results.rejectedMarkers = [];
    for (const target of ['marked.html?__fb_remote_page=stale', 'marked.html?__fb_remote_page=stale&__fb_remote_page=stale', 'duplicate.html']) {
      await win.loadURL('fb://app/' + target);
      results.rejectedMarkers.push(await js("document.querySelector('#plain input[type=hidden]').value"));
    }
    out({ results });
  } catch (e) { out({ error: String(e && e.stack || e), results }); }
});
`;

test('the preload carries the page marker without taking navigation away from the page', { timeout: 180000 }, async () => {
  const dir = fs.mkdtempSync(path.join(process.env.TMPDIR || os.tmpdir(), 'fb-marker-'));
  try {
    fs.writeFileSync(path.join(dir, 'marked.html'), page(MARKER));
    fs.writeFileSync(path.join(dir, 'duplicate.html'), page(MARKER).replace('</head>', '<meta content="stale" name="FB-REMOTE-PAGE"></head>'));
    fs.writeFileSync(path.join(dir, 'plain.html'), page(null));
    fs.writeFileSync(path.join(dir, 'page.js'), PAGE_JS);
    const main = path.join(dir, 'main.cjs');
    fs.writeFileSync(main, MAIN);
    const electron = require(path.join(root, 'node_modules', 'electron'));
    const env = { ...process.env, FB_TEST_DIR: dir, FB_TEST_PRELOAD: path.join(root, 'preload', 'preload.cjs'), ELECTRON_ENABLE_LOGGING: '0' };
    delete env.ELECTRON_RUN_AS_NODE;
    const output = await new Promise((resolve, reject) => {
      const child = spawn(electron, ['--ozone-platform=headless', `--user-data-dir=${path.join(dir, 'profile')}`, main], { env, stdio: ['ignore', 'pipe', 'pipe'] });
      let stdout = '', stderr = '';
      child.stdout.on('data', d => { stdout += d; });
      child.stderr.on('data', d => { stderr += d; });
      const timer = setTimeout(() => { child.kill('SIGKILL'); reject(new Error(`Electron timed out\n${stderr}`)); }, 150000);
      child.on('exit', () => { clearTimeout(timer); resolve({ stdout, stderr }); });
    });
    const line = output.stdout.split('\n').find(l => l.startsWith('RESULT '));
    assert.ok(line, `no result from Electron:\n${output.stdout}\n${output.stderr}`);
    const { results, error } = JSON.parse(line.slice(7));
    assert.equal(error, undefined, error);
    const key = `__fb_remote_page=${MARKER}`;
    const { marked, plain } = results;
    assert.deepEqual(results.rejectedMarkers, ['invalid-marker', 'invalid-marker', 'invalid-marker']);

    for (const page of [marked, plain]) {
      assert.equal(page.styles.length, 2);
      assert.match(page.styles[0], /size: landscape/);
      assert.match(page.styles[1], /margin:/);
    }
    // The page's own decisions stand.
    assert.deepEqual([marked.cancelled.navigated, marked.cancelled.search, marked.cancelledCount], [false, '', 1], 'a submit the page cancelled is not sent');
    assert.deepEqual([marked.refused.navigated, marked.refused.search], [false, ''], 'a link the page cancelled is not followed');
    assert.equal(marked.bare.search, '', 'a link without a query is left alone');
    // Followed links and submitted forms carry the marker, including ones added or rewritten later.
    assert.deepEqual([marked.late.navigated, marked.late.search], [true, `?late=1&${key}`]);
    assert.deepEqual([marked.rewritten.navigated, marked.rewritten.search], [true, `?status=ATTENTION&${key}`], 'stamped after the page rewrote it');
    assert.deepEqual([marked.static.navigated, marked.static.search], [true, `?x=1&${key}`]);
    assert.deepEqual([marked.form.navigated, marked.form.search], [true, `?q=1&${key}`]);
    // A marked page: links and GET forms present at load are stamped; a POST form never is.
    assert.ok(marked.stamped.static.endsWith(`?x=1&${key}`), marked.stamped.static);
    assert.equal(marked.stamped.late, '?late=1', 'this link arrived after the load-time pass');
    assert.deepEqual([marked.stamped.getField, marked.stamped.postField], [1, 0]);

    // A page without a marker behaves exactly as before: nothing is added anywhere.
    assert.deepEqual([plain.stamped.static, plain.stamped.late, plain.stamped.getField, plain.stamped.postField], ['?x=1', '?late=1', 0, 0]);
    assert.deepEqual([plain.cancelled.navigated, plain.refused.navigated], [false, false]);
    assert.deepEqual([plain.late.search, plain.rewritten.search, plain.static.search, plain.form.search], ['?late=1', '?status=ATTENTION', '?x=1', '?q=1']);
  } finally { fs.rmSync(dir, { recursive: true, force: true }); }
});

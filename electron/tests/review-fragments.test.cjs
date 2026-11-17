'use strict';
// The preload's in-place reply (Review transactions, `applyFragments`), in a real sandboxed Electron window with
// the real preload: only an element that opted in with `data-fragment`, inside the submitting form, with the same
// id and tag, is replaced; escaped bank text and scripts in a fragment stay inert; the next single-use token goes
// into the submitting form only and is what its next post (to the same path) sends. The main process side is a
// stand-in for `books.run` that records each command and answers with prepared fragments. Hidden window,
// headless platform: no display is used.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawn } = require('node:child_process');

const root = path.resolve(__dirname, '..');

const PAGE = `<!doctype html><html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'self'; style-src 'self'">
</head><body>
<form method="post" id="bankfeed-review" action="/c/00000000-0000-0000-0000-000000000001/bank-feeds/review/">
<input type="hidden" name="fb_command_token" value="first-token">
<div id="review-status" data-fragment><p>3 to review</p></div>
<table><tbody>
<tr id="review-row-1" data-fragment data-status="NEW"><td>OFFICE DEPOT</td><td><button type="submit" name="accept_one" value="1" id="go">Accept</button></td></tr>
<tr id="review-row-2" data-status="NEW"><td>not opted in</td></tr>
<tr id="review-row-3" data-fragment data-status="NEW"><td>tag must match</td></tr>
</tbody></table>
</form>
<div id="outside" data-fragment>outside the form</div>
<form method="post" id="other" action="/c/00000000-0000-0000-0000-000000000001/bank-feeds/"><input type="hidden" name="fb_command_token" value="other-token"></form>
</body></html>`;

// The reply to the first post: one fragment of each kind the preload must refuse, plus the two it must apply.
const EVIL = '&lt;img src=x onerror=window.__pwned=1&gt;&lt;/td&gt;&lt;/tr&gt;&lt;tr id="review-status" data-fragment&gt;';
const FIRST = {
  next_token: 'next-token',
  fragments: [
    { id: 'review-row-1', html: `<tr id="review-row-1" data-fragment data-status="ACCEPTED"><td class="desc">${EVIL}<script>window.__pwned=2</script></td><td><button type="submit" name="undo_one" value="1" id="go">Undo</button></td></tr>` },
    { id: 'review-status', html: '<div id="review-status" data-fragment><p>2 to review</p></div>' },
    { id: 'review-row-2', html: '<tr id="review-row-2" data-fragment><td>replaced without opting in</td></tr>' },
    { id: 'review-row-3', html: '<div id="review-row-3" data-fragment>a different tag</div>' },
    { id: 'outside', html: '<div id="outside" data-fragment>replaced outside the form</div>' },
    { id: 'review-row-1', html: '<tr id="review-row-9" data-fragment><td>a different id</td></tr>' },
    { id: 'missing', html: '<p id="missing" data-fragment>no such element</p>' },
    { id: 7, html: '<p>not a string id</p>' },
  ],
};

const MAIN = `'use strict';
const { app, BrowserWindow, ipcMain } = require('electron');
const page = process.env.FB_TEST_PAGE, preload = process.env.FB_TEST_PRELOAD;
const first = JSON.parse(process.env.FB_TEST_FIRST);
const commands = [];
const out = (v) => { process.stdout.write('RESULT ' + JSON.stringify(v) + '\\n'); app.exit(0); };
ipcMain.handle('fb:books.run', (_e, command) => {
  commands.push(command);
  const data = commands.length === 1 ? first : { next_token: 'third-token', fragments: [] };
  return { ok: true, value: { outcome: 'done', data } };
});
app.whenReady().then(async () => {
  const win = new BrowserWindow({ show: false, webPreferences: { preload, sandbox: true, contextIsolation: true, nodeIntegration: false } });
  const js = (code) => win.webContents.executeJavaScript(code);
  const until = async (code) => { for (let i = 0; i < 400; i++) { if (await js(code)) return; await new Promise(r => setTimeout(r, 25)); } throw new Error('timed out: ' + code); };
  try {
    await win.loadFile(page);
    await js("window.__events = 0; document.getElementById('bankfeed-review').addEventListener('fb:fragments', () => { window.__events++; }); true");
    await js("document.getElementById('go').click(); true");
    await until('window.__events === 1');
    const after = await js(\`({
      row1: document.getElementById('review-row-1').dataset.status,
      row1Text: document.getElementById('review-row-1').querySelector('.desc').textContent,
      imgs: document.querySelectorAll('img').length,
      scripts: document.querySelectorAll('#review-row-1 script').length,
      pwned: window.__pwned === undefined ? null : window.__pwned,
      rows: document.querySelectorAll('[id="review-status"]').length,
      status: document.getElementById('review-status').textContent,
      row2: document.getElementById('review-row-2').textContent,
      row3Tag: document.getElementById('review-row-3').tagName,
      outside: document.getElementById('outside').textContent,
      row9: !!document.getElementById('review-row-9'),
      missing: !!document.getElementById('missing'),
      token: document.querySelector('#bankfeed-review input[name=fb_command_token]').value,
      otherToken: document.querySelector('#other input[name=fb_command_token]').value,
    })\`);
    await new Promise(r => setTimeout(r, 200));
    after.pwnedLater = await js('window.__pwned === undefined ? null : window.__pwned');
    await js("document.getElementById('go').click(); true");
    await until('window.__events === 2');
    out({ after, commands });
  } catch (e) { out({ error: String(e && e.stack || e), commands }); }
});
`;

test('the preload swaps only opted-in elements of the submitting form, keeps bank text inert and binds the next token', { timeout: 180000 }, async () => {
  const dir = fs.mkdtempSync(path.join(process.env.TMPDIR || os.tmpdir(), 'fb-fragments-'));
  try {
    const page = path.join(dir, 'review.html');
    const main = path.join(dir, 'main.cjs');
    fs.writeFileSync(page, PAGE);
    fs.writeFileSync(main, MAIN);
    const electron = require(path.join(root, 'node_modules', 'electron'));
    const env = { ...process.env, FB_TEST_PAGE: page, FB_TEST_PRELOAD: path.join(root, 'preload', 'preload.cjs'), FB_TEST_FIRST: JSON.stringify(FIRST), ELECTRON_ENABLE_LOGGING: '0' };
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
    const { after, commands, error } = JSON.parse(line.slice(7));
    assert.equal(error, undefined, error);
    // Applied: the row and the status area of the submitting form.
    assert.equal(after.row1, 'ACCEPTED');
    assert.equal(after.status, '2 to review');
    // Escaped bank text arrives as text; a script in a fragment never runs; nothing breaks out of the row.
    assert.ok(after.row1Text.includes('<img src=x onerror=window.__pwned=1></td></tr><tr id="review-status" data-fragment>'), after.row1Text);
    assert.deepEqual([after.imgs, after.pwned, after.pwnedLater, after.rows], [0, null, null, 1]);
    // Refused: not opted in, a different tag, outside the form, a different id, no such element.
    assert.equal(after.row2, 'not opted in');
    assert.equal(after.row3Tag, 'TR');
    assert.equal(after.outside, 'outside the form');
    assert.deepEqual([after.row9, after.missing], [false, false]);
    // The next token goes into the submitting form only, and its next post sends it to the same path.
    assert.deepEqual([after.token, after.otherToken], ['next-token', 'other-token']);
    assert.equal(commands.length, 2);
    for (const c of commands) {
      assert.equal(c.type, 'SubmitForm');
      assert.equal(c.path, '/c/00000000-0000-0000-0000-000000000001/bank-feeds/review/');
      assert.ok(!c.fields.some(([k]) => k === 'fb_command_token'), 'the token travels apart from the fields');
    }
    assert.deepEqual([commands[0].token, commands[1].token], ['first-token', 'next-token']);
    assert.deepEqual(commands[0].fields.find(([k]) => k === 'accept_one'), ['accept_one', '1']);
    assert.deepEqual(commands[1].fields.find(([k]) => k === 'undo_one'), ['undo_one', '1']);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

'use strict';
// Remote access, client side: the child-process protocol, the sealed pairing record, and client
// mode's handling of host answers (run with plain Node: npm test). Synthetic values only.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawn } = require('node:child_process');
const remoteClient = require('../main/remote-client.cjs');
const remoteMode = require('../main/remote-mode.cjs');

const FAKE = path.join(__dirname, 'fixtures', 'fake-remote-child.cjs');
const fakeStart = (_helper, args, options) => spawn(process.execPath, [FAKE, ...args], options);

test('frames round-trip, split anywhere, and an oversized length is refused', () => {
  const got = [];
  let broken = 0;
  const feed = remoteClient.decoder(v => got.push(v), () => broken++);
  const bytes = Buffer.concat([remoteClient.encode({a: 1}), remoteClient.encode({b: 'é'})]);
  for (const b of bytes) feed(Buffer.from([b]));
  assert.deepEqual(got, [{a: 1}, {b: 'é'}]);
  const huge = Buffer.alloc(4); huge.writeUInt32BE(remoteClient.MAX_FRAME + 1, 0);
  feed(huge);
  assert.equal(broken, 1);
});

test('the child answers by request id, reports link changes, and fails pending calls when it exits', async () => {
  const links = [];
  const c = remoteClient.child({helper: process.execPath, start: fakeStart, onLink: s => links.push(s)});
  const paired = await c.pair('192.168.1.20:47100', 'CODE', 'Laptop');
  assert.equal(paired.host_name, 'Office PC');
  await assert.rejects(c.pair('192.168.1.20:47100', 'WRONG', 'Laptop'), e => e.code === 'pairing' && /did not work/.test(e.message));
  const [a, b] = await Promise.all([c.call({type: 'Render', target: '/a/'}), c.call({type: 'Run', request_id: 'r1', command: '{}'})]);
  assert.match(a.html, /\/a\//);
  assert.equal(JSON.parse(b.outcome).data.echo, 'r1');
  assert.ok(links.includes('reconnecting'));
  const hanging = c.call({type: 'Render', target: '/hang/'});
  await c.call({type: 'Render', target: '/exit/'}).catch(e => assert.equal(e.code, 'lost'));
  await assert.rejects(hanging, e => e.code === 'lost', 'a pending call is not left waiting forever');
  assert.ok(links.includes('lost'));
  c.stop();
});

test('a missing helper is reported instead of being searched for', () => {
  const c = remoteClient.child({helper: path.join(os.tmpdir(), 'no-such-fb-remote')});
  assert.throws(() => c.call({type: 'Ping'}), /connection helper is missing/);
});

function fakeStorage(backend = 'gnome_libsecret') {
  return {
    isEncryptionAvailable: () => true,
    getSelectedStorageBackend: () => backend,
    encryptString: s => Buffer.from('sealed:' + Buffer.from(s).toString('base64')),
    decryptString: b => { const t = b.toString(); if (!t.startsWith('sealed:')) throw new Error('no'); return Buffer.from(t.slice(7), 'base64').toString(); },
  };
}
const writePrivate = (file, data) => { fs.mkdirSync(path.dirname(file), {recursive: true}); fs.writeFileSync(file, data, {mode: 0o600}); };
const RECORD = {addr: '192.168.1.20:47100', host_name: 'Office PC', host_pin: 'a'.repeat(64), fingerprint: 'AB12 CD34', device_id: 'dev1', identity: {cert: '0a0b', key: '0c0d'}};

test('the pairing record is sealed, never stored without a real keyring, and summarized without secrets', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'fb-remote-store-'));
  try {
    const s = remoteClient.store({dir, safeStorage: fakeStorage(), writePrivate});
    assert.equal(s.load(), null);
    s.save(RECORD);
    const raw = fs.readFileSync(path.join(dir, remoteClient.RECORD), 'utf8');
    assert.ok(!raw.includes('0c0d') && !raw.includes('Office PC'), 'sealed on disk');
    assert.deepEqual(s.load(), RECORD);
    assert.deepEqual(s.summary(), {addr: RECORD.addr, host_name: 'Office PC', fingerprint: 'AB12 CD34'});
    assert.throws(() => s.save({...RECORD, host_pin: 'nothex'}), /Invalid pairing/);
    fs.writeFileSync(path.join(dir, remoteClient.RECORD), 'garbage');
    assert.equal(s.load(), null, 'an unreadable record is no pairing');
    s.forget();
    assert.equal(fs.existsSync(path.join(dir, remoteClient.RECORD)), false);
    if (process.platform === 'linux') {
      const plain = remoteClient.store({dir, safeStorage: fakeStorage('basic_text'), writePrivate});
      assert.equal(plain.usable(), false);
      assert.throws(() => plain.save(RECORD), /keyring is not available/);
    }
  } finally { fs.rmSync(dir, {recursive: true, force: true}); }
});

function fakeClient(answers) {
  const calls = [];
  return {
    calls,
    stopped: 0,
    async connect() { return {type: 'Session', session: 's', signed_in: null, host_name: 'Office PC', resumed: false}; },
    async pair() { return {identity: RECORD.identity, host_pin: RECORD.host_pin, device_id: 'dev1', host_name: 'Office PC', fingerprint: 'AB12 CD34'}; },
    async call(request) {
      calls.push(request);
      const next = answers.shift();
      if (next instanceof Error) throw next;
      return typeof next === 'function' ? next(request) : next;
    },
    stop() { this.stopped++; },
  };
}
function memoryStore(record = RECORD, usable = true) {
  let saved = record;
  return {usable: () => usable, load: () => saved, summary: () => saved && {addr: saved.addr, host_name: saved.host_name, fingerprint: saved.fingerprint}, save: r => { saved = r; }, forget: () => { saved = null; }};
}
const sent = [];
const fakeWindow = {isDestroyed: () => false, webContents: {send: (...a) => sent.push(a), print: (_o, cb) => cb(true)}};
function mode(answers, store = memoryStore()) {
  const client = fakeClient(answers);
  const m = remoteMode.create({client, store, window: () => fakeWindow, output: {write() {}}, dialog: {showSaveDialog: async () => ({canceled: true})}});
  return {m, client};
}
const form = (path, fields = []) => ({type: 'SubmitForm', path, token: 't', fields});

test('pages carry the connected-host bar, with the host name escaped', () => {
  const html = remoteMode.decorate('<html><head><title>x</title></head><body class="a"><p>hi</p></body></html>', 'Dana <script>', 'connected');
  assert.match(html, /<body class="a"><div id="fb-remote-bar"[^>]*>Connected to <strong>Dana &lt;script&gt;<\/strong>/);
  assert.match(html, /<link rel="stylesheet" href="\/static\/remote.css"><\/head>/);
  assert.doesNotMatch(html, /<script>/);
});

test('an unanswered command keeps its request id for an identical resubmission only', async () => {
  const lost = Object.assign(new Error('gone'), {code: 'lost'});
  const done = {type: 'Outcome', outcome: JSON.stringify({outcome: 'done', data: {}})};
  const {m, client} = mode([lost, done, done, done]);
  await m.connectSaved();
  const post = form('/c/x/lists/customers/new/', [['display_name', 'A']]);
  await assert.rejects(m.run(post), /may or may not have reached it.*will not be done twice/);
  await m.run(post);
  assert.equal(client.calls[0].request_id, client.calls[1].request_id, 'the retry of an unanswered post reuses its id');
  await m.run(post);
  assert.notEqual(client.calls[2].request_id, client.calls[1].request_id, 'an answered post is new when sent again');
  await m.run(form('/c/x/lists/customers/new/', [['display_name', 'B']]));
  assert.notEqual(client.calls[3].request_id, client.calls[2].request_id);
  for (const c of client.calls) assert.match(c.request_id, /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);
});

test('host answers are checked: addresses stay local, host-only work is refused, forms re-render once', async () => {
  const {m} = mode([
    {type: 'Outcome', outcome: JSON.stringify({outcome: 'redirect', location: 'https://evil.example/'})},
    {type: 'Outcome', outcome: JSON.stringify({outcome: 'invalid', html: '<html><head></head><body>errors</body></html>'})},
    {type: 'Refused', code: 'unauthenticated', message: 'ended'},
    {type: 'Refused', code: 'host_only', message: 'This can only be done on the host computer.'},
    {type: 'Outcome', outcome: JSON.stringify({outcome: 'external', url: 'https://x', location: '/'})},
  ]);
  await m.connectSaved();
  await assert.rejects(m.run(form('/a/')), /invalid address/);
  assert.deepEqual(await m.run(form('/b/')), {outcome: 'invalid', reload: '/b/'});
  const again = await m.render('/b/');
  assert.match(again.html, /errors/);
  assert.match(again.html, /fb-remote-bar/);
  assert.deepEqual(await m.run(form('/c/')), {outcome: 'redirect', location: '/desktop/sign-in/'});
  await assert.rejects(m.run(form('/d/')), /only be done on the host/);
  await assert.rejects(m.run(form('/e/')), /only be done on the host/);
  await assert.rejects(m.run({type: 'ExportBackup', company_id: 'x', name: 'y'}), /only be done on the host/);
});

test('screens: redirects stay local, refusals explain, a refused device disconnects', async () => {
  const notPaired = Object.assign(new Error('The host did not accept this computer.'), {code: 'not_paired'});
  const {m} = mode([
    {type: 'Page', html: '//evil.example/', status: 303, route: ''},
    {type: 'Page', html: '/desktop/sign-in/', status: 303, route: 'sign-in'},
    {type: 'Refused', code: 'host_only', message: 'This can only be done on the host computer.'},
    notPaired,
  ]);
  assert.deepEqual(await m.render('/c/x/home/'), {redirect: '/remote/connect/'}, 'not connected yet');
  await m.connectSaved();
  assert.equal((await m.render('/c/x/home/')).status, 502);
  assert.deepEqual(await m.render('/c/x/home/'), {redirect: '/desktop/sign-in/'});
  assert.match((await m.render('/settings/book-protection/')).html, /Only on the host computer/);
  const r = await m.render('/c/x/home/');
  assert.equal(r.status, 503);
  assert.match(r.html, /did not accept this computer/);
  assert.equal(m.active(), false);
});

test('pairing needs a usable keyring and saves the record before connecting', async () => {
  const blocked = mode([], memoryStore(null, false)).m;
  assert.match((await blocked.pair({addr: '192.168.1.20:47100', code: 'X', device_name: 'Laptop'})).errors[0], /keyring is not available/);
  const store = memoryStore(null);
  const {m} = mode([], store);
  assert.deepEqual(await m.pair({addr: ' 192.168.1.20:47100 ', code: 'X', device_name: 'Laptop'}), {location: '/desktop/sign-in/'});
  assert.equal(store.load().addr, '192.168.1.20:47100');
  assert.equal(m.active(), true);
  assert.equal(m.host(), 'Office PC');
  await m.forget();
  assert.equal(store.load(), null);
  assert.equal(m.active(), false);
});

test('the connect screen shows saved hosts without secrets and escapes errors', async () => {
  const {m} = mode([]);
  m.flash(['<b>bad</b>']);
  const page = (await m.render('/remote/connect/')).html;
  assert.match(page, /This computer is paired with Office PC/);
  assert.match(page, /&lt;b&gt;bad&lt;\/b&gt;/);
  assert.doesNotMatch(page, /0c0d|aaaaaaaa/);
  assert.doesNotMatch((await m.render('/remote/connect/')).html, /bad/, 'errors show once');
});

// S3: a lock while the host cannot be reached fails closed.
test('lock replaces the page at once and drops the session when the host cannot confirm it', async () => {
  const lost = Object.assign(new Error('gone'), {code: 'lost'});
  const {m, client} = mode([lost]);
  await m.connectSaved();
  const shown = [];
  assert.equal(await m.lock(location => shown.push(location)), false);
  assert.deepEqual(shown, [remoteMode.LOCKED, remoteMode.CONNECT], 'the locked page first, then the connect screen');
  assert.equal(client.stopped, 1, 'the helper (and with it the session token) is gone');
  assert.equal(m.active(), false, 'the next connection is a new session');
  assert.match((await m.render('/remote/connect/')).html, /could not confirm the sign-out/);
  assert.deepEqual(await m.render('/c/x/home/'), {redirect: '/remote/connect/'});
});

test('lock signs out on the host and shows nothing from it until the sign-out is confirmed', async () => {
  let answer;
  const {m, client} = mode([() => new Promise(resolve => { answer = resolve; })]);
  await m.connectSaved();
  const shown = [];
  const locked = m.lock(location => shown.push(location));
  assert.deepEqual(shown, [remoteMode.LOCKED]);
  assert.match((await m.render('/c/x/home/')).html, /Locked/, 'a page load during the lock shows no host content');
  await assert.rejects(m.run(form('/c/x/journal/new/')), /locked/);
  assert.equal(client.calls.length, 1, 'only the sign-out went to the host');
  answer({type: 'SignedOut'});
  assert.equal(await locked, true);
  assert.deepEqual(shown, [remoteMode.LOCKED, '/desktop/sign-in/']);
  assert.equal(client.stopped, 0);
  assert.equal(m.active(), true, 'still connected; the host asks for a sign-in');
});

test('a refused or unexpected sign-out answer also fails closed', async () => {
  const {m, client} = mode([{type: 'Refused', code: 'busy', message: 'x'}]);
  await m.connectSaved();
  assert.equal(await m.lock(), false);
  assert.equal(client.stopped, 1);
  assert.equal(m.active(), false);
});

// S6: the host never chooses where an export is saved.
test('an export suggests only a safe file name and must be saved as .csv', async () => {
  for (const [given, want] of [
    ['/home/dana/.config/autostart/update.desktop', 'update.desktop.csv'],
    ['..\\..\\Startup\\run.bat', 'run.bat.csv'],
    ['report.CSV', 'report.csv'],
    ['  .hidden', 'hidden.csv'],
    ['a‮b.csv', 'a_b.csv'],
    ['', 'export.csv'],
    ['Profit and loss (2026).csv', 'Profit and loss (2026).csv'],
  ]) assert.equal(remoteMode.exportName(given), want, given);
  assert.equal(remoteMode.exportName('x'.repeat(500)).length, 104);
  const exported = {type: 'Outcome', outcome: JSON.stringify({outcome: 'export', filename: '/home/dana/.config/autostart/update.desktop', contents: 'a,b'})};
  const asked = [];
  const written = [];
  const client = fakeClient([exported, exported]);
  let choice = '/home/dana/Documents/update.desktop';
  const m = remoteMode.create({client, store: memoryStore(), window: () => fakeWindow, output: {write: (p, c) => written.push([p, c])},
    dialog: {showSaveDialog: async (_w, options) => { asked.push(options.defaultPath); return {canceled: false, filePath: choice}; }}});
  await m.connectSaved();
  await assert.rejects(m.run({type: 'ExportHistory', company_id: 'x'}), /ending in \.csv/);
  assert.deepEqual(written, [], 'nothing written outside a .csv name');
  choice = '/home/dana/Documents/history.csv';
  assert.deepEqual(await m.run({type: 'ExportHistory', company_id: 'x'}), {outcome: 'done', data: {saved: true}});
  assert.deepEqual(asked, ['update.desktop.csv', 'update.desktop.csv'], 'no folder from the host');
  assert.deepEqual(written, [['/home/dana/Documents/history.csv', 'a,b']]);
});

// N7: pages at host addresses carry no pairing or disconnect forms; disconnecting signs out.
test('notice pages link to the disconnect screen, which alone carries the form, and disconnect signs out', async () => {
  const {m, client} = mode([{type: 'Refused', code: 'host_only', message: 'This can only be done on the host computer.'}, {type: 'SignedOut'}]);
  await m.connectSaved();
  const notice = (await m.render('/settings/recovery-key/')).html;
  assert.doesNotMatch(notice, /data-gate/);
  assert.match(notice, /href="\/remote\/disconnect\/"/);
  assert.match(notice, /href="\/">Go to the books’ home page/, 'a host-only landing is not a dead end');
  assert.match((await m.render('/remote/disconnect/')).html, /data-gate="remote-disconnect"/);
  await m.disconnect();
  assert.deepEqual(client.calls.at(-1), {type: 'SignOut'});
  assert.equal(client.stopped, 1);
  assert.deepEqual(await m.render('/remote/disconnect/'), {redirect: '/remote/connect/'});
});

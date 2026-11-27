'use strict';
// Remote access, client side: the child-process protocol, the sealed pairing record, and client
// mode's handling of host answers (run with plain Node: npm test). Synthetic values only.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawn } = require('node:child_process');
const { EventEmitter } = require('node:events');
const { PassThrough } = require('node:stream');
const remoteClient = require('../main/remote-client.cjs');
const remoteMode = require('../main/remote-mode.cjs');
const powerLock = require('../main/power-lock.cjs');

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

test('a stopped helper cannot reject the replacement helper’s calls or change its link', async () => {
  const processes = [];
  const links = [];
  const start = () => {
    const proc = Object.assign(new EventEmitter(), {stdin: new PassThrough(), stdout: new PassThrough(), kill() {}});
    processes.push(proc);
    return proc;
  };
  const client = remoteClient.child({helper: FAKE, start, onLink: link => links.push(link)});
  const old = client.call({type: 'Ping'});
  client.stop();
  await assert.rejects(old, /Disconnected/);
  const next = client.call({type: 'Ping'});
  processes[0].stdout.write(remoteClient.encode({kind: 'link', state: 'lost'}));
  processes[0].emit('exit');
  processes[1].stdout.write(remoteClient.encode({kind: 'answer', id: 2, response: {type: 'Pong'}}));
  assert.deepEqual(await next, {type: 'Pong'});
  assert.deepEqual(links, []);
  client.stop();
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

const hostSession = signedIn => ({type: 'Session', session: 's', signed_in: signedIn ? {user: 'Synthetic user'} : null, host_name: 'Office PC', resumed: false});
const networkError = code => Object.assign(new Error('Failed'), {code});
const refused = code => ({type: 'Refused', code, message: 'Your session on the host ended.'});
const outcome = response => ({type: 'Outcome', outcome: JSON.stringify(response)});
const redirect = location => outcome({outcome: 'redirect', location});
const SIGN_IN = '/desktop/sign-in/';
const HOME = '/c/x/home/';
const fixturePage = {type: 'Page', html: '<head><title>Test page</title></head><body>Test page</body>', status: 200};
const markerOf = async (mode, target = SIGN_IN) => {
  const rendered = await mode.render(target);
  assert.match(rendered.marker, /^[a-f0-9-]{36}$/);
  assert.match(rendered.html, new RegExp(`<meta name="fb-remote-page" content="${rendered.marker}">`));
  return rendered.marker;
};

function fakeClient(answers, connections = []) {
  const calls = [], dials = [];
  return {
    calls, dials,
    stopped: 0,
    async connect(...args) {
      dials.push(args);
      const next = connections.shift();
      if (next instanceof Error) throw next;
      return typeof next === 'function' ? next() : next || hostSession(dials.length === 1);
    },
    async pair() { return {identity: RECORD.identity, host_pin: RECORD.host_pin, device_id: 'dev1', host_name: 'Office PC', fingerprint: 'AB12 CD34'}; },
    async call(request) {
      if (request.type === 'Render' && request.target === SIGN_IN && this.markerProbe) {
        this.markerProbe = false;
        return fixturePage;
      }
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
function mode(answers, {store = memoryStore(), connections, ...options} = {}) {
  const client = fakeClient(answers, connections);
  // `shown`: pages the window is sent to. `offered`: [page, only if the window shows this one].
  const shown = [], offered = [];
  const m = remoteMode.create({client, store, show: (p, expected) => { if (expected) offered.push([p, expected]); else shown.push(p); }, window: () => fakeWindow,
    output: {write() {}}, dialog: {showSaveDialog: async () => ({canceled: true})}, ...options});
  const marker = () => { client.markerProbe = true; return markerOf(m); };
  return {m, client, shown, offered, marker};
}
const form = (path, fields = []) => ({type: 'SubmitForm', path, token: 't', fields});

test('pages carry the connected-host bar, with the host name escaped', () => {
  const html = remoteMode.decorate('<html><head><title>FreedomBooks · Local ledger</title></head><body class="a"><p>hi</p></body></html>', 'Dana <script>', 'connected');
  assert.match(html, /<body class="a"><div id="fb-remote-bar"[^>]*>Connected to <strong>Dana &lt;script&gt;<\/strong>/);
  assert.match(html, /<link rel="stylesheet" href="\/static\/remote.css"><\/head>/);
  assert.doesNotMatch(html, /<script>|Local ledger/);
});

test('an unanswered command keeps its request id for an identical resubmission only', async () => {
  const lost = Object.assign(new Error('gone'), {code: 'lost'});
  const done = {type: 'Outcome', outcome: JSON.stringify({outcome: 'done', data: {}})};
  const {m, client, marker} = mode([lost, done, done, done]);
  await m.connectSaved();
  const page = await marker();
  const post = form('/c/x/lists/customers/new/', [['display_name', 'A']]);
  await assert.rejects(m.run(post, page), /may or may not have reached it.*will not be done twice/);
  await m.run(post, page);
  assert.equal(client.calls[0].request_id, client.calls[1].request_id, 'the retry of an unanswered post reuses its id');
  await m.run(post, page);
  assert.notEqual(client.calls[2].request_id, client.calls[1].request_id, 'an answered post is new when sent again');
  await m.run(form('/c/x/lists/customers/new/', [['display_name', 'B']]), page);
  assert.notEqual(client.calls[3].request_id, client.calls[2].request_id);
  for (const c of client.calls) assert.match(c.request_id, /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);
});

test('switching users expires worksheet URLs, old pages and pending exports on the same connection', async () => {
  let choose;
  const saving = new Promise(resolve => { choose = resolve; });
  let writes = 0;
  const {m, marker} = mode([
    outcome({outcome:'export', filename:'a.csv', contents:'user A'}),
    redirect(SIGN_IN), redirect(HOME), fixturePage,
  ], {dialog:{showSaveDialog: () => saving}, output:{write: () => writes++}});
  await m.connectSaved();
  const a = await marker();
  const pendingSave = m.run({type:'ExportReport'}, a);
  const worksheet = '/c/x/reports/schedule-c/?mappings=private-A';
  await m.run(form('/desktop/session/', [['action','signout']]), a);
  assert.equal(m.signedIn(), false);
  const signInPage = await marker();
  assert.notEqual(signInPage, a);
  await m.run(form(SIGN_IN, [['action','signin']]), signInPage);
  assert.equal(m.signedIn(), true);
  assert.notEqual(m.printMarker(), signInPage);
  assert.deepEqual(await m.render(worksheet, a), {redirect:'/'});
  assert.deepEqual(await m.run(form(HOME), a), {outcome:'redirect', location:'/'});
  choose({filePath:'/chosen/a.csv'});
  await assert.rejects(pendingSave, /session changed/);
  assert.equal(writes, 0);
  assert.equal((await m.render(HOME, m.printMarker())).status, 200);
});

test('a sign-out with a lost reply invalidates the previous marker before reconnecting', async () => {
  const timeout = Object.assign(new Error('No reply'), {code:'timeout'});
  const {m, marker} = mode([timeout]);
  await m.connectSaved();
  const previous = await marker();
  const result = await m.run(form('/desktop/session/', [['action','signout']]), previous);
  assert.equal(result.location, remoteMode.LOCKED);
  assert.equal(m.signedIn(), false);
  assert.equal(m.accepts(previous), false);
});

test('host marker stamping canonicalizes repeated markers and preserves query encoding', () => {
  const key = remoteMode.PAGE_KEY;
  const url = new URL('fb://app/r/?a=~%2A&' + key + '=old&' + key + '=new');
  assert.deepEqual(remoteMode.pageTarget(url), {target:'/r/?a=~%2A', marker:''});
  assert.match(remoteMode.decorate('<HTML><HEAD></HEAD><BODY></BODY></HTML>', 'host', 'connected', 'm'), /remote\.css"><\/head>/);
});

test('host answers are checked: addresses stay local, host-only work is refused, forms re-render once', async () => {
  const {m, marker} = mode([
    {type: 'Outcome', outcome: JSON.stringify({outcome: 'redirect', location: 'https://evil.example/'})},
    {type: 'Outcome', outcome: JSON.stringify({outcome: 'invalid', html: '<html><head></head><body>errors</body></html>'})},
    {type: 'Refused', code: 'host_only', message: 'This can only be done on the host computer.'},
    {type: 'Outcome', outcome: JSON.stringify({outcome: 'external', url: 'https://x', location: '/'})},
    {type: 'Refused', code: 'sign_in_required', message: 'ended'},
  ]);
  await m.connectSaved();
  const page = await marker();
  await assert.rejects(m.run(form('/a/'), page), /invalid address/);
  assert.deepEqual(await m.run(form('/b/'), page), {outcome: 'invalid', reload: '/b/'});
  const again = await m.render('/b/');
  assert.match(again.html, /errors/);
  assert.match(again.html, /fb-remote-bar/);
  await assert.rejects(m.run(form('/d/'), page), /only be done on the host/);
  await assert.rejects(m.run(form('/e/'), page), /only be done on the host/);
  await assert.rejects(m.run({type: 'ExportBackup', company_id: 'x', name: 'y'}, page), /only be done on the host/);
  assert.deepEqual(await m.run(form('/c/'), page), {outcome: 'redirect', location: SIGN_IN});
});

test('screens: redirects stay local, refusals explain, a refused device disconnects', async () => {
  const notPaired = Object.assign(new Error('The host did not accept this computer.'), {code: 'not_paired'});
  const {m} = mode([
    {type: 'Page', html: '//evil.example/', status: 303, route: ''},
    {type: 'Page', html: '/expected/', status: 303, route: 'home'},
    {type: 'Refused', code: 'host_only', message: 'This can only be done on the host computer.'},
    notPaired,
  ]);
  assert.deepEqual(await m.render('/c/x/home/'), {redirect: '/remote/connect/'}, 'not connected yet');
  await m.connectSaved();
  assert.equal((await m.render('/c/x/home/')).status, 502);
  assert.equal((await m.render(HOME)).redirect, '/expected/');
  assert.match((await m.render('/settings/book-protection/')).html, /Only on the host computer/);
  assert.deepEqual(await m.render(HOME), {redirect: remoteMode.CONNECT});
  assert.match((await m.render(remoteMode.CONNECT)).html, /did not accept this computer/);
  assert.equal(m.active(), false);
});

test('pairing needs a usable keyring and saves the record before connecting', async () => {
  const blocked = mode([], {store: memoryStore(null, false)}).m;
  assert.match((await blocked.pair({addr: '192.168.1.20:47100', code: 'X', device_name: 'Laptop'})).errors[0], /keyring is not available/);
  const store = memoryStore(null);
  const {m} = mode([], {store, connections: [hostSession(false)]});
  assert.deepEqual(await m.pair({addr: ' 192.168.1.20:47100 ', code: 'X', device_name: 'Laptop'}), {location: SIGN_IN, navigated: true});
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
  let answer, restored;
  const {m, client, shown, marker} = mode([() => new Promise((_resolve, reject) => { answer = reject; })], {
    connections: [hostSession(true), () => new Promise(resolve => { restored = resolve; })],
  });
  await m.connectSaved();
  shown.length = 0;
  const oldPage = await marker();
  const lock = m.lock();
  assert.deepEqual(shown, [remoteMode.LOCKED], 'the page is replaced before the host answers');
  assert.deepEqual(await m.render(HOME), {redirect: remoteMode.LOCKED});
  assert.deepEqual(await m.run(form(HOME), oldPage), {outcome: 'redirect', location: remoteMode.LOCKED});
  answer(networkError('lost'));
  assert.equal(await lock, false);
  assert.equal(client.stopped, 1, 'the helper and its session token are gone');
  assert.equal(m.active(), true, 'the window still belongs to the paired host');
  const wait = await m.render(remoteMode.LOCKED);
  assert.match(wait.html, /Locked\. Reconnecting to Office PC/);
  assert.match(wait.html, /Try now/);
  assert.match(wait.html, /Disconnect/);
  assert.doesNotMatch(wait.html, /0c0d|could not confirm/);
  assert.deepEqual(await m.render(HOME), {redirect: remoteMode.LOCKED});
  assert.deepEqual(client.dials[1], [RECORD.addr, RECORD.identity, RECORD.host_pin]);
  assert.equal(client.dials.length, 2, 'the page and the timer share the attempt');
  const reconnecting = m.refresh();
  restored(hostSession(false));
  await reconnecting;
  assert.deepEqual(shown, [remoteMode.LOCKED, remoteMode.LOCKED, SIGN_IN]);
  assert.deepEqual(await m.render(HOME), {redirect: SIGN_IN});
  assert.deepEqual(await m.run({type: 'ExportHistory'}), {outcome: 'redirect', location: SIGN_IN});
  assert.deepEqual(client.calls, [{type: 'SignOut'}]);
});

test('lock keeps book content hidden until sign-out and fresh password sign-in', async () => {
  let answer, cover;
  const shown = [];
  const {m, client, marker} = mode([
    () => new Promise(resolve => { answer = resolve; }),
    redirect(HOME),
    {type: 'Page', html: '<body>Synthetic customer</body>', status: 200},
  ], {show: p => {
    shown.push(p);
    if (p === remoteMode.LOCKED) return new Promise(resolve => { cover = resolve; });
  }});
  await m.connectSaved();
  shown.length = 0;
  const oldPage = await marker();
  const locked = m.lock();
  assert.deepEqual(shown, [remoteMode.LOCKED]);
  assert.match((await m.render(remoteMode.LOCKED)).html, /Signing you out/);
  assert.deepEqual(await m.render(HOME), {redirect: remoteMode.LOCKED});
  assert.deepEqual(await m.run(form(HOME), oldPage), {outcome: 'redirect', location: remoteMode.LOCKED});
  answer({type: 'SignedOut'});
  await Promise.resolve();
  assert.deepEqual(shown, [remoteMode.LOCKED], 'sign-in waits for the local lock page to load');
  cover();
  assert.equal(await locked, true);
  assert.deepEqual(shown, [remoteMode.LOCKED, SIGN_IN]);
  assert.equal(client.stopped, 0);
  assert.deepEqual(await m.render(HOME), {redirect: SIGN_IN});
  assert.deepEqual(await m.run(form(SIGN_IN, [['action', 'signin'], ['password', 'Synthetic password']]), await marker()), {outcome: 'redirect', location: HOME});
  assert.match((await m.render(HOME)).html, /Synthetic customer/);
});

test('overlapping and subsequent locks leave the host sign-in page alone', async () => {
  let answer;
  const {m, client, shown} = mode([
    () => new Promise(resolve => { answer = resolve; }),
    refused('unauthenticated'),
  ]);
  await m.connectSaved();
  shown.length = 0;
  const first = m.lock();
  const overlapping = m.lock();
  answer({type: 'SignedOut'});
  await Promise.all([first, overlapping]);
  await m.lock();
  assert.deepEqual(client.calls, [{type: 'SignOut'}]);
  assert.deepEqual(shown, [remoteMode.LOCKED, SIGN_IN]);
  assert.equal(client.stopped, 0);
});

test('locking a freshly connected signed-out session is a no-op', async () => {
  const {m, client, shown} = mode([], {connections: [hostSession(false)]});
  await m.connectSaved();
  shown.length = 0;
  await m.lock();
  assert.deepEqual(client.calls, []);
  assert.deepEqual(shown, []);
});

test('lock clears an MFA challenge but leaves a completed manual sign-out alone', async () => {
  const {m, client, shown, marker} = mode([redirect(SIGN_IN), {type: 'SignedOut'}, redirect(HOME), redirect(SIGN_IN)], {
    connections: [hostSession(false)],
  });
  await m.connectSaved();
  shown.length = 0;
  await m.run(form(SIGN_IN, [['action', 'signin']]), await marker());
  await m.lock();
  assert.deepEqual(shown, [remoteMode.LOCKED, SIGN_IN], 'the password-verified challenge is signed out');
  const page = await marker();
  await m.run(form(SIGN_IN, [['action', 'signin']]), page);
  await m.run(form('/desktop/session/', [['action', 'signout']]), await marker());
  await m.lock();
  assert.equal(client.calls.filter(r => r.type === 'SignOut').length, 1);
  assert.deepEqual(shown, [remoteMode.LOCKED, SIGN_IN]);
});

test('an expired or unknown session confirms lock and starts a fresh session', async () => {
  const {m, client, shown} = mode([refused('unauthenticated')]);
  await m.connectSaved();
  shown.length = 0;
  assert.equal(await m.lock(), true);
  await m.refresh();
  assert.deepEqual(shown, [remoteMode.LOCKED, remoteMode.LOCKED, SIGN_IN]);
  assert.equal(client.stopped, 1);
  assert.equal(client.dials.length, 2);
});

test('sign-in-required confirms lock without discarding the live session', async () => {
  const {m, client, shown} = mode([refused('sign_in_required')]);
  await m.connectSaved();
  shown.length = 0;
  assert.equal(await m.lock(), true);
  assert.deepEqual(shown, [remoteMode.LOCKED, SIGN_IN]);
  assert.equal(client.stopped, 0);
});

test('Try now retries a refusal without reusing the old session', async () => {
  const {m, client, shown} = mode([refused('busy')], {
    connections: [hostSession(true), networkError('unreachable'), {...hostSession(true), resumed: true}, hostSession(false)],
  });
  await m.connectSaved();
  assert.equal(await m.lock(), false);
  await m.refresh();
  assert.match((await m.render(remoteMode.LOCKED + '?retry=1')).html, /Locked\. Reconnecting/);
  await m.refresh();
  assert.equal(shown.at(-1), remoteMode.LOCKED, 'refuses a resumed authenticated session');
  assert.match((await m.render(remoteMode.LOCKED + '?retry=1')).html, /Locked\. Reconnecting/);
  await m.refresh();
  assert.equal(shown.at(-1), SIGN_IN);
  assert.equal(client.dials.length, 4);
});

test('Disconnect cancels in-flight recovery and does not send another sign-out', async () => {
  let answer;
  const {m, client, shown, offered} = mode([networkError('lost')], {
    connections: [hostSession(true), () => new Promise(resolve => { answer = resolve; })],
  });
  await m.connectSaved();
  await m.lock();
  const recovering = m.refresh();
  assert.match((await m.render(remoteMode.DISCONNECT)).html, /data-gate="remote-disconnect"/);
  await m.disconnect();
  answer(hostSession(false));
  await recovering;
  assert.deepEqual(client.calls, [{type: 'SignOut'}]);
  assert.equal(m.active(), false);
  assert.equal(shown.at(-1), remoteMode.LOCKED);
  assert.deepEqual(offered, [[remoteMode.CONNECT, remoteMode.CONNECTING]], 'a window left on the cover goes to Connect, any other page stays');
});

// Review of c5a1e486, Nit 4: a Disconnect waits up to five seconds for the host. A window sent to
// the cover in that time used to stay on "Connecting…".
test('a window sent to the cover while Disconnect waits is told it is disconnecting, then gets Connect', async () => {
  let answer;
  const {m, offered} = mode([() => new Promise(resolve => { answer = resolve; })]);
  await m.connectSaved();
  const leaving = m.disconnect();
  const cover = await m.render(remoteMode.CONNECTING);
  assert.match(cover.html, /Disconnecting from the host/);
  assert.deepEqual(await m.render(remoteMode.CONNECT), {redirect: remoteMode.CONNECTING});
  assert.deepEqual(offered, []);
  answer({type: 'SignedOut'});
  await leaving;
  assert.deepEqual(offered, [[remoteMode.CONNECT, remoteMode.CONNECTING]]);
  assert.match((await m.render(remoteMode.CONNECT)).html, /Connect to a FreedomBooks host/);
});

test('recovery goes to Connect with a specific error only when the pairing cannot be used', async () => {
  for (const [error, message] of [['host_mismatch', /not the host you paired with/], ['not_paired', /did not accept this computer/]]) {
    const {m, shown} = mode([networkError('lost')], {connections: [hostSession(true), networkError(error)]});
    await m.connectSaved();
    await m.lock();
    await m.refresh();
    assert.equal(shown.at(-1), remoteMode.CONNECT);
    assert.match((await m.render(remoteMode.CONNECT)).html, message);
  }
  const store = memoryStore();
  const {m, shown} = mode([() => { store.forget(); throw networkError('lost'); }], {store});
  await m.connectSaved();
  await m.lock();
  assert.equal(shown.at(-1), remoteMode.CONNECT);
  assert.match((await m.render(remoteMode.CONNECT)).html, /could not read the saved pairing/);
});

test('a dropped transport resumes a valid session and reloads only a failed page', async () => {
  const reloaded = [];
  const {m, client} = mode([networkError('lost'), networkError('lost'), {type: 'Pong'}], {show: (...args) => reloaded.push(args)});
  await m.connectSaved();
  reloaded.length = 0;
  assert.equal((await m.render(HOME)).status, 503);
  await m.refresh();
  assert.deepEqual(reloaded, []);
  m.setLink('lost');
  await m.refresh();
  assert.deepEqual(reloaded, [[HOME, HOME]]);
  assert.equal(m.link(), 'connected');
  assert.equal(client.dials.length, 1, 'the helper resumed the existing session');
});

test('when the host restarts the active probe detects session loss and shows sign-in', async () => {
  const {m, client, shown} = mode([refused('unauthenticated')]);
  await m.connectSaved();
  shown.length = 0;
  await m.refresh();
  assert.deepEqual(shown, [remoteMode.LOCKED]);
  await m.render(remoteMode.LOCKED);
  await m.refresh();
  assert.equal(shown.at(-1), SIGN_IN);
  assert.equal(client.dials.length, 2);
});

test('a lock invalidates late host content, pending exports and print responses', async () => {
  const answers = [];
  const written = [];
  let dialogOpened, finishDialog;
  const dialogReady = new Promise(resolve => { dialogOpened = resolve; });
  const deferred = () => new Promise(resolve => answers.push(resolve));
  const {m, marker} = mode([deferred, deferred, deferred, {type: 'SignedOut'}], {
    dialog: {showSaveDialog: () => new Promise(resolve => { finishDialog = resolve; dialogOpened(); })},
    output: {write: (...args) => written.push(args)},
    window: () => ({...fakeWindow, webContents: {...fakeWindow.webContents, print: () => assert.fail('printed after lock')}}),
  });
  await m.connectSaved();
  const onScreen = await marker();
  const page = m.render(HOME);
  const print = assert.rejects(m.run(form('/print/'), onScreen), /session changed/);
  const saved = assert.rejects(m.run({type: 'ExportHistory'}, onScreen), /session changed/);
  answers[2](outcome({outcome: 'export', filename: 'synthetic.csv', contents: 'Synthetic customer'}));
  await dialogReady;
  await m.lock();
  answers[0]({type: 'Page', html: 'Synthetic customer', status: 200});
  answers[1](outcome({outcome: 'done', data: {desktop_print: true}}));
  finishDialog({filePath: '/synthetic.csv'});
  assert.deepEqual(await page, {redirect: SIGN_IN});
  await Promise.all([print, saved]);
  assert.deepEqual(written, []);
});

test('a lock discards a session whose sign-in is still in flight', async () => {
  let answer;
  const {m, client, shown, marker} = mode([() => new Promise(resolve => { answer = resolve; })], {
    connections: [hostSession(false), hostSession(false)],
  });
  await m.connectSaved();
  const login = assert.rejects(m.run(form(SIGN_IN, [['action', 'signin']]), await marker()), /session changed/);
  assert.equal(await m.lock(), false);
  const recovered = m.refresh();
  answer(redirect(HOME));
  await login;
  await recovered;
  assert.equal(client.stopped, 1);
  assert.equal(shown.at(-1), SIGN_IN);
  assert.deepEqual(await m.render(HOME), {redirect: SIGN_IN});
});

const HOST_B = {...RECORD, addr: '127.0.0.1:47102', device_id: 'dev2', host_name: 'Second synthetic host', host_pin: 'b'.repeat(64)};
const PAIR_B = {addr: HOST_B.addr, code: 'SYNTHETIC', device_name: 'Laptop'};

function twoHosts() {
  const pairReply = Promise.withResolvers(), connectStarted = Promise.withResolvers(), connectReply = Promise.withResolvers();
  const oldRender = Promise.withResolvers();
  const deliveries = [], shown = [];
  let target = null;
  const client = {
    pair: () => pairReply.promise,
    async connect(addr) {
      if (addr === HOST_B.addr) {
        connectStarted.resolve();
        return connectReply.promise;
      }
      target = RECORD.addr;
      return hostSession(false);
    },
    call(request) {
      deliveries.push([target, request]);
      if (request.target === SIGN_IN + '?held=1') return oldRender.promise;
      if (request.type === 'Render') return fixturePage;
      if (request.type === 'Run') return redirect(HOME);
      if (request.type === 'SignOut') return {type: 'SignedOut'};
      assert.fail(`Unexpected ${request.type}`);
    },
    stop() { target = null; },
  };
  const m = remoteMode.create({client, store: memoryStore(), window: () => fakeWindow, show: location => shown.push(location)});
  return {m, shown, deliveries, oldRender, pairReply, connectStarted, connectReply, target: () => target,
    handshake() { target = HOST_B.addr; },
  };
}

test('pair/connect block host traffic through helper adoption, and main replaces the page after Back', async () => {
  const {m, shown, deliveries, oldRender, pairReply, connectStarted, connectReply, target, handshake} = twoHosts();
  await m.connectSaved();
  const markerA = await markerOf(m);
  const latePage = m.render(SIGN_IN + '?held=1', markerA);
  const post = form(SIGN_IN, [['action', 'signin'], ['password', 'Synthetic host A password']]);
  const switching = m.pair(PAIR_B);
  const connecting = {redirect: remoteMode.CONNECTING};
  const blocked = {outcome: 'redirect', location: remoteMode.CONNECTING};
  const checkGate = async () => {
    assert.deepEqual(await m.render('/'), connecting);
    assert.deepEqual(await m.render(SIGN_IN + '?input=synthetic', markerA), connecting);
    assert.doesNotMatch((await m.render(remoteMode.CONNECTING)).html, /Office PC|Second synthetic host|Test page/);
    assert.equal((await m.render('/c/00000000-0000-0000-0000-000000000000/document-logo/', markerA)).status, 404);
    assert.deepEqual(m.admit(null), blocked);
    assert.deepEqual(await m.run(post, markerA), blocked);
    assert.deepEqual(await m.run({type: 'ExportHistory'}, markerA), blocked);
    await m.refresh();
    assert.deepEqual(deliveries.map(([addr, request]) => [addr, request.type, request.target]), [
      [RECORD.addr, 'Render', SIGN_IN], [RECORD.addr, 'Render', SIGN_IN + '?held=1'],
    ]);
  };
  assert.equal(target(), RECORD.addr);
  await checkGate();
  pairReply.resolve(HOST_B);
  await connectStarted.promise;
  assert.equal(target(), RECORD.addr, 'Pair and the start of Connect do not replace the helper client');
  await checkGate();
  handshake();
  assert.equal(target(), HOST_B.addr, 'the helper has switched before Electron receives its reply');
  await checkGate();
  connectReply.resolve(hostSession(false));
  await switching;
  assert.equal(shown.at(-1), SIGN_IN, 'main navigates even after the previous document tried to leave');
  oldRender.resolve(fixturePage);
  assert.deepEqual(await latePage, {redirect: SIGN_IN});
  const stale = {outcome: 'redirect', location: SIGN_IN};
  assert.deepEqual(await m.run(post, markerA), stale);
  assert.deepEqual(await m.run(post, null), stale);
  const pageB = await m.render(SIGN_IN);
  assert.notEqual(pageB.marker, markerA);
  assert.match(pageB.html, /Connected to <strong>Second synthetic host<\/strong>/);
  assert.match(pageB.html, /<title>Test page — connected to Second synthetic host<\/title>/);
  assert.deepEqual(await m.run(form(SIGN_IN, [['action', 'signin'], ['password', 'Synthetic host B password']]), pageB.marker), {outcome: 'redirect', location: HOME});
  assert.deepEqual(deliveries.slice(2).map(([addr, request]) => [addr, request.type]), [[HOST_B.addr, 'Render'], [HOST_B.addr, 'Run']]);
  assert.doesNotMatch(JSON.stringify(deliveries), /Synthetic host A password|input=synthetic/);
  await m.disconnect();
  assert.deepEqual(deliveries.at(-1), [HOST_B.addr, {type: 'SignOut'}]);
  assert.equal(m.admit(pageB.marker).location, remoteMode.CONNECT, 'stale pages cannot fall through to local commands');
});

test('failed and cancelled connection changes leave no active helper or stuck gate', async () => {
  for (const cancel of [false, true]) {
    const {m, pairReply, connectStarted, connectReply, target} = twoHosts();
    await m.connectSaved();
    const attempt = m.pair(PAIR_B);
    pairReply.resolve(HOST_B);
    await connectStarted.promise;
    if (cancel) await m.disconnect();
    connectReply.reject(networkError('lost'));
    await attempt;
    assert.equal(target(), null);
    assert.equal(m.active(), false);
    assert.equal((await m.connectSaved()).navigated, true);
    assert.equal((await m.pair(PAIR_B)).navigated, true);
    assert.equal(m.active(), false);
  }
});

test('macOS power events, window show and focus drive lock and immediate retry', () => {
  const powerMonitor = new EventEmitter(), window = new EventEmitter(), timers = new Map();
  let locks = 0, retries = 0, destroyed = false;
  window.isDestroyed = () => destroyed;
  powerLock.watch({
    powerMonitor, window, remote: {watch() {}, refresh: () => { retries++; }}, lockNow: reason => { assert.equal(reason, 'idle'); locks++; },
    setTimer: (fn, delay) => { timers.set(delay, fn); return {unref() {}}; },
  });
  for (const event of ['lock-screen', 'suspend']) powerMonitor.emit(event);
  for (const event of ['resume', 'unlock-screen']) powerMonitor.emit(event);
  for (const event of ['show', 'focus']) window.emit(event);
  assert.equal(locks, 2);
  assert.equal(retries, 4);
  destroyed = true;
  window.emit('focus');
  assert.equal(retries, 4);
});

test('the idle timer locks while hosting without entering the books, and survives exceptions', async () => {
  const powerMonitor = new EventEmitter(), window = new EventEmitter(), timers = new Map();
  let idle = 899, hosting = true, busy = true, locks = 0, stateReads = 0;
  powerMonitor.getSystemIdleTime = () => idle;
  window.isDestroyed = () => false;
  powerLock.watch({
    powerMonitor, window, remote: {watch() {}, active: () => false}, busy: () => busy, idleSeconds: 900,
    native: {hostingActive: () => hosting, appState: () => { stateReads++; throw new Error('BUSY'); }},
    lockNow: () => { locks++; return Promise.reject(new Error('background navigation failed')); },
    setTimer: (fn, delay) => { timers.set(delay, fn); return {unref() {}}; },
  });
  const tick = timers.get(30_000);
  tick();
  assert.equal(locks, 0);
  idle = 900;
  tick();
  assert.equal(locks, 1);
  assert.equal(stateReads, 0);
  hosting = false;
  tick();
  assert.equal(locks, 2, 'a busy local worker must defer the lock');
  assert.equal(stateReads, 0);
  busy = false;
  assert.doesNotThrow(tick);
  assert.equal(stateReads, 1);
  hosting = true;
  tick();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(locks, 3);
});

// S6: the host never chooses where an export is saved.
test('an export suggests only a safe file name and must be saved as .csv', async () => {
  for (const [given, want] of [
    ['/home/dana/.config/autostart/update.desktop', 'update.desktop.csv'],
    ['..\\..\\Startup\\run.bat', 'run.bat.csv'],
    ['report.CSV', 'report.csv'],
    ['CON.report.csv', 'export.csv'],
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
  client.markerProbe = true;
  const page = await markerOf(m);
  await assert.rejects(m.run({type: 'ExportHistory', company_id: 'x'}, page), /ending in \.csv/);
  assert.deepEqual(written, [], 'nothing written outside a .csv name');
  choice = '/home/dana/Documents/history.csv';
  assert.deepEqual(await m.run({type: 'ExportHistory', company_id: 'x'}, page), {outcome: 'done', data: {saved: true}});
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

test('Connect with a revoked saved pairing asks for a new code', async () => {
  const {m} = mode([], {connections: [networkError('not_paired')]});
  await m.connectSaved();
  assert.match((await m.render(remoteMode.CONNECT)).html, /did not accept this computer.*new pairing code/);
});

test('only a live session pings every second and background reconnects slow to one per minute', async () => {
  let timer = null;
  const delays = [];
  const schedule = {
    setTimer(fn, delay) { assert.equal(timer, null); timer = {fn, delay}; return timer; },
    cancelTimer(handle) { if (handle === timer) timer = null; },
  };
  const tick = async () => {
    assert.ok(timer);
    const {fn, delay} = timer;
    delays.push(delay);
    timer = null;
    fn();
    await new Promise(resolve => setImmediate(resolve));
  };
  const {m, client} = mode([{type: 'Pong'}, refused('host_locked'), {type: 'Pong'}], {
    connections: [hostSession(true), ...Array.from({length: 6}, () => networkError('unreachable')), hostSession(false)],
    ...schedule,
  });
  m.watch();
  assert.equal(timer, null);
  await m.connectSaved();
  await tick();
  await tick();
  await m.render(remoteMode.LOCKED);
  assert.equal(client.dials.length, 1, 'displaying the notice does not start another connection');
  for (let n = 0; n < 6; n++) {
    await tick();
    assert.equal(client.dials.length, n + 2, 'one helper per scheduled attempt');
  }
  assert.deepEqual(delays, [1000, 1000, 2000, 5000, 15000, 30000, 60000, 60000]);
  assert.equal(timer.delay, 60000);
  await m.render(remoteMode.LOCKED + '?retry=1');
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(m.currentPage(), SIGN_IN);
  await tick();
  assert.equal(delays.at(-1), 1000);
  await m.disconnect();
  assert.equal(timer, null);
});

test('an idle client shows the stop or close notice across retries and reconnects to sign-in', async () => {
  for (const reason of ['The host stopped sharing these books.', 'The host closed these books.']) {
    const {m, shown} = mode([{type: 'Refused', code: 'host_locked', message: reason}], {
      connections: [hostSession(true), networkError('unreachable'), hostSession(false)],
    });
    await m.connectSaved();
    await m.refresh();
    assert.equal(shown.at(-1), remoteMode.LOCKED);
    assert.ok((await m.render(remoteMode.LOCKED)).html.includes(reason));
    await m.refresh();
    await m.refresh();
    assert.equal(shown.at(-1), SIGN_IN);
  }
});

test('a host print view carries the current session and its button opens the output chooser', async () => {
  const target = '/c/book/invoices/invoice/print/';
  const page = {type:'Page', html:'<button id="print-now">',status:200};
  const chosen = [];
  const {m, marker} = mode([page, page, outcome({outcome:'done', data:{desktop_print:true, target}})], {
    choosePrint: async path=>chosen.push(path),
  });
  await m.connectSaved();
  const current = await marker();
  assert.equal((await m.render(target)).redirect, target+'?__fb_remote_page='+current);
  assert.match((await m.render(target,current)).html, /print-now/);
  await m.run(form(target,[['action','desktop_print']]), current);
  assert.deepEqual(chosen,[target]);
});

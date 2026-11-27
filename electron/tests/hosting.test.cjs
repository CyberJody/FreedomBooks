'use strict';
// "Host these books": the Hosting screen, the addresses it offers, and the network watch (run with
// plain Node: npm test). fb-node is a fake here; its own tests cover the Rust side. Synthetic values.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const hosting = require('../main/hosting.cjs');

const IFACES = {
  lo: [{address: '127.0.0.1', family: 'IPv4', internal: true, netmask: '255.0.0.0'}, {address: '::1', family: 'IPv6', internal: true}],
  eth0: [{address: '192.168.1.20', family: 'IPv4', internal: false, netmask: '255.255.255.0'}, {address: '2001:db8::5', family: 'IPv6', internal: false}, {address: 'fe80::1', family: 'IPv6', internal: false}],
  tailscale0: [{address: '100.101.102.103', family: 'IPv4', internal: false, netmask: '255.255.255.255'}, {address: 'fd7a:115c:a1e0::1', family: 'IPv6', internal: false}],
  wwan0: [{address: '100.70.1.2', family: 'IPv4', internal: false, netmask: '255.192.0.0'}, {address: '203.0.113.9', family: 'IPv4', internal: false}],
};

test('only this computer’s private addresses are offered; never all interfaces or a public one', () => {
  const offered = hosting.addresses(IFACES).map(a => a.address);
  assert.deepEqual(offered.sort(), ['100.101.102.103', '100.70.1.2', '192.168.1.20', 'fd7a:115c:a1e0::1'].sort());
  for (const never of ['0.0.0.0', '::', '203.0.113.9', '2001:db8::5', '127.0.0.1', 'fe80::1']) assert.ok(!offered.includes(never), never);
  assert.ok(hosting.addresses(IFACES, {loopback: true}).some(a => a.address === '127.0.0.1'), 'loopback only in development runs');
  const cgnat = hosting.addresses(IFACES).find(a => a.address === '100.70.1.2');
  assert.match(cgnat.warn, /no Tailscale interface owns it/, 'a 100.64/10 address without a Tailscale interface is flagged');
  assert.equal(hosting.addresses(IFACES).find(a => a.address === '100.101.102.103').warn, '');
  assert.equal(hosting.bindText('fd7a:115c:a1e0::1', 'IPv6', 47100), '[fd7a:115c:a1e0::1]:47100');
  assert.equal(hosting.hostOf('[fd7a:115c:a1e0::1]:47100'), 'fd7a:115c:a1e0::1');
  assert.equal(hosting.hostOf('192.168.1.20:47100'), '192.168.1.20');
});

const RUNNING = {generation: '123', available: true, open: true, admin: true, running: true, addr: '192.168.1.20:47100', bind: '192.168.1.20:47100', fingerprint: 'AB12-CD34', sessions: 2,
  pairing: {code: 'ABCD-EFGH-JKMN-PQRS', seconds: 420}, audit: {lines: 12, error: null},
  devices: [{id: 'a'.repeat(32), name: '<b>Front</b> desk', paired_at: 1, paired_by: 'Owner', revoked: false, recorded: false, connected: true, last_seen: 2},
    {id: 'b'.repeat(32), name: 'Old laptop', paired_at: 1, revoked: true, recorded: false, unsaved: 'The list of paired computers could not be saved (read-only).'}]};

test('the screen shows the address, the code, the paired computers and every outstanding warning, escaped', () => {
  const html = hosting.page(RUNNING);
  assert.match(html, /192\.168\.1\.20:47100/);
  assert.match(html, /ABCD-EFGH-JKMN-PQRS/);
  assert.ok(!html.includes('<b>Front</b>') && html.includes('&lt;b&gt;Front&lt;/b&gt; desk'), 'device names are escaped');
  assert.match(html, /data-gate="hosting-revoke"[^]*value="a{32}"/);
  assert.ok(!/value="b{32}"/.test(html), 'a revoked computer has no Revoke button');
  assert.match(html, /Not saved: The list of paired computers could not be saved/, 'the listener’s "not saved" stays visible');
  assert.match(html, /not saved in the books yet/, 'a revocation missing from the sealed record stays visible');
  assert.match(html, /locking this computer signs out only you/);
  const paused = hosting.page({...RUNNING, audit: {lines: 12, error: 'the audit log could not be written: No space left'}});
  assert.match(paused, /Remote access is paused: the audit log could not be written/);
  assert.match(paused, /data-gate="hosting-restart"/, 'A1: the restart that repairs it is offered');
  const refused = hosting.page({available: true, open: true, admin: true, running: false, problem: {kind: 'devices-rolled-back', message: 'older than the one these books recorded'}, devices: []}, {choices: hosting.addresses(IFACES)});
  assert.match(refused, /Hosting did not start/);
  assert.match(refused, /name="kind" value="devices-rolled-back"/);
  assert.match(refused, /data-gate="hosting-on"/);
  assert.ok(!/0\.0\.0\.0|all interfaces/i.test(refused), 'never offers all interfaces');
  assert.match(hosting.page({available: true, open: true, admin: false}), /Only an Administrator/);
  assert.ok(!hosting.page({available: true, open: true, admin: false}).includes('data-gate'), 'no controls for anyone else');
  assert.match(hosting.page({available: false}), /remote-access helper is missing/);
});

function controller(overrides = {}) {
  const calls = [];
  let interfaces = IFACES;
  let hostingOn = true;
  const statuses = [];
  const native = {
    appState: () => JSON.stringify({open: true, hosting: hostingOn}),
    hostingState: () => JSON.stringify({...RUNNING, running: hostingOn}),
    hostingStatus: () => JSON.stringify({running: hostingOn, bind: hostingOn ? RUNNING.bind : '', connected: 1}),
    hostingOnAsync: (bind, name) => { calls.push(['on', bind, typeof name]); return Promise.resolve('{"ok":true}'); },
    hostingEndAsync: reason => { calls.push(['end', reason]); hostingOn = false; return Promise.resolve('{"ok":true}'); },
    hostingRestartAsync: name => { calls.push(['restart', typeof name]); return Promise.resolve('{"ok":true}'); },
    hostingWanted: () => false,
    ...overrides,
  };
  const c = hosting.create({native, exclusive: work => Promise.resolve().then(work), os: {networkInterfaces: () => interfaces, hostname: () => 'office-pc'}, onStatus: s => statuses.push(s)});
  return {c, calls, statuses, setInterfaces: i => { interfaces = i; }, setOn: on => { hostingOn = on; }};
}

test('turning on accepts only a current address and a real port', async () => {
  const {c, calls} = controller();
  await c.on({address: '8.8.8.8', port: '47100'});
  await c.on({address: '0.0.0.0', port: '47100'});
  await c.on({address: '192.168.1.20', port: '80'});
  assert.deepEqual(calls, [], 'nothing reaches fb-node');
  assert.match(c.render(), /Choose one of this computer’s current addresses/);
  await c.on({address: 'fd7a:115c:a1e0::1', port: '47100'});
  assert.deepEqual(calls, [['on', '[fd7a:115c:a1e0::1]:47100', 'string']]);
});

test('hosting stops and says why when its address leaves the computer or its network changes', async () => {
  const {c, calls, setInterfaces} = controller();
  c.check();
  assert.deepEqual(calls, [], 'the address is here');
  setInterfaces({...IFACES, eth0: [{address: '192.168.1.20', family: 'IPv4', internal: false, netmask: '255.255.0.0'}]});
  c.check();
  await new Promise(r => setImmediate(r));
  assert.equal(calls.length, 1);
  assert.match(calls[0][1], /network of 192\.168\.1\.20 changed, so hosting stopped/);
  const second = controller();
  second.c.check();
  second.setInterfaces({lo: IFACES.lo});
  second.c.check();
  await new Promise(r => setImmediate(r));
  assert.match(second.calls[0][1], /192\.168\.1\.20 is no longer on this computer/);
  assert.match(second.c.render(), /no longer on this computer/, 'the screen says why');
});

test('main and preload: hosting calls come only from the Hosting screen and the host’s lock signs out only them', () => {
  const main = fs.readFileSync(path.join(__dirname, '..', 'main', 'main.cjs'), 'utf8');
  for (const name of ['on', 'off', 'pair', 'cancel', 'revoke', 'repair', 'restart']) {
    assert.match(main, new RegExp(`'hosting\\.${name}': async \\([^)]*\\) => \\{ remoteSender\\(event, \\[hosting\\.PATH\\]\\);`), name);
  }
  assert.match(main, /hostingNow = native\.hostingActive\(\);[^]*if \(hostingNow\) \{\n\s+try \{ native\.signOut\(\); \} catch[^]*loadURL\(`\$\{ORIGIN\}\/desktop\/sign-in\/`\);\n\s+return;\n\s+\}\n\s+if \(busy\)/, 'the host person’s lock signs out only them, clears the window at once and is checked before a busy desktop defers it');
  assert.match(main, /'hosting\.restart': async[^\n]*return hosting\.restart\(\); \}/);
  assert.match(main, /label:'Host these books…'/);
  const preload = fs.readFileSync(path.join(__dirname, '..', 'preload', 'preload.cjs'), 'utf8');
  assert.match(preload, /\['hosting-off', 'hosting-pair', 'hosting-cancel', 'hosting-restart'\]\.includes\(form\.dataset\.gate\)/);
  const guards = require('../main/guards.cjs');
  assert.throws(() => guards.hostingRevoke({device: '../x'}));
  assert.throws(() => guards.hostingRepair({kind: 'rm -rf'}));
  assert.throws(() => guards.hostingOn({address: 'evil.example', port: '1'}));
  assert.deepEqual(guards.hostingOn({address: '192.168.1.20', port: '47100'}), {address: '192.168.1.20', port: '47100'});
});

// Review S2: after a restart hosting resumes with nobody signed in at the host, so the Hosting
// state has no address (it is for an Administrator). The watch reads the host's status instead.
test('hosting that resumed after a restart keeps running with nobody signed in at the host', async () => {
  const {c, calls, statuses, setOn} = controller({
    hostingState: () => JSON.stringify({available: true, open: true, admin: false}),
    hostingWanted: () => true,
    hostingResumeAsync: () => { setOn(true); return Promise.resolve('{"ok":true,"running":true}'); },
  });
  setOn(false);
  await c.resume();
  for (let i = 0; i < 3; i++) { c.check(); await new Promise(r => setImmediate(r)); }
  assert.deepEqual(calls, [], 'never stopped');
  assert.ok(statuses.length >= 4 && statuses.every(s => s.running && s.bind === RUNNING.bind));
  // An unknown address is never taken for one that left this computer.
  const blank = controller({hostingStatus: () => JSON.stringify({running: true, bind: '', connected: 0})});
  blank.c.check();
  await new Promise(r => setImmediate(r));
  assert.deepEqual(blank.calls, []);
});

test('Restart hosting is one native call (it checks the Administrator first); the title says hosting is on', async () => {
  const {c, calls} = controller();
  await c.restart();
  assert.deepEqual(calls, [['restart', 'string']], 'nothing ends hosting before fb-node checked the Administrator');
  assert.equal(hosting.statusLabel({running: true, connected: 1}), 'Hosting on — 1 computer connected');
  assert.equal(hosting.statusLabel({running: true, connected: 0}), 'Hosting on — 0 computers connected');
  assert.equal(hosting.statusLabel({running: false, connected: 0}), '');
  const main = fs.readFileSync(path.join(__dirname, '..', 'main', 'main.cjs'), 'utf8');
  assert.match(main, /return hostingTitle \? `\$\{title\} — \$\{hostingTitle\}` : title;/, 'S5: the window title outside the Hosting screen');
});

test('a hosting administrator sees the local and remote signed-in names escaped on Users', () => {
  const {c} = controller({hostingActive: () => true, hostingState: () => JSON.stringify({
    running: true, admin: true, admin_name: 'Pat', signed_in: [['Casey', '<Office>']],
  })});
  assert.match(c.usersPage('<!-- hosting-presence -->'), /Pat · this computer.*Casey · &lt;Office&gt;/);
});

test('Hosting shows live presence, retained sessions, local time zone and the current company', () => {
  const html = hosting.page({...RUNNING, company:'a'.repeat(36), sessions:1, audit:{lines:1}});
  assert.match(html, /1 computer connected now\. 1 remote session, including/);
  assert.match(html, /1 entry, checked/);
  assert.match(html, /<td>Now<\/td>/);
  assert.match(html, /href="\/c\/a{36}\/home\/"/);
  assert.doesNotMatch(html, /docs\/hosting\.md/);
});
test('Stop with a connected computer and Revoke require confirmation', async () => {
  for (const answer of [false,true]) {
    let stops=0, revokes=0, held=false;
    const prompts=[];
    const c=hosting.create({native:{
      hostingStatus:()=>JSON.stringify({connected:1}), hostingState:()=>{assert.equal(held,false); return JSON.stringify(RUNNING);},
      hostingOffAsync:async()=>{ stops++; }, hostingRevokeAsync:async(device,generation)=>{assert.equal(device,RUNNING.devices[0].id); assert.equal(generation,RUNNING.generation); revokes++; return '{"revoked":true}';},
    }, confirm:async message=>{prompts.push(message); return answer;}, exclusive:fn=>fn(), os:{networkInterfaces:()=>IFACES}});
    c.render();
    held=true;
    await c.off();
    await c.revoke({device:'a'.repeat(32)});
    assert.deepEqual(prompts, ['Stop hosting?', 'Revoke <b>Front</b> desk?']);
    assert.equal(stops,Number(answer));
    assert.equal(revokes,Number(answer));
  }
});

test('Hosting refreshes when the connection list changes and stays still on an unchanged tick', () => {
  let s = {...RUNNING};
  let held = false;
  const {c} = controller({hostingState: skipIfBusy => {
    if (held) { assert.equal(skipIfBusy, true); throw new Error('BUSY'); }
    return JSON.stringify(s);
  }});
  c.render();
  assert.equal(c.poll(), null);
  s = {...s, devices: []};
  held = true;
  assert.equal(c.poll(), null);
  held = false;
  assert.match(c.poll(), /No computer is paired yet/);
  assert.equal(c.poll(), null);
});

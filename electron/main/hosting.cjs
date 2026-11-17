'use strict';
// "Host these books" (docs/hosting.md): Settings › Host these books… on the computer that has the
// books. Off by default. An Administrator signed in here turns it on for one private address of
// this computer, pairs other computers with one-time codes, and revokes them. Everything that
// matters is decided in Rust (fb-node's hosting module, which checks the Administrator, the address
// scope and the sealed host record, and runs the fb-remote listener); this module renders the
// screen from fb-node's hosting state, offers only addresses this computer has right now, and stops
// hosting when the chosen address leaves the computer or its network changes.
const PATH = '/hosting/';
const STYLE = '/static/remote.css';
const DEFAULT_PORT = 47100;

const escape = s => String(s ?? '').replace(/[&<>"']/g, c => ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'}[c]));

function v4(address) {
  const o = address.split('.').map(Number);
  if (o.length !== 4 || o.some(n => !Number.isInteger(n) || n < 0 || n > 255)) return null;
  return o;
}

// The same private ranges fb-remote-proto's scope check allows (it checks again): loopback,
// RFC 1918, link-local, Tailscale/CGNAT 100.64.0.0/10, IPv6 unique-local. IPv6 link-local needs a
// zone and is not offered.
function classify(address, family) {
  if (family === 'IPv4' || family === 4) {
    const o = v4(address);
    if (!o) return null;
    if (o[0] === 127) return 'loopback';
    if (o[0] === 10 || (o[0] === 172 && o[1] >= 16 && o[1] <= 31) || (o[0] === 192 && o[1] === 168) || (o[0] === 169 && o[1] === 254)) return 'lan';
    if (o[0] === 100 && o[1] >= 64 && o[1] <= 127) return 'tailscale';
    return null;
  }
  const a = String(address).toLowerCase();
  if (a === '::1') return 'loopback';
  if (/^fd7a:115c:a1e0:/.test(a)) return 'tailscale';
  if (/^f[cd][0-9a-f]{2}:/.test(a)) return 'lan';
  return null;
}

const TAILSCALE_NAME = /tailscale|^utun|^ts\d/i;

// This computer's current private addresses, one entry per address. `loopback` only in
// development and smoke runs (it reaches no other computer).
function addresses(interfaces, {loopback = false} = {}) {
  const out = [];
  for (const [name, list] of Object.entries(interfaces || {})) {
    for (const entry of list || []) {
      const kind = classify(entry.address, entry.family);
      if (!kind || (kind === 'loopback' && !loopback)) continue;
      if (entry.internal && kind !== 'loopback') continue;
      const warn = kind === 'tailscale' && entry.family !== 'IPv6' && entry.family !== 6 && !TAILSCALE_NAME.test(name)
        ? 'This address is in the range Tailscale uses, but no Tailscale interface owns it (it may be your internet provider’s shared address). Use it only if you know it is your tailnet.' : '';
      out.push({address: entry.address, family: entry.family === 6 ? 'IPv6' : entry.family, interface: name, netmask: entry.netmask || '', kind, warn});
    }
  }
  return out;
}

function bindText(address, family, port) {
  return family === 'IPv6' ? `[${address}]:${port}` : `${address}:${port}`;
}

function hostOf(bind) {
  const m = /^\[(.*)\]:\d+$/.exec(bind || '') || /^([^:]+):\d+$/.exec(bind || '');
  return m ? m[1] : '';
}

function when(seconds) {
  if (!seconds) return 'never';
  const d = new Date(seconds * 1000);
  return d.toLocaleString(undefined, {dateStyle: 'medium', timeStyle: 'short'});
}

function shell(inner) {
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Host these books — FreedomBooks</title><link rel="stylesheet" href="${STYLE}"></head><body class="fb-remote-page fb-hosting-page"><main>${inner}</main></body></html>`;
}

const REPAIRS = {
  'audit-lost-key': 'Set the old audit log aside and start a new one',
  'audit-rolled-back': 'Set the old audit log aside and start a new one',
  'audit-damaged': 'Set the old audit log aside and start a new one',
  'devices-rolled-back': 'Set the list of paired computers aside (every computer pairs again)',
  'devices-damaged': 'Set the list of paired computers aside (every computer pairs again)',
};

function devicesTable(s) {
  const rows = (s.devices || []).map(d => {
    const status = d.revoked ? 'Revoked' : d.connected ? 'Connected now' : 'Paired';
    const warnings = [];
    if (d.unsaved) warnings.push(d.unsaved);
    if (d.revoked && d.recorded === false) warnings.push('This revocation is not saved in the books yet. It is in force now; keep this screen open until this warning goes away.');
    const revoke = !d.revoked ? `<form method="post" action="${PATH}" data-gate="hosting-revoke"><input type="hidden" name="device" value="${escape(d.id)}"><button type="submit" class="secondary">Revoke</button></form>` : '';
    return `<tr data-device="${escape(d.id)}"><td>${escape(d.name)}</td><td>${escape(when(d.paired_at))}${d.paired_by ? ` by ${escape(d.paired_by)}` : ''}</td><td>${escape(d.revoked ? '—' : when(d.last_seen))}</td><td>${escape(status)}${warnings.map(w => `<div class="fb-remote-warning" role="alert">Not saved: ${escape(w)}</div>`).join('')}</td><td>${revoke}</td></tr>`;
  }).join('');
  if (!rows) return '<p>No computer is paired yet.</p>';
  return `<table class="fb-hosting-devices"><thead><tr><th>Computer</th><th>Paired</th><th>Last seen</th><th>Status</th><th></th></tr></thead><tbody>${rows}</tbody></table>`;
}

function page(s, {choices = [], errors = [], notices = [], port = DEFAULT_PORT} = {}) {
  const list = errors.length ? `<div class="fb-remote-errors" role="alert">${errors.map(e => `<p>${escape(e)}</p>`).join('')}</div>` : '';
  const told = notices.length ? `<div class="fb-remote-warning" role="status">${notices.map(e => `<p>${escape(e)}</p>`).join('')}</div>` : '';
  const head = `<h1>Host these books</h1>${list}${told}`;
  if (!s.available) return shell(`${head}<p>This copy of FreedomBooks cannot host: its remote-access helper is missing. Reinstall FreedomBooks.</p><p><a href="/">Back to the books</a></p>`);
  if (!s.open) return shell(`${head}<p>Unlock the books first.</p><p><a href="/">Back</a></p>`);
  if (!s.admin) return shell(`${head}<p>Only an Administrator signed in on this computer can manage hosting.</p><p><a href="/">Back to the books</a></p>`);
  if (s.copy) return shell(`${head}<p>A restored copy cannot be hosted. Host the books in use, or choose “Use this copy as my books” first.</p><p><a href="/">Back to the books</a></p>`);
  const problem = s.problem ? `<section class="fb-remote-errors" role="alert"><h2>Hosting did not start</h2><p>${escape(s.problem.message)}</p>
<form method="post" action="${PATH}" data-gate="hosting-repair"><input type="hidden" name="kind" value="${escape(s.problem.kind)}"><button type="submit">${escape(REPAIRS[s.problem.kind] || 'Repair')}</button></form>
<p>The repair is recorded in the new remote-access audit log with your name. Nothing in the books themselves changes.</p></section>` : '';
  const audit = s.audit || {};
  const paused = audit.error || audit.record_error;
  const health = s.running ? `<section><h2>Audit log</h2>${paused
    ? `<div class="fb-remote-errors" role="alert"><p>Remote access is paused: ${escape(paused)}</p><p>Restarting hosting checks and repairs the log, and records the repair.</p><form method="post" action="${PATH}" data-gate="hosting-restart"><button type="submit">Restart hosting</button></form></div>`
    : `<p>${escape(audit.lines)} entries, checked when hosting started and kept in step with the books. Each entry is chained and sealed, so editing, removing or putting back an older copy is detected.</p>`}</section>` : '';
  let main;
  if (s.running) {
    const pairing = s.pairing
      ? `<p class="fb-hosting-code">Pairing code: <strong><code id="fb-hosting-code">${escape(s.pairing.code)}</code></strong></p><p>On the other computer choose <em>File › Connect to a FreedomBooks host…</em> and enter the address above, this code and a name for that computer. The code works once, for about ${escape(Math.max(1, Math.ceil(s.pairing.seconds / 60)))} more minute(s).</p>
<form method="post" action="${PATH}" data-gate="hosting-cancel"><button type="submit" class="secondary">Cancel this code</button></form>`
      : `<form method="post" action="${PATH}" data-gate="hosting-pair"><button type="submit">Pair a computer…</button></form>`;
    main = `<section><h2>Hosting is on</h2><p>Address to give the other computers: <strong><code id="fb-hosting-addr">${escape(s.addr)}</code></strong></p>
<p>This computer’s fingerprint: <code>${escape(s.fingerprint)}</code> (the other computer shows it after pairing; they should match).</p>
<p>${escape(s.sessions)} connected session(s). While hosting, locking this computer signs out only you; the books stay open here for the people connected. Closing the books or quitting stops hosting and tells them.</p>
<form method="post" action="${PATH}" data-gate="hosting-off"><button type="submit" class="secondary">Stop hosting</button></form></section>
<section><h2>Pair a computer</h2>${pairing}</section>`;
  } else {
    const stopped = s.stopped && s.hosted_before ? `<p class="fb-remote-warning" role="status">${escape(s.stopped)}</p>` : '';
    const options = choices.map((c, i) => `<label class="fb-hosting-choice"><input type="radio" name="address" value="${escape(c.address)}" ${(s.bind && hostOf(s.bind) === c.address) || (!s.bind && i === 0) ? 'checked' : ''}> <code>${escape(c.address)}</code> (${escape(c.interface)}${c.kind === 'tailscale' ? ', Tailscale' : c.kind === 'loopback' ? ', this computer only' : ', local network'})${c.warn ? `<span class="fb-remote-warning">${escape(c.warn)}</span>` : ''}</label>`).join('');
    main = `<section><h2>Hosting is off</h2>${stopped}<p>Other computers on your own network or tailnet can use these books through this computer while it is on and the books are open here. Each person signs in with their own user and password. Nothing is reachable from the internet.</p>
${choices.length ? `<form method="post" action="${PATH}" data-gate="hosting-on"><fieldset><legend>Listen on</legend>${options}</fieldset>
<label>Port <input name="port" inputmode="numeric" maxlength="5" value="${escape(s.bind ? s.bind.split(':').pop() : port)}"></label>
<button type="submit">Turn on hosting</button></form>` : '<p class="fb-remote-warning">This computer has no private network address right now (a home or office network, or Tailscale). Connect to one, then open this screen again.</p>'}</section>`;
  }
  return shell(`${head}${problem}${main}${health}<section><h2>Paired computers</h2>${devicesTable(s)}</section>
<p class="fb-hosting-note">Hosting works only inside your own network or tailnet; there is no port forwarding or internet exposure. See the Hosting guide (docs/hosting.md) for limits.</p>
<p><a href="/">Back to the books</a></p>`);
}

// What the window title says while hosting (S5): visible to whoever is at the host, signed in or not.
function statusLabel(s) {
  if (!s || !s.running) return '';
  const n = Number(s.connected) || 0;
  return `Hosting on — ${n} ${n === 1 ? 'computer' : 'computers'} connected`;
}

// The controller main.cjs uses. `native` is fb-node; `exclusive` runs one long task at a time.
// `onStatus` hears {running, bind, connected} after every change and every watch tick.
function create({native, exclusive, os, loopback = false, onNotice = () => {}, onStatus = () => {}, note = () => {}}) {
  let flashed = {errors: [], notices: []};
  let watching = null; // {address, interface, netmask}
  let timer = null;

  const current = () => addresses(os.networkInterfaces(), {loopback});
  const state = () => JSON.parse(native.hostingState());
  // The listening address and connected computers, with nobody signed in (fb-node hosting_status).
  const status = () => JSON.parse(native.hostingStatus());
  function publish() {
    let s;
    try { s = status(); } catch (e) { note('hosting-status', e); return null; }
    onStatus(s);
    return s;
  }

  function flash(kind, message) { flashed[kind].push(String(message)); }
  function result(error) {
    if (error) flash('errors', error.message || error);
    return {outcome: 'redirect', location: PATH};
  }
  async function task(work) {
    try { await exclusive(work); return result(); } catch (e) { return result(e); }
  }
  function remember(bind) {
    const host = hostOf(bind);
    const found = current().find(c => c.address === host);
    watching = found ? {address: found.address, interface: found.interface, netmask: found.netmask} : null;
    return found;
  }
  // Stop hosting when the chosen address leaves this computer or moves to another network. The
  // address comes from the host's status, which needs nobody signed in (hosting that resumed after
  // a restart keeps its watch); an unknown address is never taken for one that left.
  function check() {
    const s = publish();
    if (!s) return;
    if (!s.running) { watching = null; return; }
    const host = hostOf(s.bind);
    if (!host) return;
    if (!watching || watching.address !== host) {
      if (!remember(s.bind)) watching = {address: host, interface: null};
    }
    const now = current().find(c => c.address === watching.address);
    const changed = !now ? `The address ${watching.address} is no longer on this computer` : (watching.interface && (now.interface !== watching.interface || now.netmask !== watching.netmask)) ? `The network of ${watching.address} changed` : '';
    if (!changed) return;
    const reason = `${changed}, so hosting stopped. Connected computers were told. Choose an address on the Hosting screen to host again.`;
    watching = null;
    // Promise-wrapped: a busy desktop (a backup) makes exclusive throw; the next tick tries again.
    Promise.resolve().then(() => exclusive(() => native.hostingEndAsync(reason))).then(() => { flash('notices', reason); publish(); onNotice(reason); }, e => note('hosting-network', e));
  }

  return {
    PATH,
    handles: pathname => pathname === PATH,
    render() {
      let s;
      try { s = state(); } catch (e) { s = {available: true, open: true, admin: false}; flash('errors', e.message || e); }
      const shown = flashed; flashed = {errors: [], notices: []};
      return page(s, {choices: current(), ...shown});
    },
    async on({address, port}) {
      const choice = current().find(c => c.address === address);
      if (!choice) return result(new Error('Choose one of this computer’s current addresses.'));
      const n = Number(port);
      if (!/^\d{1,5}$/.test(port) || n < 1024 || n > 65535) return result(new Error('Choose a port from 1024 to 65535, for example 47100.'));
      const bind = bindText(choice.address, choice.family, n);
      const r = await task(() => native.hostingOnAsync(bind, os.hostname()));
      remember(bind);
      publish();
      return r;
    },
    off: async () => { const r = await task(() => native.hostingOffAsync()); publish(); return r; },
    pair: () => task(() => native.hostingPairAsync()),
    cancel: () => task(() => native.hostingCancelPairAsync()),
    async revoke({device}) {
      try {
        const r = JSON.parse(await exclusive(() => native.hostingRevokeAsync(device)));
        if (r.unsaved) flash('notices', `Revoked, but not saved yet: ${r.unsaved}`);
        if (r.record_unsaved) flash('notices', r.record_unsaved);
        if (r.already) flash('notices', 'That computer was already revoked.');
        return result();
      } catch (e) { return result(e); }
    },
    async repair({kind}) {
      try {
        const r = JSON.parse(await exclusive(() => native.hostingRepairAsync(kind, os.hostname())));
        if (r.aside) flash('notices', `The old file was set aside (kept, not deleted) as ${r.aside}. The repair is recorded in the new audit log with your name.`);
      } catch (e) { publish(); return result(e); }
      const s = publish();
      if (s && s.running) remember(s.bind);
      return result();
    },
    // One native call: it checks the Administrator before anything stops (review N3).
    async restart() {
      const r = await task(() => native.hostingRestartAsync(os.hostname()));
      const s = publish();
      if (s && s.running) remember(s.bind);
      return r;
    },
    // After the books open: start again if an Administrator left hosting on.
    async resume() {
      try {
        if (!native.hostingWanted()) return;
        const r = JSON.parse(await exclusive(() => native.hostingResumeAsync(os.hostname())));
        const s = publish();
        if (r.running && s && s.running) remember(s.bind);
      } catch (e) {
        flash('errors', `Hosting did not start again: ${e.message || e}`);
        note('hosting-resume', e);
      }
    },
    watch(interval = 5000) {
      if (timer) return;
      timer = setInterval(check, interval);
      timer.unref?.();
    },
    check,
    publish,
  };
}

module.exports = {create, page, addresses, classify, bindText, hostOf, statusLabel, PATH};

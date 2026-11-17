'use strict';
// Client mode: this desktop shows a FreedomBooks host's screens instead of its own books. Pages
// (fb://app/ GETs) are rendered by the host; form posts go to the host as typed commands with a
// request id. The renderer is unchanged and stays isolated: it still talks only to this process,
// and this process talks to the host only through the `fb-remote client` child.
//
// Not available in client mode (the host refuses them, and this module never offers them): book
// unlock/creation/restore, protection, recovery key, backups, attachments (file upload is not built
// yet), AI connections, bank/Gmail/Stripe connection sign-in. Printing prints the host's print view
// on this computer's printer.
const { randomUUID } = require('node:crypto');

const PREFIX = '/remote/';
const CONNECT = '/remote/connect/';
const LOCKED = '/remote/locked/';
const DISCONNECT = '/remote/disconnect/';
// How long a lock or a disconnect waits for the host to confirm the sign-out before dropping the
// session on this side anyway (the connection helper is stopped, which forgets the session).
const SIGN_OUT_WAIT = 5000;

function within(promise, ms) {
  let timer;
  return Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(Object.assign(new Error('timeout'), {code: 'timeout'})), ms); timer.unref?.(); })]).finally(() => clearTimeout(timer));
}

// The Save dialog's suggested name for a host's export: the host chooses the contents, never the
// folder. Only the last path part, letters, digits and a few separators, at most 100 characters,
// always .csv.
function exportName(name) {
  let base = String(name || '').split(/[\\/]/).pop().normalize('NFC');
  base = base.replace(/\.csv$/i, '').replace(/[^\p{L}\p{N} ._()-]/gu, '_').replace(/^[\s._-]+/, '').replace(/[\s.]+$/, '').slice(0, 100).replace(/[\s.]+$/, '');
  return `${base || 'export'}.csv`;
}
const REMOTE_STYLE = '/static/remote.css';

const escape = s => String(s).replace(/[&<>"']/g, c => ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'}[c]));

function localPath(location) {
  return typeof location === 'string' && location.startsWith('/') && !location.startsWith('//') && !location.includes('\\') && location.length <= 4096;
}

// The connected-host bar, added to every host page: who you are connected to, and the link state.
function decorate(html, host, state) {
  const bar = `<div id="fb-remote-bar" class="fb-remote-bar" role="status" data-state="${escape(state)}">Connected to <strong>${escape(host)}</strong><span id="fb-remote-state">${state === 'connected' ? '' : ' · reconnecting…'}</span></div>`;
  let out = html.includes('</head>') ? html.replace('</head>', `<link rel="stylesheet" href="${REMOTE_STYLE}"></head>`) : html;
  const body = /<body[^>]*>/i.exec(out);
  out = body ? out.slice(0, body.index + body[0].length) + bar + out.slice(body.index + body[0].length) : bar + out;
  return out;
}

function shell(title, inner) {
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>${escape(title)} — FreedomBooks</title><link rel="stylesheet" href="${REMOTE_STYLE}"></head><body class="fb-remote-page"><main>${inner}</main></body></html>`;
}

function connectPage({saved, errors = [], usable = true}) {
  const list = errors.length ? `<div class="fb-remote-errors" role="alert">${errors.map(e => `<p>${escape(e)}</p>`).join('')}</div>` : '';
  const savedPart = saved ? `<section><h2>This computer is paired with ${escape(saved.host_name)}</h2>
<p>Host address <code>${escape(saved.addr)}</code>, fingerprint <code>${escape(saved.fingerprint)}</code>.</p>
<form method="post" action="${CONNECT}" data-gate="remote-connect"><button type="submit">Connect</button></form>
<form method="post" action="${CONNECT}" data-gate="remote-forget"><button type="submit" class="secondary">Forget this host</button></form></section>` : '';
  const keyring = usable ? '' : '<p class="fb-remote-warning">This computer’s keyring is not available, so a pairing cannot be kept safely. Unlock or set up the keyring first.</p>';
  return shell('Connect to a FreedomBooks host', `<h1>Connect to a FreedomBooks host</h1>${list}${savedPart}
<section><h2>${saved ? 'Pair with a different host' : 'Pair this computer'}</h2>
<p>On the host computer, an administrator chooses <em>Pair a computer</em> and reads you the address and the one-time code. The code works once, for ten minutes. Connections stay inside your own network or tailnet.</p>${keyring}
<form method="post" action="${CONNECT}" data-gate="remote-pair">
<label>Host address <input name="addr" required maxlength="300" placeholder="192.168.1.20:47100" autocomplete="off"></label>
<label>Pairing code <input name="code" required maxlength="40" placeholder="ABCD-EFGH-JKMN-PQRS" autocomplete="off"></label>
<label>Name for this computer <input name="device_name" required maxlength="60" autocomplete="off"></label>
<button type="submit">Pair and connect</button></form></section>
<p><a href="/">Back to the books on this computer</a></p>`);
}

// Pages rendered here may sit at a host address; only the connect and disconnect screens (whose
// content is always this computer's own) carry the forms that pair, forget or disconnect.
function noticePage(title, message, {retry, home = false, disconnect = true} = {}) {
  return shell(title, `<h1>${escape(title)}</h1><p>${escape(message)}</p>
${retry ? `<p><a href="${escape(retry)}">Try again</a></p>` : ''}
${home ? '<p><a href="/">Go to the books’ home page</a></p>' : ''}
${disconnect ? `<p><a href="${DISCONNECT}">Disconnect from the host…</a></p>` : ''}`);
}

function disconnectPage(host) {
  return shell('Disconnect from the host', `<h1>Disconnect from ${escape(host || 'the host')}</h1><p>You will be signed out on the host. The host’s books stay open there.</p>
<form method="post" action="${DISCONNECT}" data-gate="remote-disconnect"><button type="submit">Disconnect</button></form>
<p><a href="/">Stay connected</a></p>`);
}

function lockedPage() {
  return shell('Locked', '<h1>Locked</h1><p>Signing you out on the host…</p>');
}

function create({client, store, window, output, dialog, onChange = () => {}}) {
  let connected = null; // {host_name, addr}
  let link = 'connected';
  let pending = null;   // one-shot re-rendered form, as main.cjs does locally
  let last = null;      // the last command sent: {key, id, answered}
  let flashed = [];     // errors for the next connect screen
  let locking = false;  // a lock's sign-out is in flight: show nothing from the host

  function setLink(state) {
    link = state;
    const win = window();
    if (win && !win.isDestroyed()) win.webContents.send('fb:remote-link', state);
    onChange();
  }

  async function connectWith(record) {
    const response = await client.connect(record.addr, record.identity, record.host_pin);
    if (!response || response.type !== 'Session') throw new Error(response && response.message ? response.message : 'The host refused the connection.');
    connected = {host_name: record.host_name || response.host_name, addr: record.addr};
    link = 'connected';
    pending = null; last = null;
    onChange();
    return response.signed_in ? '/' : '/desktop/sign-in/';
  }

  function refusalPage(r, target) {
    if (r.code === 'unauthenticated' || r.code === 'sign_in_required') return {redirect: '/desktop/sign-in/'};
    const titles = {host_only: 'Only on the host computer', throttled: 'Please wait', busy: 'The host is busy', host_locked: 'The host’s books are locked'};
    return {html: noticePage(titles[r.code] || 'The host refused this', r.message, {retry: ['busy', 'host_locked', 'throttled'].includes(r.code) ? target : null, home: r.code === 'host_only', disconnect: true}), status: 200};
  }

  return {
    active: () => !!connected,
    host: () => connected && connected.host_name,
    link: () => link,
    setLink,
    handles: pathname => pathname.startsWith(PREFIX),
    locking: () => locking,
    flash: errors => { flashed = errors.map(String); },

    // A local client-mode page (the connect screen) or a host page.
    async render(target) {
      const url = new URL(target, 'fb://app');
      if (url.pathname === CONNECT) {
        if (url.searchParams.get('auto') === '1' && !connected) {
          const saved = store.load();
          if (saved) {
            try { return {redirect: await connectWith(saved)}; }
            catch (e) { return {html: connectPage({saved: store.summary(), errors: [e.message], usable: store.usable()}), status: 200}; }
          }
        }
        const errors = flashed; flashed = [];
        return {html: connectPage({saved: store.summary(), errors, usable: store.usable()}), status: errors.length ? 400 : 200};
      }
      if (url.pathname === LOCKED || (locking && !url.pathname.startsWith(PREFIX))) return {html: lockedPage(), status: 200};
      if (url.pathname === DISCONNECT) return connected ? {html: disconnectPage(connected.host_name), status: 200} : {redirect: CONNECT};
      if (url.pathname.startsWith(PREFIX)) return {html: noticePage('Not found', 'There is no such screen.'), status: 404};
      if (!connected) return {redirect: CONNECT};
      // Company logos on document pages come from the host's book as data.
      const logo = /^\/c\/([0-9a-f-]{36})\/document-logo\/$/.exec(url.pathname);
      if (logo) {
        try {
          const r = await client.call({type: 'Query', query: JSON.stringify({type: 'CompanyLogo', company_id: logo[1]})});
          const data = r.type === 'Data' ? JSON.parse(r.data) : null;
          if (data && ['image/png', 'image/jpeg'].includes(data.mime) && typeof data.base64 === 'string') return {binary: Buffer.from(data.base64, 'base64'), mime: data.mime};
        } catch { /* no logo */ }
        return {html: '', status: 404};
      }
      if (pending && pending.path === target) { const p = pending; pending = null; return {html: decorate(p.html, connected.host_name, link), status: p.status}; }
      let r;
      try { r = await client.call({type: 'Render', target}); }
      catch (e) {
        if (e.code === 'not_paired' || e.code === 'host_mismatch') { connected = null; onChange(); }
        return {html: noticePage('The host is not reachable', e.message, {retry: target}), status: 503};
      }
      if (r.type === 'Page') {
        if (r.status >= 300 && r.status < 400) {
          if (!localPath(r.html)) return {html: noticePage('The host refused this', 'The host answered with an invalid address.'), status: 502};
          return {redirect: r.html};
        }
        return {html: decorate(r.html, connected.host_name, link), status: r.status, route: r.route};
      }
      if (r.type === 'Refused') return refusalPage(r, target);
      return {html: noticePage('The host refused this', 'The host answered in a way this version does not understand.'), status: 502};
    },

    // A renderer command (already checked by guards.command).
    async run(command) {
      if (!connected) throw new Error('Not connected to a host.');
      if (locking) throw new Error('This computer is locked. Sign in again.');
      if (!['SubmitForm', 'ExportHistory', 'ExportReport'].includes(command.type)) throw new Error('This can only be done on the host computer.');
      // An identical resubmission of a command that got no answer reuses its request id, so the
      // host answers it from the first attempt instead of doing it twice.
      const key = JSON.stringify(command);
      const id = last && last.key === key && !last.answered ? last.id : randomUUID();
      last = {key, id, answered: false};
      let r;
      try { r = await client.call({type: 'Run', request_id: id, command: JSON.stringify(command)}); }
      catch (e) {
        if (e.code === 'lost' || e.code === 'timeout') throw new Error('The connection to the host was lost. Your last action may or may not have reached it. When the connection is back, submit the same form again: it will not be done twice.');
        throw e;
      }
      last.answered = true;
      if (r.type === 'Refused') {
        if (r.code === 'unauthenticated' || r.code === 'sign_in_required') return {outcome: 'redirect', location: '/desktop/sign-in/'};
        throw new Error(r.message);
      }
      if (r.type !== 'Outcome') throw new Error('The host answered in a way this version does not understand.');
      const outcome = JSON.parse(r.outcome);
      if (outcome.outcome === 'redirect') {
        if (!localPath(outcome.location)) throw new Error('The host answered with an invalid address.');
        return outcome;
      }
      if (['invalid', 'page'].includes(outcome.outcome) && command.type === 'SubmitForm') {
        pending = {path: command.path, html: outcome.html, status: outcome.status || 400};
        return {outcome: 'invalid', reload: command.path};
      }
      if (outcome.outcome === 'export') {
        const selected = await dialog.showSaveDialog(window(), {defaultPath: exportName(outcome.filename), filters: [{name: 'CSV', extensions: ['csv']}], properties: ['showOverwriteConfirmation']});
        if (selected.canceled || !selected.filePath) return {outcome: 'done', data: {saved: false}};
        if (!/\.csv$/i.test(selected.filePath)) throw new Error('Choose a file name ending in .csv. Nothing was saved.');
        output.write(selected.filePath, outcome.contents);
        return {outcome: 'done', data: {saved: true}};
      }
      if (outcome.outcome === 'done' && outcome.data && outcome.data.desktop_print === true) {
        const win = window();
        try {
          await new Promise((resolve, reject) => win.webContents.print({printBackground: true, pageSize: 'Letter', silent: false}, (ok, reason) => ok ? resolve() : reject(new Error(reason || 'Printing was cancelled.'))));
        } catch (e) { outcome.data.print_error = String(e.message || e); }
        return outcome;
      }
      if (outcome.outcome === 'external') throw new Error('This can only be done on the host computer.');
      return outcome;
    },

    async pair({addr, code, device_name}) {
      if (!store.usable()) return {errors: ['This computer’s keyring is not available, so a pairing cannot be kept safely. Unlock or set up the keyring, then try again.']};
      let paired;
      try { paired = await client.pair(addr.trim(), code, device_name); }
      catch (e) { return {errors: [e.message]}; }
      const record = {addr: addr.trim(), host_name: paired.host_name, host_pin: paired.host_pin, fingerprint: paired.fingerprint, device_id: paired.device_id, identity: paired.identity};
      try { store.save(record); } catch (e) { return {errors: [e.message]}; }
      try { return {location: await connectWith(record)}; }
      catch (e) { return {errors: [e.message]}; }
    },

    async connectSaved() {
      const saved = store.load();
      if (!saved) return {errors: ['This computer is not paired with a host.']};
      try { return {location: await connectWith(saved)}; }
      catch (e) { return {errors: [e.message]}; }
    },

    // Lock (Ctrl+L, idle, OS lock, suspend): this person's session on the host ends; the host's
    // books stay open. Fails closed: `show` replaces the page at once, and if the host does not
    // confirm the sign-out, the connection helper is stopped (forgetting the session), so the next
    // connection starts a new session that needs a sign-in. Resolves to whether the host confirmed.
    async lock(show = () => {}) {
      if (!connected) return true;
      locking = true;
      pending = null; last = null;
      show(LOCKED);
      let confirmed = false;
      try {
        const r = await within(client.call({type: 'SignOut'}), SIGN_OUT_WAIT);
        confirmed = !!r && r.type === 'SignedOut';
      } catch { /* unreachable: fall through and drop the session here */ }
      locking = false;
      if (!confirmed) {
        client.stop();
        connected = null;
        flashed = ['This computer was locked while the host could not confirm the sign-out, so the connection was closed. Connect again and sign in.'];
        onChange();
        show(CONNECT);
        return false;
      }
      show('/desktop/sign-in/');
      return true;
    },

    // Leave the host: sign out there (briefly waited for), then close the connection.
    async disconnect() {
      const was = connected;
      connected = null; pending = null; last = null; locking = false;
      onChange();
      if (was) { try { await within(client.call({type: 'SignOut'}), SIGN_OUT_WAIT); } catch { /* the host ends idle sessions itself */ } }
      client.stop();
    },

    async forget() {
      await this.disconnect();
      store.forget();
    },

  };
}

module.exports = {create, decorate, connectPage, noticePage, localPath, exportName, CONNECT, DISCONNECT, LOCKED};

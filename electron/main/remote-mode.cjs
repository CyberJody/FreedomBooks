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
const {exportName} = require('./save-file.cjs');

const PREFIX = '/remote/';
const CONNECT = '/remote/connect/';
const CONNECTING = '/remote/connecting/';
const LOCKED = '/remote/locked/';
const DISCONNECT = '/remote/disconnect/';
const SIGN_IN = '/desktop/sign-in/';
const PAGE_KEY = '__fb_remote_page';
const PAIRING_ERRORS = {
  not_paired: 'The host did not accept this computer. Ask the host’s administrator for a new pairing code.',
  host_mismatch: 'The computer at this address is not the host you paired with. Check with your administrator before pairing again.',
};
// How long a lock or a disconnect waits for the host to confirm the sign-out before dropping the
// session on this side anyway (the connection helper is stopped, which forgets the session).
const SIGN_OUT_WAIT = 5000;
// An idle session must hear a stop notice within the host's 1.5-second grace.
// Recovery creates a helper and opens a new connection, so use progressively longer waits.
const PING_INTERVAL = 1000;
const RETRY_DELAYS = [2000, 5000, 15000, 30000, 60000];

function within(promise, ms) {
  let timer;
  return Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(Object.assign(new Error('timeout'), {code: 'timeout'})), ms); timer.unref?.(); })]).finally(() => clearTimeout(timer));
}

const REMOTE_STYLE = '/static/remote.css';

const escape = s => String(s).replace(/[&<>"']/g, c => ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'}[c]));

function localPath(location) {
  return typeof location === 'string' && location.startsWith('/') && !location.startsWith('//') && !location.includes('\\') && location.length <= 4096;
}

function stampTarget(target, marker) {
  const url = new URL(target, 'fb://app');
  if (url.search) url.search += `&${PAGE_KEY}=${marker}`;
  return url.pathname + url.search + url.hash;
}

const guards = require('./guards.cjs');

function csvPage(route, url, status, body) {
  return Boolean(guards.csvExportFilename(route, url, status, body));
}

// The connected-host bar, added to every host page: who you are connected to, and the link state.
function decorate(html, host, state, marker) {
  const bar = `<div id="fb-remote-bar" class="fb-remote-bar" role="status" data-state="${escape(state)}">Connected to <strong>${escape(host)}</strong><span id="fb-remote-state">${state === 'connected' ? '' : ' · reconnecting…'}</span></div>`;
  const stamp = `<meta name="fb-remote-page" content="${escape(marker)}">`;
  let out = /<head\b[^>]*>/i.test(html) ? html.replace(/<head\b[^>]*>/i, head => head + stamp) : stamp + html;
  out = out.replace(/ · Local ledger(?=<\/title>)/i, '').replace(/<\/title>/i, () => ` — connected to ${escape(host)}</title>`);
  out = out.replace('</head>', `<link rel="stylesheet" href="${REMOTE_STYLE}"></head>`);
  out = out.replace(/(\bsrc="\/c\/[0-9a-f-]{36}\/document-logo\/)(")/g, `$1?${PAGE_KEY}=${marker}$2`);
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

function disconnectPage(host, back) {
  return shell('Disconnect from the host', `<h1>Disconnect from ${escape(host || 'the host')}</h1><p>You will be signed out on the host. The host’s books stay open there.</p>
<form method="post" action="${DISCONNECT}" data-gate="remote-disconnect"><button type="submit">Disconnect</button></form>
<p><a href="${back}">Stay connected</a></p>`);
}

function lockedPage() {
  return shell('Locked', '<h1>Locked</h1><p>Signing you out on the host…</p>');
}

function reconnectPage(host, locked, reason) {
  return shell(locked ? 'Locked' : 'Reconnecting', `<h1>${locked ? 'Locked. ' : ''}Reconnecting to ${escape(host)}…</h1>
${reason ? `<p role="status">${escape(reason)}</p>` : ''}
<p><a href="${LOCKED}?retry=1">Try now</a></p>
<p><a href="${DISCONNECT}">Disconnect</a></p>`);
}

function create({client, store, window, output, dialog, choosePrint, onChange = () => {}, show = () => {}, setTimer = setTimeout, cancelTimer = clearTimeout}) {
  let connected = null; // host details, including while waiting to reconnect
  let session = null;   // current access, null if locking or starting a fresh session
  let link = 'connected';
  let pending = null;   // one-shot re-rendered form, as main.cjs does locally
  let last = null;      // the last command sent: {key, id, answered}
  let flashed = [];     // errors for the next connect screen
  let locking = false;
  let locked = false;
  let stopReason = '';
  let refreshing = null;
  let failedPage = null;
  let transition = null;
  let watching = false;
  let timer = null;
  let retryStep = 0;

  function scheduleRefresh() {
    cancelTimer(timer);
    timer = null;
    if (!watching || !connected || transition || locking || refreshing) return;
    const delay = session && link === 'connected' ? PING_INTERVAL : RETRY_DELAYS[retryStep];
    timer = setTimer(() => {
      timer = null;
      const win = window();
      if (win && !win.isDestroyed()) void refresh()?.catch(() => {});
    }, delay);
    timer.unref?.();
  }

  const newSession = signedIn => ({signedIn, marker: randomUUID()});
  const accepts = marker => !!session && !transition && session.marker === marker;

  function setLink(state) {
    link = state;
    if (state === 'connected') retryStep = 0;
    scheduleRefresh();
    const win = window();
    if (win && !win.isDestroyed()) win.webContents.send('fb:remote-link', state);
    onChange();
  }

  function clearRequests() {
    cancelTimer(timer);
    timer = null;
    pending = null;
    last = null;
    failedPage = null;
    refreshing = null;
  }

  async function openSession(record) {
    const response = await client.connect(record.addr, record.identity, record.host_pin);
    if (response?.type !== 'Session') throw new Error(response?.message || 'The host refused the connection.');
    return response;
  }

  async function changeConnection(recordForAttempt, {cover = true, recovering = false} = {}) {
    if (transition || locking) return {location: currentPage(), navigated: true};
    const attempt = transition = {recovering};
    session = null;
    clearRequests();
    onChange();
    if (cover) show(CONNECTING);
    try {
      const record = await recordForAttempt(() => transition === attempt);
      if (transition !== attempt) return {location: currentPage(), navigated: true};
      const response = await openSession(record);
      if (transition !== attempt) return {location: currentPage(), navigated: true};
      if (recovering && (response.signed_in || response.resumed)) throw new Error('The host did not open a fresh session.');
      connected = {host_name: record.host_name || response.host_name, addr: record.addr, device_id: record.device_id, host_pin: record.host_pin};
      transition = null;
      session = newSession(!!response.signed_in);
      if (!recovering) locked = false;
      stopReason = '';
      setLink('connected');
      const location = currentPage();
      show(location);
      return {location, navigated: true};
    } catch (e) {
      if (transition !== attempt) return {location: currentPage(), navigated: true};
      client.stop();
      transition = null;
      if (recovering && !Object.hasOwn(PAIRING_ERRORS, e.code)) {
        retryStep = Math.min(retryStep + 1, RETRY_DELAYS.length - 1);
        scheduleRefresh();
        onChange();
        return {location: LOCKED, navigated: true};
      }
      connected = null;
      onChange();
      const errors = [Object.hasOwn(PAIRING_ERRORS, e.code) ? PAIRING_ERRORS[e.code] : e.message];
      flashed = errors;
      show(CONNECT);
      return {errors, navigated: true};
    }
  }

  function pairingProblem(message) {
    connected = null;
    session = null;
    locking = false;
    clearRequests();
    client.stop();
    flashed = [message];
    onChange();
    return CONNECT;
  }

  function startRecovery(reason = '') {
    stopReason = reason;
    retryStep = 0;
    session = null;
    clearRequests();
    client.stop();
    setLink('reconnecting');
    return LOCKED;
  }

  function currentPage() {
    if (transition) return transition.recovering ? LOCKED : CONNECTING;
    return !connected ? CONNECT : !session ? LOCKED : session.signedIn ? '/' : SIGN_IN;
  }

  // A fresh helper gets only the sealed pairing. It can never resume the old session.
  async function reconnect(host) {
    let saved;
    try { saved = store.load(); } catch { /* unavailable keyring */ }
    if (!saved) {
      show(pairingProblem('FreedomBooks could not read the saved pairing. Unlock this computer’s keyring or pair with the host again.'));
      return;
    }
    if (saved.addr !== host.addr || saved.device_id !== host.device_id || saved.host_pin !== host.host_pin) {
      show(pairingProblem('The saved pairing changed. Connect to the host you want to use.'));
      return;
    }
    await changeConnection(() => saved, {cover: false, recovering: true});
  }

  // A Ping detects a dropped transport even on an untouched page. The helper first tries to
  // resume the same session, and a Pong on a live connection does not reset the host's idle limit.
  async function probe(current) {
    let r;
    try { r = await client.call({type: 'Ping'}); }
    catch (e) {
      if (session !== current) return;
      if (Object.hasOwn(PAIRING_ERRORS, e.code)) show(pairingProblem(PAIRING_ERRORS[e.code]));
      else if (e.code === 'not_connected') show(startRecovery());
      else {
        retryStep = Math.min(retryStep + 1, RETRY_DELAYS.length - 1);
        setLink('lost');
      }
      return;
    }
    if (session !== current) return;
    if (r?.type === 'Refused' && ['host_locked', 'unauthenticated'].includes(r.code)) show(startRecovery(r.code === 'host_locked' ? r.message : ''));
    if (r?.type === 'Pong') {
      if (link !== 'connected') setLink('connected');
      if (failedPage) {
        const [target, expected] = failedPage;
        failedPage = null;
        show(target, expected);
      }
    }
  }

  function refresh() {
    if (transition || !connected || locking) return refreshing;
    if (!refreshing) {
      const attempt = session ? probe(session) : reconnect(connected);
      refreshing = attempt;
      // Only the current attempt may release the retry slot.
      const done = () => {
        if (refreshing === attempt) {
          refreshing = null;
          scheduleRefresh();
        }
      };
      void attempt.then(done, done);
    }
    return refreshing;
  }

  function refusalLocation(r) {
    if (r.code === 'host_locked') return startRecovery(r.message);
    if (r.code === 'unauthenticated') return startRecovery();
    if (r.code === 'sign_in_required') {
      session.signedIn = false;
      return SIGN_IN;
    }
    return null;
  }

  function refusalPage(r, target) {
    const titles = {host_only: 'Only on the host computer', throttled: 'Please wait', busy: 'The host is busy', host_locked: 'The host’s books are locked'};
    return {html: noticePage(titles[r.code] || 'The host refused this', r.message, {retry: ['busy', 'host_locked', 'throttled'].includes(r.code) ? target : null, home: r.code === 'host_only', disconnect: true}), status: 200};
  }

  return {
    active: () => !!connected || !!transition,
    host: () => connected && connected.host_name,
    link: () => link,
    setLink,
    refresh,
    watch() { watching = true; scheduleRefresh(); },
    handles: pathname => pathname.startsWith(PREFIX),
    flash: errors => { flashed = errors.map(String); },
    accepts,
    printMarker: () => session?.signedIn && !transition ? session.marker : null,
    async printPage(target, marker) {
      if (!accepts(marker) || !session.signedIn) throw new Error('Sign in and open a print view first.');
      const page = await client.call({type: 'Render', target});
      if (!accepts(marker) || !session.signedIn) throw new Error('The active user changed. Open the print view again.');
      if (page.type === 'Refused') {
        const location = refusalLocation(page);
        if (location) show(location);
        throw new Error(page.message);
      }
      return page.type === 'Page' ? page : null;
    },
    currentPage,
    admit(marker) {
      if (accepts(marker) || (marker === null && !connected && !transition)) return null;
      return {outcome: 'redirect', location: currentPage()};
    },

    // A local client-mode page (the connect screen) or a host page.
    async render(target, marker = null) {
      const url = new URL(target, 'fb://app');
      const logo = /^\/c\/([0-9a-f-]{36})\/document-logo\/$/.exec(url.pathname);
      if (logo && !accepts(marker)) return {html: '', status: 404};
      if (transition && !transition.recovering) {
        const cover = transition.leaving ? shell('Disconnecting', '<h1>Disconnecting from the host…</h1>') : shell('Connecting', '<h1>Connecting to a FreedomBooks host…</h1>');
        return url.pathname === CONNECTING ? {html: cover, status: 200} : {redirect: CONNECTING};
      }
      if (marker !== null && !accepts(marker)) return {redirect: currentPage()};
      if (url.pathname === CONNECTING) return {redirect: currentPage()};
      if (url.pathname === CONNECT) {
        if (connected && !session) return {redirect: LOCKED};
        if (url.searchParams.get('auto') === '1' && !connected) {
          const saved = store.load();
          if (saved) {
            void changeConnection(() => saved);
            return {redirect: CONNECTING};
          }
        }
        const errors = flashed; flashed = [];
        return {html: connectPage({saved: store.summary(), errors, usable: store.usable()}), status: errors.length ? 400 : 200};
      }
      if (url.pathname === LOCKED) {
        if (locking) return {html: lockedPage(), status: 200};
        if (!connected || session) return {redirect: currentPage()};
        const html = reconnectPage(connected.host_name, locked, stopReason);
        if (url.searchParams.get('retry') === '1') void refresh();
        return {html, status: 200};
      }
      if (url.pathname === DISCONNECT) return connected ? {html: disconnectPage(connected.host_name, session ? '/' : LOCKED), status: 200} : {redirect: CONNECT};
      if (url.pathname.startsWith(PREFIX)) return {html: noticePage('Not found', 'There is no such screen.'), status: 404};
      if (!session || (url.search && !accepts(marker))) return {redirect: currentPage()};
      const current = session;
      const stampedTarget = stampTarget(target, current.marker);
      if (!current.signedIn && url.pathname !== SIGN_IN) return {redirect: SIGN_IN};
      // Company logos on document pages come from the host's book as data.
      if (logo) {
        try {
          const r = await client.call({type: 'Query', query: JSON.stringify({type: 'CompanyLogo', company_id: logo[1]})});
          const data = r.type === 'Data' ? JSON.parse(r.data) : null;
          if (session === current && current.signedIn && data && ['image/png', 'image/jpeg'].includes(data.mime) && typeof data.base64 === 'string') return {binary: Buffer.from(data.base64, 'base64'), mime: data.mime, marker: current.marker};
        } catch { /* no logo */ }
        return {html: '', status: 404};
      }
      if (pending && pending.path === target) { const p = pending; pending = null; return {html: decorate(p.html, connected.host_name, link, current.marker), status: p.status, marker: current.marker}; }
      let r;
      try { r = await client.call({type: 'Render', target}); }
      catch (e) {
        if (session !== current) return {redirect: currentPage()};
        if (Object.hasOwn(PAIRING_ERRORS, e.code)) return {redirect: pairingProblem(PAIRING_ERRORS[e.code])};
        if (e.code === 'not_connected') return {redirect: startRecovery()};
        failedPage = [stampedTarget, marker ? stampedTarget : target];
        return {html: noticePage('The host is not reachable', 'FreedomBooks will keep trying to reconnect.', {retry: stampedTarget}), status: 503};
      }
      if (session !== current || (!current.signedIn && url.pathname !== SIGN_IN)) return {redirect: currentPage()};
      if (failedPage?.[0] === stampedTarget) failedPage = null;
      if (r.type === 'Page') {
        if (r.status >= 300 && r.status < 400) {
          if (!localPath(r.html)) return {html: noticePage('The host refused this', 'The host answered with an invalid address.'), status: 502};
          if (r.html === SIGN_IN && url.pathname !== SIGN_IN) current.signedIn = false;
          return {redirect: stampTarget(r.html, current.marker), marker: current.marker};
        }
        const csv = csvPage(r.route, url, r.status, r.html);
        // Bind even a queryless print view to the session that supplied its visible contents.
        if (!csv && !marker && r.status === 200 && r.html.includes('id="print-now"')) {
          return {redirect: `${target}${url.search ? '&' : '?'}${PAGE_KEY}=${current.marker}`, marker: current.marker};
        }
        return {html: csv ? r.html : decorate(r.html, connected.host_name, link, current.marker), status: r.status, route: r.route, marker: current.marker};
      }
      if (r.type === 'Refused') {
        const redirect = refusalLocation(r);
        if (redirect && redirect !== target) return {redirect};
        return {...refusalPage(r, stampedTarget), marker: current.marker};
      }
      return {html: noticePage('The host refused this', 'The host answered in a way this version does not understand.'), status: 502};
    },

    // A renderer command (already checked by guards.command).
    async run(command, marker) {
      if (!accepts(marker)) return {outcome: 'redirect', location: currentPage()};
      if (!['SubmitForm', 'ExportHistory', 'ExportReport'].includes(command.type)) throw new Error('This can only be done on the host computer.');
      const current = session;
      const signIn = command.type === 'SubmitForm' && command.path === SIGN_IN;
      if (!current.signedIn && !signIn) return {outcome: 'redirect', location: SIGN_IN};
      if (signIn && current.signingIn) throw new Error('A sign-in is already being checked.');
      const action = signIn && command.fields.find(([key]) => key === 'action')?.[1];
      const requireSession = () => {
        if (session !== current) throw new Error('The session changed. Open the page again.');
      };
      // An identical resubmission of a command that got no answer reuses its request id, so the
      // host answers it from the first attempt instead of doing it twice.
      const key = JSON.stringify(command);
      const id = last && last.key === key && !last.answered ? last.id : randomUUID();
      const sent = last = {key, id, answered: false};
      let r;
      if (signIn) current.signingIn = true;
      try { r = await client.call({type: 'Run', request_id: id, command: JSON.stringify(command)}); }
      catch (e) {
        requireSession();
        if (Object.hasOwn(PAIRING_ERRORS, e.code)) return {outcome: 'redirect', location: pairingProblem(PAIRING_ERRORS[e.code])};
        if (e.code === 'not_connected' || (signIn && ['lost', 'timeout'].includes(e.code))) {
          return {outcome: 'redirect', location: startRecovery()};
        }
        if (e.code === 'lost' || e.code === 'timeout') throw new Error('The connection to the host was lost. Your last action may or may not have reached it. When the connection is back, submit the same form again: it will not be done twice.');
        throw e;
      } finally {
        if (signIn) current.signingIn = false;
      }
      requireSession();
      if (!current.signedIn && !signIn) return {outcome: 'redirect', location: SIGN_IN};
      sent.answered = true;
      if (r.type === 'Refused') {
        const location = refusalLocation(r);
        if (location) return {outcome: 'redirect', location};
        throw new Error(r.message);
      }
      if (r.type !== 'Outcome') throw new Error('The host answered in a way this version does not understand.');
      const outcome = JSON.parse(r.outcome);
      if (outcome.outcome === 'redirect') {
        if (!localPath(outcome.location)) throw new Error('The host answered with an invalid address.');
        if (signIn) {
          current.signedIn = outcome.location !== SIGN_IN;
          if (current.signedIn) locked = false;
          // A password may have been accepted even though the second factor is still pending.
          current.mfaPending = !current.signedIn && action === 'signin';
        } else if (outcome.location === SIGN_IN) {
          current.signedIn = false;
        }
        return {...outcome, location: stampTarget(outcome.location, current.marker)};
      }
      if (['invalid', 'page'].includes(outcome.outcome) && command.type === 'SubmitForm') {
        pending = {path: command.path, html: outcome.html, status: outcome.status || 400};
        return {outcome: 'invalid', reload: stampTarget(command.path, current.marker)};
      }
      if (outcome.outcome === 'export') {
        const selected = await dialog.showSaveDialog(window(), {defaultPath: exportName(outcome.filename), filters: [{name: 'CSV', extensions: ['csv']}], properties: ['showOverwriteConfirmation']});
        requireSession();
        if (!current.signedIn) throw new Error('Sign in again before saving an export.');
        if (selected.canceled || !selected.filePath) return {outcome: 'done', data: {saved: false}};
        if (!/\.csv$/i.test(selected.filePath)) throw new Error('Choose a file name ending in .csv. Nothing was saved.');
        output.write(selected.filePath, outcome.contents);
        return {outcome: 'done', data: {saved: true}};
      }
      if (outcome.outcome === 'done' && outcome.data && outcome.data.desktop_print === true) {
        try {
          await choosePrint(outcome.data.target);
        } catch (e) { outcome.data.print_error = String(e.message || e); }
        requireSession();
        return outcome;
      }
      if (outcome.outcome === 'external') throw new Error('This can only be done on the host computer.');
      return outcome;
    },

    async pair({addr, code, device_name}) {
      if (transition) return {location: currentPage(), navigated: true};
      if (connected && !session) return {location: LOCKED};
      if (!store.usable()) return {errors: ['This computer’s keyring is not available, so a pairing cannot be kept safely. Unlock or set up the keyring, then try again.']};
      return changeConnection(async isCurrent => {
        const paired = await client.pair(addr.trim(), code, device_name);
        const record = {addr: addr.trim(), host_name: paired.host_name, host_pin: paired.host_pin, fingerprint: paired.fingerprint, device_id: paired.device_id, identity: paired.identity};
        if (isCurrent()) store.save(record);
        return record;
      });
    },

    async connectSaved() {
      if (transition) return {location: currentPage(), navigated: true};
      if (connected && !session) return {location: LOCKED};
      const saved = store.load();
      if (!saved) return {errors: ['This computer is not paired with a host.']};
      return changeConnection(() => saved);
    },

    // Cover the old page before waiting for the host, and invalidate any reads or exports
    // already in flight. A lock during a sign-in drops the token instead of racing that sign-in.
    async lock() {
      if (!session || !(session.signedIn || session.mfaPending || session.signingIn)) return true;
      const host = connected;
      const signingIn = session.signingIn;
      session = null;
      locking = true;
      locked = true;
      clearRequests();
      onChange();
      const covered = show(LOCKED);
      let answer;
      if (!signingIn) {
        try { answer = await within(client.call({type: 'SignOut'}), SIGN_OUT_WAIT); }
        catch (e) {
          if (connected === host && Object.hasOwn(PAIRING_ERRORS, e.code)) {
            const location = pairingProblem(PAIRING_ERRORS[e.code]);
            await covered;
            show(location);
            return false;
          }
        }
      }
      if (connected !== host) return false;
      const signedOut = answer?.type === 'SignedOut' || (answer?.type === 'Refused' && answer.code === 'sign_in_required');
      const ended = answer?.type === 'Refused' && answer.code === 'unauthenticated';
      if (!signedOut) startRecovery(answer?.code === 'host_locked' ? answer.message : '');
      await covered;
      if (connected !== host) return signedOut || ended;
      locking = false;
      if (signedOut) {
        session = newSession(false);
        scheduleRefresh();
        show(SIGN_IN);
      } else {
        show(LOCKED);
        void refresh();
      }
      return signedOut || ended;
    },

    // Leave the host. When a lock or recovery is in progress there is no session to sign out.
    async disconnect() {
      const was = session, inFlight = transition;
      const leaving = transition = {leaving: true};
      connected = null;
      session = null;
      locking = false;
      clearRequests();
      onChange();
      if (!inFlight && was && (was.signedIn || was.mfaPending) && !was.signingIn) {
        try { await within(client.call({type: 'SignOut'}), SIGN_OUT_WAIT); } catch { /* the host ends idle sessions */ }
      }
      if (transition === leaving) {
        client.stop();
        transition = null;
        onChange();
        // A window sent to the cover while this waited (File › Connect, a reload) would stay there.
        show(CONNECT, CONNECTING);
      }
    },

    async forget() {
      await this.disconnect();
      store.forget();
    },

  };
}

module.exports = {create, decorate, connectPage, noticePage, localPath, exportName, csvPage, CONNECT, CONNECTING, DISCONNECT, LOCKED, PAGE_KEY};

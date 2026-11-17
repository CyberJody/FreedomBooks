'use strict';
// Remote access, client side (docs/remote-access-design.md). The TLS connection to a host is
// owned by the `fb-remote client` child process (Rust; mutual TLS with the host pinned at pairing).
// This module starts that process, talks to it in length-prefixed JSON frames over its stdio, and
// keeps the pairing record (device identity and host pin) sealed by the OS key store. The
// renderer never sees any of it, and nothing here opens a book.
const fs = require('node:fs');
const path = require('node:path');
const { spawn } = require('node:child_process');

const MAX_FRAME = 8 * 1024 * 1024 + 64 * 1024;
const RECORD = 'remote-host.sealed';
const PAIR_TIMEOUT = 60 * 1000;
const CALL_TIMEOUT = 4 * 60 * 1000;

function encode(value) {
  const body = Buffer.from(JSON.stringify(value), 'utf8');
  if (body.length > MAX_FRAME) throw new Error('Request too large.');
  const head = Buffer.alloc(4);
  head.writeUInt32BE(body.length, 0);
  return Buffer.concat([head, body]);
}

// Incremental frame reader: feed chunks, get whole frames. A declared length over the limit is a
// broken stream (the child is then stopped).
function decoder(onFrame, onBroken) {
  let buffer = Buffer.alloc(0);
  return chunk => {
    buffer = Buffer.concat([buffer, chunk]);
    while (buffer.length >= 4) {
      const len = buffer.readUInt32BE(0);
      if (len > MAX_FRAME) { buffer = Buffer.alloc(0); onBroken(); return; }
      if (buffer.length < 4 + len) return;
      const body = buffer.subarray(4, 4 + len);
      buffer = buffer.subarray(4 + len);
      let value;
      try { value = JSON.parse(body.toString('utf8')); } catch { onBroken(); return; }
      onFrame(value);
    }
  };
}

// The child process. `helper` is a fixed path beside the native module, never searched for.
function child({helper, start = spawn, onLink = () => {}}) {
  let proc = null, next = 1;
  const pending = new Map();
  const fail = message => {
    for (const [, p] of pending) { clearTimeout(p.timer); p.reject(Object.assign(new Error(message), {code: 'lost'})); }
    pending.clear();
  };
  function ensure() {
    if (proc) return proc;
    if (!helper || !fs.existsSync(helper)) throw new Error('This copy of FreedomBooks cannot connect to a host: its connection helper is missing.');
    proc = start(helper, ['client'], {stdio: ['pipe', 'pipe', 'ignore'], env: {}, windowsHide: true});
    const mine = proc;
    mine.stdout.on('data', decoder(message => {
      if (message.kind === 'link') return onLink(message.state);
      const p = pending.get(message.id);
      if (!p) return;
      pending.delete(message.id);
      clearTimeout(p.timer);
      if (message.kind === 'error') p.reject(Object.assign(new Error(message.message), {code: message.code}));
      else p.resolve(message);
    }, () => { try { mine.kill(); } catch { /* already gone */ } }));
    mine.on('exit', () => { if (proc === mine) proc = null; fail('The connection helper stopped.'); onLink('lost'); });
    mine.on('error', () => { if (proc === mine) proc = null; fail('The connection helper could not start.'); });
    mine.stdin.on('error', () => {});
    return mine;
  }
  function send(op, fields, timeout) {
    const p = ensure();
    const id = next++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { pending.delete(id); reject(Object.assign(new Error('The host did not answer in time.'), {code: 'timeout'})); }, timeout);
      pending.set(id, {resolve, reject, timer});
      try { p.stdin.write(encode({op, id, ...fields})); }
      catch (e) { pending.delete(id); clearTimeout(timer); reject(e); }
    });
  }
  return {
    pair: (addr, code, device_name) => send('pair', {addr, code, device_name}, PAIR_TIMEOUT),
    connect: (addr, identity, host_pin) => send('connect', {addr, identity, host_pin}, PAIR_TIMEOUT).then(m => m.response),
    call: request => send('call', {request}, CALL_TIMEOUT).then(m => m.response),
    stop() {
      const p = proc; proc = null;
      fail('Disconnected from the host.');
      if (p) { try { p.stdin.end(); } catch { /* gone */ } setTimeout(() => { try { p.kill(); } catch { /* gone */ } }, 500).unref?.(); }
    },
    running: () => !!proc,
  };
}

// The sealed pairing record: {addr, host_name, host_pin, fingerprint, device_id, identity:{cert,key}}.
// Only with a working OS key store (Linux basic_text is not one): the device key is a secret.
function store({dir, safeStorage, writePrivate}) {
  const file = path.join(dir, RECORD);
  const usable = () => safeStorage.isEncryptionAvailable() && (process.platform !== 'linux' || safeStorage.getSelectedStorageBackend() !== 'basic_text');
  const HEX = /^[0-9a-f]+$/;
  function valid(r) {
    return r && typeof r === 'object' && typeof r.addr === 'string' && r.addr.length <= 300 && typeof r.host_name === 'string' && r.host_name.length <= 100
      && typeof r.host_pin === 'string' && /^[0-9a-f]{64}$/.test(r.host_pin) && typeof r.fingerprint === 'string' && typeof r.device_id === 'string'
      && r.identity && typeof r.identity.cert === 'string' && HEX.test(r.identity.cert) && typeof r.identity.key === 'string' && HEX.test(r.identity.key);
  }
  return {
    usable,
    save(record) {
      if (!usable()) throw new Error('This computer’s keyring is not available, so the pairing cannot be kept safely. Unlock or set up the keyring, then pair again.');
      if (!valid(record)) throw new Error('Invalid pairing.');
      const text = JSON.stringify(record);
      writePrivate(file, safeStorage.encryptString(text));
      if (safeStorage.decryptString(fs.readFileSync(file)) !== text) throw new Error('The keyring did not keep the pairing.');
    },
    load() {
      if (!usable()) return null;
      let raw;
      try {
        const st = fs.lstatSync(file);
        if (!st.isFile() || st.size > 65536) return null;
        raw = fs.readFileSync(file);
      } catch { return null; }
      try { const r = JSON.parse(safeStorage.decryptString(raw)); return valid(r) ? r : null; } catch { return null; }
    },
    // The public part, for screens: never the identity.
    summary() {
      const r = this.load();
      return r ? {addr: r.addr, host_name: r.host_name, fingerprint: r.fingerprint} : null;
    },
    forget() { fs.rmSync(file, {force: true}); },
  };
}

module.exports = {child, store, encode, decoder, MAX_FRAME, RECORD};

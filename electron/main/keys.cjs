// Local OS sealing is independent of the portable book-key record.
'use strict';
const {safeStorage} = require('electron');
const fs = require('node:fs');
const path = require('node:path');
const {randomBytes, randomUUID} = require('node:crypto');

const SEALED_UNREADABLE = 'The key saved on this computer could not be read. Enter your password or recovery key to open your books; FreedomBooks then saves a new key on this computer.';

function userDataDir(app) {
  const override = require('./development.cjs').userData(app);
  if (override) {
    if (!path.isAbsolute(override)) throw new Error('FB_DESKTOP_USER_DATA must be an absolute path');
    return override;
  }
  return require('./identity.cjs').dataDir(app);
}
function readRegular(file, max = 65536) {
  const fd = fs.openSync(file, fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW || 0));
  try {
    const st = fs.fstatSync(fd);
    if (!st.isFile() || st.size > max) throw new Error('Invalid key file.');
    return fs.readFileSync(fd);
  } finally { fs.closeSync(fd); }
}
function writePrivate(file, data) {
  fs.mkdirSync(path.dirname(file), {recursive:true,mode:0o700});
  const temporary = `${file}.tmp-${randomUUID()}`;
  let fd;
  try {
    fd = fs.openSync(temporary, 'wx', 0o600);
    fs.writeFileSync(fd, data);
    fs.fsyncSync(fd);
    fs.closeSync(fd); fd = undefined;
    fs.renameSync(temporary, file);
    if (process.platform !== 'win32') {
      const directory = fs.openSync(path.dirname(file), 'r');
      try { fs.fsyncSync(directory); } finally { fs.closeSync(directory); }
    }
  } finally {
    if (fd !== undefined) fs.closeSync(fd);
    fs.rmSync(temporary, {force:true});
  }
}

const syncDir = dir => {
  if (process.platform === 'win32') return;
  const directory = fs.openSync(dir, 'r');
  try { fs.fsyncSync(directory); } finally { fs.closeSync(directory); }
};
// At most this many distinct set-aside copies of one key file are kept. A keyring that keeps
// changing its answer can't fill the folder: past the limit nothing more is set aside (and nothing
// that would need setting aside is replaced). Nothing is ever deleted to make room.
const SET_ASIDE_LIMIT = 8;
// The set-aside copies of `file` under `label`, newest first (by when the bytes were written; a
// set-aside is a hard link, so that is the original file's time).
function asideOf(file, label) {
  const dir = path.dirname(file), prefix = `${path.basename(file)}.${label}-`;
  let names = [];
  try { names = fs.readdirSync(dir); } catch (e) { if (e.code !== 'ENOENT') throw e; }
  return names.filter(n => n.startsWith(prefix) && !n.includes('.tmp-')).map(n => {
    const full = path.join(dir, n);
    try { const st = fs.lstatSync(full); return st.isFile() ? {file:full, time:st.mtimeMs} : null; } catch { return null; }
  }).filter(Boolean).sort((a, b) => b.time - a.time || (b.file > a.file ? 1 : -1)).map(e => e.file);
}
const sameBytes = (a, b) => { try { return readRegular(a).equals(readRegular(b)); } catch { return false; } };

// Moves an unreadable or superseded key file out of the way under a new, unique name in the same
// folder. Nothing is deleted: the bytes stay on disk for a later manual recovery. When an identical
// copy is already set aside, the file is only unlinked (that copy remains). With `keep`, the file
// also stays where it is (a second name is added). Past SET_ASIDE_LIMIT copies it refuses.
function setAside(file, label = 'unreadable', keep = false) {
  const existing = asideOf(file, label);
  const same = existing.find(other => sameBytes(file, other));
  if (same) {
    if (!keep) { fs.unlinkSync(file); syncDir(path.dirname(file)); }
    return same;
  }
  if (existing.length >= SET_ASIDE_LIMIT) throw Object.assign(new Error('Too many set-aside key files.'), {code:'FB_SET_ASIDE_LIMIT'});
  const stamp = new Date().toISOString().replace(/[-:]/g, '').replace(/\.\d+Z$/, 'Z');
  for (let n = 0; n < 100; n++) {
    const target = `${file}.${label}-${stamp}${n ? '-' + n : ''}`;
    try { fs.linkSync(file, target); }
    catch (e) { if (e.code === 'EEXIST') continue; throw e; }
    if (!keep) fs.unlinkSync(file);
    syncDir(path.dirname(file));
    return target;
  }
  throw new Error('A key file could not be set aside.');
}
// Put a set-aside copy back in place of `file` in one rename. The file it replaces, if any, is kept
// under a set-aside name first (or matches one already kept), so no bytes are lost either way.
function restoreAside(candidate, file) {
  if (fs.existsSync(file)) setAside(file, 'unreadable', true);
  fs.renameSync(candidate, file);
  syncDir(path.dirname(file));
}

// Connection-key states this launch can be in. `replaced` tells sign-in why saved connections
// need connecting again; a connection no key here can read is flagged in any state but `unavailable`.
const CONNECTION = Object.freeze({READY:'ready', CREATED:'created', REPLACED:'replaced', UNAVAILABLE:'unavailable'});
const HEX_KEY = /^[0-9a-f]{64}$/i;

function store(root, storage = safeStorage) {
  const books = path.join(root, 'books');
  const defaultBook = path.join(books, 'default.fbbook');
  const legacySealed = path.join(root, 'book-key.sealed');
  const wrapPath = path.join(root, 'book-key.lock.json');
  const activeFile = path.join(root, 'active-book.json');
  let active = defaultBook;
  // Temporary files from a writePrivate that never renamed (an interrupted start) hold nothing needed.
  for (const name of fs.existsSync(root) ? fs.readdirSync(root) : []) {
    if (/^(?:active-book\.json|connection-key\.(?:sealed|private)|book-key\.(?:sealed|lock\.json))\.tmp-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(name)) {
      const file = path.join(root, name);
      if (fs.lstatSync(file).isFile()) fs.rmSync(file, {force:true});
    }
  }
  function acceptable(file) {
    return typeof file === 'string' && path.isAbsolute(file) && file === path.resolve(file) && file.startsWith(books + path.sep) && file.endsWith('.fbbook');
  }
  // `saved`: active-book.json names an existing book. `default`: no choice was ever saved.
  // `missing`/`unreadable`: a saved choice can't be followed, so nothing opens without the owner
  // choosing, and there is no main book until the owner makes one (fb-app's "Use this as my books").
  let activeChoice = 'default';
  const readChoice = () => {
    try {
      const saved = JSON.parse(readRegular(activeFile).toString('utf8'));
      return acceptable(saved.path) && fs.existsSync(saved.path) ? {state:'saved', path:saved.path} : {state:'missing'};
    } catch (e) { return {state: e.code === 'ENOENT' ? 'default' : 'unreadable'}; }
  };
  const choice = readChoice();
  activeChoice = choice.state;
  if (choice.path) active = choice.path;
  let main = ['saved', 'default'].includes(activeChoice) ? active : null;
  const backend = () => process.platform === 'linux' ? storage.getSelectedStorageBackend() : 'os';
  const usable = () => storage.isEncryptionAvailable() && backend() !== 'basic_text';
  // A keyring that seals and opens a fresh value right now. Only a working keyring can prove that a
  // saved file was sealed by a different one; anything less means "unavailable right now".
  let works;
  const keyringWorks = () => {
    if (works !== undefined) return works;
    works = false;
    try {
      if (usable()) {
        const probe = randomBytes(16).toString('hex');
        works = storage.decryptString(storage.encryptString(probe)) === probe;
      }
    } catch { works = false; }
    return works;
  };
  const sealedOf = file => fs.existsSync(file) ? file : null;
  function sealVerified(file, value) {
    writePrivate(file, storage.encryptString(value));
    if (storage.decryptString(readRegular(file)) !== value) throw new Error('The keyring did not keep the saved key.');
  }
  return {
    bookPath: () => active,
    defaultBook: () => defaultBook,
    mainBook: () => main,
    activeChoice: () => activeChoice,
    exists: () => fs.existsSync(active),
    backend,
    passwordRequired: () => !usable(),
    osStoreUsable: usable,
    keyringWorks,
    hasPasswordWrap: () => active === defaultBook && fs.existsSync(wrapPath),
    readWrap: () => readRegular(wrapPath).toString('utf8'),
    returnToMain() { if (main) active = main; },
    // Every book file this installation can open: the main one, restored copies and set-aside books.
    books() {
      const found = [];
      const add = (file, kind) => {
        try {
          const st = fs.lstatSync(file);
          if (st.isFile() && acceptable(file) && !found.some(b => b.path === file)) found.push({path:file, kind, modified:st.mtimeMs, active:file === main, current:file === active});
        } catch { /* gone meanwhile */ }
      };
      const each = (dir, fn) => { try { for (const e of fs.readdirSync(dir, {withFileTypes:true})) fn(e); } catch (e) { if (e.code !== 'ENOENT') throw e; } };
      add(defaultBook, 'main');
      each(books, entry => {
        if (!entry.isDirectory()) return;
        const folder = path.join(books, entry.name);
        if (entry.name === 'set-aside') {
          each(folder, inner => { if (inner.isDirectory()) each(path.join(folder, inner.name), f => { if (f.isFile() && f.name.endsWith('.fbbook')) add(path.join(folder, inner.name, f.name), 'set-aside'); }); });
        } else each(folder, f => { if (f.isFile() && f.name.endsWith('.fbbook')) add(path.join(folder, f.name), 'restored'); });
      });
      if (main && !found.some(b => b.path === main)) add(main, 'restored');
      return found;
    },
    // Point the gate at another listed book without changing which book is saved as the main one.
    select(file) {
      if (!acceptable(file) || !this.books().some(b => b.path === file)) throw new Error('Choose one of the books listed on this computer.');
      active = file;
    },
    // After any open: the open book is the one shown. Only the main book itself is ever saved as the
    // choice (the first open of a never-saved default book); any other book is a copy being looked at.
    // Nothing here makes a book the main one: that is fb-app's confirmed, audited "Use this as my books".
    remember(file) {
      if (!acceptable(file)) throw new Error('Invalid active book.');
      active = file;
      if (file !== main || activeChoice === 'saved') return;
      writePrivate(activeFile, JSON.stringify({path:file}));
      activeChoice = 'saved';
    },
    isMain: file => !!main && file === main,
    // fb-app made `file` the main books and pointed active-book.json at it (or, when it could not
    // write it, finishes that at the next start from its committed journal). Follow it here; what
    // the file on disk says is re-read, not assumed.
    adoptMain(file) {
      if (!acceptable(file)) throw new Error('Invalid active book.');
      active = file; main = file;
      if (readChoice().path === file) activeChoice = 'saved';
    },
    // Why the saved book key can't open the books now: 'ask' (the owner chose a prompt), 'keyring'
    // (no working keyring this launch), 'missing' (none saved here) or 'unreadable' (sealed by another keyring).
    bookKeyStatus(askPassword) {
      if (askPassword) return 'ask';
      if (!keyringWorks()) return 'keyring';
      const file = sealedOf(active + '.os-key') || (active === defaultBook ? sealedOf(legacySealed) : null);
      if (!file) return 'missing';
      try { storage.decryptString(readRegular(file)); return 'present'; } catch { return 'unreadable'; }
    },
    // Saved book keys set aside earlier (unreadable at the time, perhaps only because the keyring
    // answered differently that launch) that this keyring opens now, newest first. The caller proves
    // one by opening the book with it before restoreBookKey puts it back.
    bookKeyCandidates() {
      if (!keyringWorks()) return [];
      const found = [];
      for (const file of asideOf(active + '.os-key', 'unreadable')) {
        try { const key = storage.decryptString(readRegular(file)); if (HEX_KEY.test(key)) found.push({file, key}); } catch { /* still unreadable */ }
      }
      return found;
    },
    restoreBookKey(candidate) {
      if (!asideOf(active + '.os-key', 'unreadable').includes(candidate)) throw new Error('Not a set-aside key of these books.');
      restoreAside(candidate, active + '.os-key');
    },
    // Returns true when an unreadable sealed key was set aside and replaced. A damaged file never blocks an unlock.
    sealBook(keyHex) {
      if (!keyringWorks()) return false;
      const file = active + '.os-key';
      let current = null, damaged = false;
      if (fs.existsSync(file)) {
        try { current = storage.decryptString(readRegular(file)); } catch { damaged = true; }
      }
      if (current === keyHex) return false;
      if (damaged) setAside(file);
      sealVerified(file, keyHex);
      return damaged;
    },
    readSealedKey() {
      if (!usable()) throw new Error('The saved key is not available on this computer. Enter your password or recovery key.');
      const file = fs.existsSync(active + '.os-key') ? active + '.os-key' : legacySealed;
      try { return storage.decryptString(readRegular(file)); }
      catch { throw new Error(SEALED_UNREADABLE); }
    },
    // Kept for old tools while the portable record is adopted; no key file is overwritten silently.
    saveKeys(keyHex, wrappedJson) {
      if (wrappedJson) { writePrivate(wrapPath, wrappedJson); fs.rmSync(legacySealed, {force:true}); }
      else if (usable()) writePrivate(legacySealed, storage.encryptString(keyHex));
      else throw new Error('An app-lock password is required on this computer.');
    },
    removeRedundantSealedKey() {
      if (fs.existsSync(wrapPath)) fs.rmSync(legacySealed, {force:true});
    },
    // Only for an owner's choice (ask every time, or the built-in key): the saved copy is redundant.
    removeSealedKey() {
      fs.rmSync(active + '.os-key', {force:true});
      if (active === defaultBook) fs.rmSync(legacySealed, {force:true});
    },
    // The installation's connection key and what this launch knows about it:
    //  ready       the saved key opened; connections work.
    //  created     no key existed; a new one was saved.
    //  replaced    a working keyring proved the sealed key unreadable (another keyring sealed it), and
    //              no key set aside earlier opens either. The file was set aside, a new key sealed;
    //              saved connections must be connected again. A set-aside key this keyring opens is
    //              put back instead (ready), so a keyring that answered differently for one launch
    //              costs nothing once it answers as before.
    //  unavailable the keyring can't be used right now. Nothing is written; connections wait.
    // `key` is null when unavailable. No key material is logged or put in an error.
    connectionKey() {
      const sealed = path.join(root, 'connection-key.sealed');
      const fallback = path.join(root, 'connection-key.private');
      const ok = key => typeof key === 'string' && HEX_KEY.test(key);
      const unavailable = reason => ({key:null, state:CONNECTION.UNAVAILABLE, reason});
      const readFallback = () => {
        const st = fs.lstatSync(fallback);
        if (!st.isFile() || (process.platform !== 'win32' && (st.mode & 0o077))) return null;
        const key = readRegular(fallback).toString('utf8');
        return ok(key) ? key : null;
      };
      // Plaintext keys an older version left and a readable sealed key superseded. Saved connections
      // written under one of them are read with it (and re-saved under the key in use).
      const alternates = () => asideOf(fallback, 'superseded').slice(0, 4).map(file => {
        try {
          const st = fs.lstatSync(file);
          if (!st.isFile() || (process.platform !== 'win32' && (st.mode & 0o077))) return null;
          const key = readRegular(file).toString('utf8');
          return ok(key) ? key : null;
        } catch { return null; }
      }).filter(Boolean);
      // A sealed key set aside earlier that this keyring opens: a keyring that answered with a
      // different secret for one launch (a locked collection, a duplicate item) has come back.
      const reopen = () => {
        for (const file of asideOf(sealed, 'unreadable')) {
          try { const key = storage.decryptString(readRegular(file)); if (ok(key)) return {file, key}; } catch { /* not this keyring's */ }
        }
        return null;
      };
      const limited = e => e && e.code === 'FB_SET_ASIDE_LIMIT';
      if (fs.existsSync(sealed)) {
        // A sealed key exists: never mint a replacement unless a working keyring proves it unreadable.
        if (!keyringWorks()) return unavailable('keyring');
        let key = null;
        try { key = storage.decryptString(readRegular(sealed)); } catch { key = null; }
        if (ok(key)) {
          // A plaintext key written by an older version while the keyring was away: kept aside, and
          // offered for reading what was saved under it.
          if (fs.existsSync(fallback)) {
            try { setAside(fallback, 'superseded'); } catch (e) { if (!limited(e)) throw e; }
          }
          return {key, state:CONNECTION.READY, alternates:alternates()};
        }
        // Before deciding the key is lost, try every key set aside earlier.
        const found = reopen();
        if (found) {
          try { restoreAside(found.file, sealed); }
          catch (e) { if (limited(e)) return unavailable('set-aside-limit'); throw e; }
          if (storage.decryptString(readRegular(sealed)) !== found.key) throw new Error('The keyring did not keep the saved key.');
          return {key:found.key, state:CONNECTION.READY, restored:true, alternates:alternates()};
        }
        // Proven unreadable here: keep the old file, start a new key, connections reconnect once.
        let kept;
        try { kept = setAside(sealed); }
        catch (e) { if (limited(e)) return unavailable('set-aside-limit'); throw e; }
        let next = fs.existsSync(fallback) ? readFallback() : null;
        if (!next) next = randomBytes(32).toString('hex');
        sealVerified(sealed, next);
        if (fs.existsSync(fallback)) {
          try { setAside(fallback, 'superseded'); } catch (e) { if (!limited(e)) throw e; }
        }
        return {key:next, state:CONNECTION.REPLACED, setAside:path.basename(kept), alternates:alternates().filter(k => k !== next)};
      }
      if (fs.existsSync(fallback)) {
        const key = readFallback();
        if (!key) return unavailable('fallback');
        if (keyringWorks()) {
          // The same key, now sealed and proven readable, so the plaintext copy can go.
          sealVerified(sealed, key);
          fs.unlinkSync(fallback);
        }
        return {key, state:CONNECTION.READY, alternates:alternates()};
      }
      // No key in place. A key set aside earlier is never abandoned for a new one: put back one this
      // keyring opens; with no working keyring, wait.
      const earlier = asideOf(sealed, 'unreadable').length > 0;
      if (earlier) {
        if (!keyringWorks()) return unavailable('keyring');
        const found = reopen();
        if (found) {
          restoreAside(found.file, sealed);
          return {key:found.key, state:CONNECTION.READY, restored:true, alternates:alternates()};
        }
      }
      const key = randomBytes(32).toString('hex');
      if (keyringWorks()) sealVerified(sealed, key);
      else writePrivate(fallback, key);
      // Saved connections were written under a key this computer no longer reads: say so once.
      return {key, state: earlier ? CONNECTION.REPLACED : CONNECTION.CREATED, alternates:alternates()};
    },
  };
}
// Shared by startup and the unlock form for books without a portable key record.
function openLegacyBook(native, local, secret = '', allowSavedKey = true) {
  try { return JSON.parse(native.openProtectedBook(local.bookPath(), secret)); }
  catch (openingError) {
    if (openingError.code !== 'FB_BOOK_WRONG_KEY' || !allowSavedKey) throw openingError;
    // Opening may have recovered a displaced portable record. Its password policy now applies:
    // a stale saved key must never turn rejected input into a successful modern-book unlock.
    if (JSON.parse(native.protectionState(local.bookPath())).known !== false) throw openingError;
    let key;
    try { key = local.readSealedKey(); }
    catch { throw openingError; }
    return JSON.parse(native.openBook(local.bookPath(), key));
  }
}
module.exports = {userDataDir, store, writePrivate, setAside, SET_ASIDE_LIMIT, CONNECTION, SEALED_UNREADABLE, openLegacyBook};

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
  try {
    const saved = JSON.parse(readRegular(activeFile).toString('utf8'));
    if (acceptable(saved.path) && fs.existsSync(saved.path)) active = saved.path;
  } catch (e) { if (e.code !== 'ENOENT') throw e; }
  let main = active;
  const backend = () => process.platform === 'linux' ? storage.getSelectedStorageBackend() : 'os';
  const usable = () => storage.isEncryptionAvailable() && backend() !== 'basic_text';
  return {
    bookPath: () => active,
    defaultBook: () => defaultBook,
    exists: () => fs.existsSync(active),
    backend,
    passwordRequired: () => !usable(),
    osStoreUsable: usable,
    hasPasswordWrap: () => active === defaultBook && fs.existsSync(wrapPath),
    readWrap: () => readRegular(wrapPath).toString('utf8'),
    returnToMain() { active = main; },
    remember(file, temporary = false) {
      if (!acceptable(file)) throw new Error('Invalid active book.');
      if (temporary && fs.existsSync(main)) { active = file; return; }
      if (active === file && fs.existsSync(activeFile)) return;
      active = file;
      main = file;
      writePrivate(activeFile, JSON.stringify({path:file}));
    },
    // Returns true when an unreadable sealed key was replaced. A damaged file never blocks an unlock.
    sealBook(keyHex) {
      if (!usable()) return false;
      const file = active + '.os-key';
      let current = null, damaged = false;
      if (fs.existsSync(file)) {
        try { current = storage.decryptString(readRegular(file)); } catch { damaged = true; }
      }
      if (current === keyHex) return false;
      writePrivate(file, storage.encryptString(keyHex));
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
    removeSealedKey() {
      fs.rmSync(active + '.os-key', {force:true});
      if (active === defaultBook) fs.rmSync(legacySealed, {force:true});
    },
    connectionKey() {
      const sealed = path.join(root, 'connection-key.sealed');
      const fallback = path.join(root, 'connection-key.private');
      let key;
      // The 0600 file is newer than any sealed key: it is written only when the keyring is unusable.
      if (fs.existsSync(fallback)) {
        const st = fs.lstatSync(fallback);
        if (process.platform !== 'win32' && (st.mode & 0o077)) throw new Error('The connection key must be readable only by its owner.');
        key = readRegular(fallback).toString('utf8');
        if (usable()) { writePrivate(sealed, storage.encryptString(key)); fs.unlinkSync(fallback); }
      } else if (fs.existsSync(sealed) && usable()) {
        key = storage.decryptString(readRegular(sealed));
      } else if (fs.existsSync(sealed)) {
        // The keyring went away: one new key, kept in the 0600 file. Saved connections need reconnecting once.
        key = randomBytes(32).toString('hex');
        writePrivate(fallback, key);
      } else {
        key = randomBytes(32).toString('hex');
        if (usable()) writePrivate(sealed, storage.encryptString(key));
        else writePrivate(fallback, key);
      }
      if (!/^[0-9a-f]{64}$/i.test(key)) throw new Error('The connection key is damaged.');
      return key;
    },
  };
}
// Shared by startup and the unlock form for books without a portable key record.
function openLegacyBook(native, local, secret = '', allowSavedKey = true) {
  try { return JSON.parse(native.openProtectedBook(local.bookPath(), secret)); }
  catch (openingError) {
    if (openingError.code !== 'FB_BOOK_WRONG_KEY' || !allowSavedKey) throw openingError;
    let key;
    try { key = local.readSealedKey(); }
    catch { throw openingError; }
    return JSON.parse(native.openBook(local.bookPath(), key));
  }
}
module.exports = {userDataDir, store, writePrivate, SEALED_UNREADABLE, openLegacyBook};

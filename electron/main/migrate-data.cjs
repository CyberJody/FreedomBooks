'use strict';
const fs = require('node:fs');
const path = require('node:path');
const {createHash} = require('node:crypto');

function syncDir(directory) {
  const fd = fs.openSync(directory, 'r');
  try {fs.fsyncSync(fd);} finally {fs.closeSync(fd);}
}
function hash(file) {return createHash('sha256').update(fs.readFileSync(file)).digest('hex');}

// A fixed private stage makes interrupted moves resumable. Nothing is published
// as the active book until books and both possible key wrappers have moved.
function migrate(source, destination, io = fs) {
  const stage = path.join(path.dirname(destination), '.FreedomBooks-migration');
  const payload = ['books', 'book-key.lock.json', 'book-key.sealed'];
  if (!fs.existsSync(stage) && !fs.existsSync(path.join(source, 'books/default.fbbook'))) return;
  if (fs.existsSync(path.join(destination, 'books/default.fbbook'))) return;
  if (fs.existsSync(destination)) {
    // Never mix an unrelated/new key with the old book, or overwrite any data.
    if (fs.readdirSync(destination).length) throw new Error('Book migration destination is not empty; keep both folders and contact support.');
    fs.rmdirSync(destination);
  }
  fs.mkdirSync(stage, {recursive: true, mode: 0o700});
  fs.chmodSync(stage, 0o700);
  function validate(file) {
    const stat = fs.lstatSync(file);
    if (stat.isSymbolicLink() || (!stat.isFile() && !stat.isDirectory())) throw new Error('Book migration refuses linked or special files.');
    fs.chmodSync(file, stat.isDirectory() ? 0o700 : 0o600);
    if (stat.isDirectory()) for (const name of fs.readdirSync(file)) validate(path.join(file, name));
  }
  function move(from, to) {
    const stat = fs.lstatSync(from);
    if (stat.isSymbolicLink() || (!stat.isFile() && !stat.isDirectory())) throw new Error('Book migration refuses linked or special files.');
    if (fs.existsSync(to)) {
      if (stat.isDirectory() && fs.lstatSync(to).isDirectory()) {
        for (const name of fs.readdirSync(from)) move(path.join(from, name), path.join(to, name));
        fs.rmdirSync(from);
        return;
      }
      // A cross-device copy can survive a power loss before source removal.
      if (!stat.isFile() || !fs.lstatSync(to).isFile() || hash(from) !== hash(to)) throw new Error('Conflicting files during book migration.');
      fs.chmodSync(to, 0o600);
      fs.unlinkSync(from);
      syncDir(path.dirname(from));
      return;
    }
    fs.chmodSync(from, stat.isDirectory() ? 0o700 : 0o600);
    try {
      io.renameSync(from, to);
      syncDir(path.dirname(to));
      syncDir(path.dirname(from));
      return;
    } catch (error) {if (error.code !== 'EXDEV') throw error;}
    if (stat.isDirectory()) {
      fs.mkdirSync(to, {mode: 0o700});
      for (const name of fs.readdirSync(from)) move(path.join(from, name), path.join(to, name));
      syncDir(to);
      fs.rmdirSync(from);
    } else {
      const temp = `${to}.copying`;
      // Only this migration owns the private stage; discard an interrupted copy.
      fs.rmSync(temp, {force: true});
      fs.copyFileSync(from, temp, fs.constants.COPYFILE_EXCL);
      fs.chmodSync(temp, 0o600);
      const fd = fs.openSync(temp, 'r');
      try {fs.fsyncSync(fd);} finally {fs.closeSync(fd);}
      if (hash(from) !== hash(temp)) throw new Error('Book migration copy hash mismatch.');
      fs.renameSync(temp, to);
      syncDir(path.dirname(to));
      fs.unlinkSync(from);
    }
    syncDir(path.dirname(from));
    syncDir(path.dirname(to));
  }
  for (const name of payload) {
    const from = path.join(source, name);
    if (fs.existsSync(from)) {validate(from); move(from, path.join(stage, name));}
  }
  if (!fs.existsSync(path.join(stage, 'books/default.fbbook'))) throw new Error('Book migration is incomplete.');
  fs.renameSync(stage, destination);
  syncDir(path.dirname(destination));
}

module.exports = {migrate};

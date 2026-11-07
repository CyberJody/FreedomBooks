// User-selected destinations only. Internal book/key writes never use a Save dialog.
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const {randomUUID} = require('node:crypto');
const MESSAGE = 'Choose a different file name. That location is used by FreedomBooks.';
const refuse = () => { throw new Error(MESSAGE); };

function stat(file) {
  try { return fs.lstatSync(file); }
  catch (error) { if (error.code === 'ENOENT') return null; throw error; }
}
// Resolve existing ancestors too: a not-yet-created data folder can have a symlinked parent.
function real(file) {
  try { return fs.realpathSync.native(file); }
  catch (error) {
    if (error.code !== 'ENOENT') throw error;
    const parent = path.dirname(file);
    if (parent === file) throw error;
    return path.join(real(parent), path.basename(file));
  }
}
function inside(file, root) {
  const relative = path.relative(root, file);
  return relative === '' || (!relative.startsWith('..' + path.sep) && relative !== '..' && !path.isAbsolute(relative));
}

function create({dataDir, bookPath = () => null} = {}) {
  function validate(file) {
    if (typeof file !== 'string' || !path.isAbsolute(file)) refuse();
    const chosen = path.resolve(file);
    const entry = stat(file);
    if (entry && (!entry.isFile() || entry.isSymbolicLink())) refuse();
    const parent = fs.realpathSync.native(path.dirname(file));
    const resolved = entry ? fs.realpathSync.native(file) : path.join(parent, path.basename(chosen));
    const xdgHome = process.env.XDG_DATA_HOME || path.join(os.homedir(), '.local/share');
    const roots = [xdgHome + path.sep + 'FreedomBooks', dataDir].filter(Boolean);
    const book = bookPath();
    // Book key sidecars (.keys.json, .os-key, .rekey*) sit beside the book; older book-key.* files and the
    // connection key sit in the data directory (keys.cjs). Listed here so hard links to them refuse too.
    const keyFiles = dataDir ? ['book-key.sealed', 'book-key.lock.json', 'connection-key.sealed', 'connection-key.private'].map(name => path.join(dataDir, name)) : [];
    const sidecars = ['-wal', '-shm', '.key', '-key', '.keys.json', '.os-key', '.rekey.json', '.rekey-before'];
    const protectedFiles = [...(book ? [book, ...sidecars.map(suffix => book + suffix)] : []), ...keyFiles];
    for (const candidate of [chosen, resolved]) {
      if (/\.fbbook/i.test(path.basename(candidate))) refuse();
      for (const root of roots) {
        if (inside(candidate, path.resolve(root)) || inside(candidate, real(path.isAbsolute(root) ? root : path.resolve(root)))) refuse();
      }
      for (const sensitive of protectedFiles) {
        if (candidate === path.resolve(sensitive) || candidate === real(path.isAbsolute(sensitive) ? sensitive : path.resolve(sensitive))) refuse();
        const info = stat(sensitive);
        if (entry && info && entry.dev === info.dev && entry.ino === info.ino) refuse();
      }
    }
    return {parent, name: path.basename(chosen)};
  }

  function write(file, contents) {
    const selected = validate(file);
    const dir = process.platform === 'win32' ? null : fs.openSync(selected.parent, fs.constants.O_RDONLY | (fs.constants.O_DIRECTORY || 0) | (fs.constants.O_NOFOLLOW || 0));
    // Linux fd-relative paths pin the directory even if an ancestor is renamed or made a symlink.
    // Elsewhere use the canonical directory and check its identity before replacing the leaf.
    const directory = process.platform === 'linux' ? `/proc/self/fd/${dir}` : selected.parent;
    const destination = path.join(directory, selected.name);
    const temporary = path.join(directory, `.freedombooks-save-${randomUUID()}.tmp`);
    let fd;
    let created = false;
    try {
      const opened = dir === null ? fs.statSync(selected.parent) : fs.fstatSync(dir);
      const current = fs.statSync(selected.parent);
      if (opened.dev !== current.dev || opened.ino !== current.ino) refuse();
      validate(destination);
      // wx is O_CREAT|O_EXCL: even a dangling link at the temporary name is refused.
      fd = fs.openSync(temporary, 'wx', 0o600);
      created = true;
      fs.writeFileSync(fd, contents, {encoding:'utf8'});
      fs.fsyncSync(fd);
      fs.closeSync(fd);
      fd = undefined;
      validate(destination);
      const currentParent = fs.statSync(selected.parent);
      if (opened.dev !== currentParent.dev || opened.ino !== currentParent.ino) refuse();
      // rename replaces the directory entry; it never opens/follows a raced destination link.
      fs.renameSync(temporary, destination);
      created = false;
      if (process.platform !== 'win32') fs.fsyncSync(dir);
    } finally {
      try {
        if (fd !== undefined) fs.closeSync(fd);
        if (created) fs.unlinkSync(temporary);
      } finally {
        if (dir !== null) fs.closeSync(dir);
      }
    }
  }
  return {validate, write};
}
module.exports = {create, MESSAGE};

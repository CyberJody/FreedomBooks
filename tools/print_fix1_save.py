"""Install one main-process policy for every user-selected output destination."""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

def edit(name, old, new):
    p = ROOT / name
    text = p.read_text()
    assert old in text, (name, old)
    p.write_text(text.replace(old, new))

(ROOT / 'electron/main/save-file.cjs').write_text(r'''// User-selected destinations only. Internal book/key writes never use a Save dialog.
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
  try { return fs.realpathSync(file); }
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
    const entry = stat(chosen);
    if (entry && (!entry.isFile() || entry.isSymbolicLink())) refuse();
    const parent = fs.realpathSync(path.dirname(chosen));
    const resolved = entry ? fs.realpathSync(chosen) : path.join(parent, path.basename(chosen));
    const roots = [path.join(process.env.XDG_DATA_HOME || path.join(os.homedir(), '.local/share'), 'FreedomBooks'), dataDir].filter(Boolean);
    const book = bookPath();
    const protectedFiles = book ? [book, book + '-wal', book + '-shm', book + '.key', book + '-key',
      path.join(path.dirname(book), 'book-key.sealed'), path.join(path.dirname(book), 'book-key.lock.json')] : [];
    for (const candidate of [chosen, resolved]) {
      if (/\.fbbook/i.test(path.basename(candidate))) refuse();
      for (const root of roots) {
        if (inside(candidate, path.resolve(root)) || inside(candidate, real(path.resolve(root)))) refuse();
      }
      for (const sensitive of protectedFiles) {
        if (candidate === path.resolve(sensitive) || candidate === real(path.resolve(sensitive))) refuse();
        const info = stat(sensitive);
        if (entry && info && entry.dev === info.dev && entry.ino === info.ino) refuse();
      }
    }
    return {parent, name: path.basename(chosen)};
  }

  function write(file, contents) {
    const selected = validate(file);
    const dir = fs.openSync(selected.parent, fs.constants.O_RDONLY | (fs.constants.O_DIRECTORY || 0) | (fs.constants.O_NOFOLLOW || 0));
    // Linux fd-relative paths pin the directory even if an ancestor is renamed or made a symlink.
    // Elsewhere use the canonical directory and check its identity before replacing the leaf.
    const directory = process.platform === 'linux' ? `/proc/self/fd/${dir}` : selected.parent;
    const destination = path.join(directory, selected.name);
    const temporary = path.join(directory, `.freedombooks-save-${randomUUID()}.tmp`);
    let fd;
    let created = false;
    try {
      const opened = fs.fstatSync(dir);
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
      if (fd !== undefined) fs.closeSync(fd);
      if (created) fs.unlinkSync(temporary);
      fs.closeSync(dir);
    }
  }
  return {validate, write};
}
module.exports = {create, MESSAGE};
''')
edit('electron/main/printing.cjs', "const fs = require('node:fs');", "const saves = require('./save-file.cjs');")
edit('electron/main/printing.cjs', 'function controller(native) {', "function controller(native, output = saves.create({bookPath:()=>JSON.parse(native.appState()).path})) {")
edit('electron/main/printing.cjs', '      current(win,target); // A lock/navigation', '      output.validate(selected.filePath);\n      current(win,target); // A lock/navigation')
edit('electron/main/printing.cjs', '      fs.writeFileSync(selected.filePath,pdf,{mode:0o600});\n      fs.chmodSync(selected.filePath,0o600);', '      output.write(selected.filePath,pdf);')
edit('electron/main/main.cjs', "const printing = require('./printing.cjs').controller(native);\n", '')
edit('electron/main/main.cjs', 'const store = keys.store(userData);', "const store = keys.store(userData);\nconst output = require('./save-file.cjs').create({dataDir:userData, bookPath:()=>JSON.parse(native.appState()).path || store.bookPath()});\nconst printing = require('./printing.cjs').controller(native, output);")
edit('electron/main/main.cjs', "filters: [{ name: 'CSV', extensions: ['csv'] }]", "filters: [{ name: 'CSV', extensions: ['csv'] }], properties: ['showOverwriteConfirmation']")
edit('electron/main/main.cjs', "      fs.writeFileSync(selected.filePath, outcome.contents, { encoding: 'utf8', mode: 0o600 });", "      output.write(selected.filePath, outcome.contents);")
edit('electron/main/main.cjs', "const fs = require('node:fs');\n", '')
# Keep the existing controller tests focused on UI authorization; the filesystem policy has real-I/O tests.
edit('electron/tests/printing.test.cjs', "const sandbox={URL,module:{exports:{}},require:name=>name==='electron'?electron:{writeFileSync:(file,pdf,options)=>state.saved.push({file,pdf,options}),chmodSync:(_file,mode)=>state.calls.push(['mode',mode])}};", "const output={validate:()=>{},write:(file,pdf)=>state.saved.push({file,pdf,options:{mode:0o600}})};\n  const sandbox={URL,module:{exports:{}},require:name=>name==='electron'?electron:require(name.startsWith('.')?path.join(__dirname,'../main',name):name)};")
edit('electron/tests/printing.test.cjs', 'sandbox.module.exports.controller(native)', 'sandbox.module.exports.controller(native,output)')
print('Installed shared protected atomic saves for PDF and every CSV export')

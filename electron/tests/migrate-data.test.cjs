'use strict';
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const {migrate} = require('../main/migrate-data.cjs');

for (const crossDevice of [false, true]) test(`book and key migration; EXDEV=${crossDevice}`, () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'fb-migrate-'));
  try {
    const source = path.join(root, 'config/app.freedombooks.FreedomBooks');
    const dest = path.join(root, 'data/FreedomBooks');
    fs.mkdirSync(path.join(source, 'books'), {recursive: true, mode: 0o700});
    const files = {'books/default.fbbook': 'encrypted fixture', 'book-key.lock.json': 'wrapped fixture', 'book-key.sealed': 'sealed fixture'};
    for (const [name, bytes] of Object.entries(files)) fs.writeFileSync(path.join(source, name), bytes, {mode: 0o600});
    fs.writeFileSync(path.join(source, 'Preferences'), 'settings remain');
    let renames = 0;
    const io = {renameSync: (from, to) => {
      renames++;
      if (crossDevice) throw Object.assign(new Error('different filesystem'), {code: 'EXDEV'});
      fs.renameSync(from, to);
    }};
    migrate(source, dest, io);
    assert.ok(renames > 0);
    for (const [name, bytes] of Object.entries(files)) {
      assert.equal(fs.readFileSync(path.join(dest, name), 'utf8'), bytes);
      assert.equal(fs.statSync(path.join(dest, name)).mode & 0o777, 0o600);
      assert.equal(fs.existsSync(path.join(source, name)), false);
    }
    for (const directory of [dest, path.join(dest, 'books')]) assert.equal(fs.statSync(directory).mode & 0o777, 0o700);
    assert.equal(fs.readFileSync(path.join(source, 'Preferences'), 'utf8'), 'settings remain');
    // A different existing book is never overwritten by a legacy profile.
    fs.mkdirSync(path.join(source, 'books'));
    fs.writeFileSync(path.join(source, 'books/default.fbbook'), 'other book');
    migrate(source, dest, io);
    assert.equal(fs.readFileSync(path.join(dest, 'books/default.fbbook'), 'utf8'), files['books/default.fbbook']);
    assert.equal(fs.readFileSync(path.join(source, 'books/default.fbbook'), 'utf8'), 'other book');
  } finally {fs.rmSync(root, {recursive: true, force: true});}
});

test('interrupted migration resumes after book moved but before key move', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'fb-resume-'));
  try {
    const source = path.join(root, 'config');
    const dest = path.join(root, 'data/FreedomBooks');
    fs.mkdirSync(path.join(source, 'books'), {recursive: true});
    fs.writeFileSync(path.join(source, 'books/default.fbbook'), 'book');
    fs.writeFileSync(path.join(source, 'book-key.lock.json'), 'key');
    assert.throws(() => migrate(source, dest, {renameSync(from, to) {
      if (from.endsWith('book-key.lock.json')) throw new Error('power loss');
      fs.renameSync(from, to);
    }}), /power loss/);
    assert.equal(fs.existsSync(dest), false);
    migrate(source, dest);
    assert.equal(fs.readFileSync(path.join(dest, 'books/default.fbbook'), 'utf8'), 'book');
    assert.equal(fs.readFileSync(path.join(dest, 'book-key.lock.json'), 'utf8'), 'key');
  } finally {fs.rmSync(root, {recursive: true, force: true});}
});

test('cross-device corrupt copy is refused before the source is removed, then retries', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'fb-copy-'));
  const copy = fs.copyFileSync;
  try {
    const source = path.join(root, 'config');
    const dest = path.join(root, 'data/FreedomBooks');
    fs.mkdirSync(path.join(source, 'books'), {recursive: true});
    fs.writeFileSync(path.join(source, 'books/default.fbbook'), 'original bytes');
    fs.writeFileSync(path.join(source, 'book-key.lock.json'), 'wrapped key');
    fs.copyFileSync = (from, to, flags) => {copy(from, to, flags); fs.appendFileSync(to, 'corrupt');};
    const io = {renameSync() {throw Object.assign(new Error('EXDEV'), {code: 'EXDEV'});}};
    assert.throws(() => migrate(source, dest, io), /hash mismatch/);
    assert.equal(fs.readFileSync(path.join(source, 'books/default.fbbook'), 'utf8'), 'original bytes');
    assert.equal(fs.existsSync(dest), false);
    fs.copyFileSync = copy;
    migrate(source, dest, io);
    assert.equal(fs.readFileSync(path.join(dest, 'books/default.fbbook'), 'utf8'), 'original bytes');
    assert.equal(fs.readFileSync(path.join(dest, 'book-key.lock.json'), 'utf8'), 'wrapped key');
  } finally {fs.copyFileSync = copy; fs.rmSync(root, {recursive: true, force: true});}
});

test('destination key without a book is never mixed with the legacy book', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'fb-conflict-'));
  try {
    const source = path.join(root, 'config');
    const dest = path.join(root, 'data/FreedomBooks');
    fs.mkdirSync(path.join(source, 'books'), {recursive: true});
    fs.writeFileSync(path.join(source, 'books/default.fbbook'), 'legacy');
    fs.mkdirSync(dest, {recursive: true});
    fs.writeFileSync(path.join(dest, 'book-key.lock.json'), 'different key');
    assert.throws(() => migrate(source, dest), /destination is not empty/);
    assert.equal(fs.readFileSync(path.join(dest, 'book-key.lock.json'), 'utf8'), 'different key');
    assert.equal(fs.readFileSync(path.join(source, 'books/default.fbbook'), 'utf8'), 'legacy');
  } finally {fs.rmSync(root, {recursive: true, force: true});}
});

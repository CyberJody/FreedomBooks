const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const crypto = require('node:crypto');

function load(storage) {
  const sandbox = {module:{exports:{}},process,Buffer,require:name=>name==='electron'?{safeStorage:storage}:require(name)};
  vm.runInNewContext(fs.readFileSync(path.join(__dirname,'../main/keys.cjs'),'utf8'),sandbox);
  return sandbox.module.exports;
}
function fakeStorage(usable) {
  const key = crypto.randomBytes(32);
  return {
    isEncryptionAvailable:()=>true,
    getSelectedStorageBackend:()=>usable?'gnome_libsecret':'basic_text',
    encryptString(text) {
      const nonce=crypto.randomBytes(12), cipher=crypto.createCipheriv('aes-256-gcm',key,nonce);
      const bytes=Buffer.concat([cipher.update(text,'utf8'),cipher.final()]);
      return Buffer.concat([nonce,cipher.getAuthTag(),bytes]);
    },
    decryptString(bytes) {
      const cipher=crypto.createDecipheriv('aes-256-gcm',key,bytes.subarray(0,12));
      cipher.setAuthTag(bytes.subarray(12,28));
      return Buffer.concat([cipher.update(bytes.subarray(28)),cipher.final()]).toString('utf8');
    },
  };
}
function fixture(fn) {
  const root=fs.mkdtempSync(path.join(process.env.TMPDIR,'fb-key-storage-'));
  try { return fn(root); } finally { fs.rmSync(root,{recursive:true,force:true}); }
}
test('connection keys are independent per install and 0600 without a keyring',()=>fixture(root=>{
  const api=load(fakeStorage(false));
  const first=api.store(path.join(root,'first')), second=api.store(path.join(root,'second'));
  const a=first.connectionKey(), b=second.connectionKey();
  assert.notEqual(a,b);
  assert.equal(first.connectionKey(),a);
  assert.equal(fs.statSync(path.join(root,'first/connection-key.private')).mode & 0o777,0o600);
  assert.equal(fs.statSync(path.join(root,'first')).mode & 0o777,0o700);
  fs.chmodSync(path.join(root,'first/connection-key.private'),0o644);
  assert.throws(()=>first.connectionKey(),/only by its owner/);
}));
test('OS-sealed connection keys migrate the fallback without a plaintext residue',()=>fixture(root=>{
  const fallback=load(fakeStorage(false)).store(root);
  const expected=fallback.connectionKey();
  const storage=fakeStorage(true), api=load(storage), sealed=api.store(root);
  assert.equal(sealed.connectionKey(),expected);
  assert.ok(!fs.existsSync(path.join(root,'connection-key.private')));
  const bytes=fs.readFileSync(path.join(root,'connection-key.sealed'));
  assert.ok(!bytes.includes(Buffer.from(expected)));
  assert.equal(api.store(root).connectionKey(),expected);
  assert.throws(()=>load(fakeStorage(true)).store(root).connectionKey());
}));
test('atomic key writes are private and never follow the destination symlink',()=>fixture(root=>{
  const api=load(fakeStorage(false));
  const target=path.join(root,'key'), outside=path.join(root,'unrelated');
  fs.writeFileSync(outside,'untouched');
  fs.symlinkSync(outside,target);
  api.writePrivate(target,'synthetic new bytes');
  assert.equal(fs.readFileSync(outside,'utf8'),'untouched');
  assert.equal(fs.readFileSync(target,'utf8'),'synthetic new bytes');
  assert.equal(fs.statSync(target).mode & 0o777,0o600);
  assert.deepEqual(fs.readdirSync(root).sort(),['key','unrelated']);
}));
test('active books stay inside this installation and book seals are local',()=>fixture(root=>{
  const api=load(fakeStorage(true)), store=api.store(root);
  assert.throws(()=>store.remember('/outside.fbbook'),/Invalid active book/);
  const book=path.join(root,'books/restored-copy/copy.fbbook');
  fs.mkdirSync(path.dirname(book),{recursive:true}); fs.writeFileSync(book,'synthetic');
  store.remember(book); store.sealBook('synthetic-book-key');
  assert.equal(api.store(root).bookPath(),book);
  assert.equal(api.store(root).readSealedKey(),'synthetic-book-key');
  assert.ok(!fs.readFileSync(book+'.os-key').includes(Buffer.from('synthetic-book-key')));
  store.removeSealedKey();
  assert.ok(!fs.existsSync(book+'.os-key'));
}));
test('temporary restored viewing returns to the main book on lock and restart',()=>fixture(root=>{
  const api=load(fakeStorage(true)), store=api.store(root);
  const main=store.defaultBook(), restored=path.join(root,'books/restored-copy/copy.fbbook');
  fs.mkdirSync(path.dirname(main),{recursive:true});
  fs.mkdirSync(path.dirname(restored),{recursive:true});
  fs.writeFileSync(main,'current synthetic data'); fs.writeFileSync(restored,'older synthetic data');
  store.remember(main);
  store.remember(restored,true);
  assert.equal(store.bookPath(),restored);
  assert.equal(api.store(root).bookPath(),main);
  store.returnToMain();
  assert.equal(store.bookPath(),main);
  assert.equal(fs.readFileSync(store.bookPath(),'utf8'),'current synthetic data');
}));
test('a damaged or unreadable sealed book key is replaced, never thrown at a correct unlock',()=>fixture(root=>{
  const storage=fakeStorage(true), api=load(storage), store=api.store(root);
  const book=path.join(root,'books/default.fbbook');
  fs.mkdirSync(path.dirname(book),{recursive:true}); fs.writeFileSync(book,'synthetic');
  fs.writeFileSync(book+'.os-key',Buffer.from('synthetic garbage, not ciphertext'));
  assert.throws(()=>store.readSealedKey(),{message:api.SEALED_UNREADABLE});
  assert.equal(store.sealBook('synthetic-book-key'),true);
  assert.equal(store.readSealedKey(),'synthetic-book-key');
  assert.equal(store.sealBook('synthetic-book-key'),false);
  // Sealed by a keyring this computer no longer has: same plain answer, same repair.
  fs.writeFileSync(book+'.os-key',fakeStorage(true).encryptString('another computer'));
  assert.throws(()=>store.readSealedKey(),{message:api.SEALED_UNREADABLE});
  assert.equal(store.sealBook('synthetic-book-key'),true);
  assert.equal(api.store(root).readSealedKey(),'synthetic-book-key');
}));
test('a keyring that disappears after sealing gives one persisted 0600 connection key, not one per launch',()=>fixture(root=>{
  const storage=fakeStorage(true);
  const sealedKey=load(storage).store(root).connectionKey();
  const gone={...storage,getSelectedStorageBackend:()=>'basic_text'};
  const first=load(gone).store(root).connectionKey();
  assert.notEqual(first,sealedKey);
  assert.equal(load(gone).store(root).connectionKey(),first);
  assert.equal(fs.statSync(path.join(root,'connection-key.private')).mode & 0o777,0o600);
  // The keyring returns: the key saved meanwhile is kept and sealed again; the plaintext file goes.
  assert.equal(load(storage).store(root).connectionKey(),first);
  assert.ok(!fs.existsSync(path.join(root,'connection-key.private')));
  assert.equal(load(storage).store(root).connectionKey(),first);
}));
test('temporary key files left by an interrupted write are removed on the next start',()=>fixture(root=>{
  const id=crypto.randomUUID();
  const leftovers=[`active-book.json.tmp-${id}`,`connection-key.sealed.tmp-${id}`,`connection-key.private.tmp-${id}`];
  for(const name of leftovers)fs.writeFileSync(path.join(root,name),'synthetic');
  fs.writeFileSync(path.join(root,'notes.tmp-'+id),'not ours');
  load(fakeStorage(true)).store(root);
  assert.deepEqual(fs.readdirSync(root).sort(),['notes.tmp-'+id]);
}));


for (const [kind, message] of [
  ['damaged', "This file is damaged or isn't a FreedomBooks book. Nothing was changed."],
  ['too-new', 'These books were saved by a newer version of FreedomBooks (book version 67; this app reads up to 65). Install the latest version to open them. Nothing was changed.'],
  ['wrong-key', "That password or key doesn't open these books. Check it and try again, or use your recovery key."],
]) {
  test(`legacy opening preserves ${kind} when the saved key is unavailable`, () => {
    const {openLegacyBook} = load(fakeStorage(false));
    const opening = new Error(message);
    opening.code = kind === 'wrong-key' ? 'FB_BOOK_WRONG_KEY' : 'GenericFailure';
    const local = {bookPath:()=>'/synthetic/default.fbbook', readSealedKey:()=>{throw new Error('saved key unavailable');}};
    const native = {openProtectedBook:()=>{throw opening;}, openBook:()=>{throw new Error('must not open without a key');}};
    assert.throws(()=>openLegacyBook(native, local, 'synthetic input'), error=>error===opening);
  });
}

test('legacy opening preserves the native failure without an OS key store at startup', () => {
  const {openLegacyBook} = load(fakeStorage(false));
  const opening = new Error("This file is damaged or isn't a FreedomBooks book. Nothing was changed.");
  const local = {bookPath:()=>'/synthetic/default.fbbook', readSealedKey:()=>{throw new Error('must not read a key without an OS store');}};
  assert.throws(()=>openLegacyBook({openProtectedBook:()=>{throw opening;}}, local, '', false), error=>error===opening);
});

test('legacy opening still opens a copied built-in book without reading a local key', () => {
  const {openLegacyBook} = load(fakeStorage(false));
  const local = {bookPath:()=>'/synthetic/default.fbbook', readSealedKey:()=>{throw new Error('must not read a key');}};
  const opened = openLegacyBook({openProtectedBook:()=>JSON.stringify({home:'/synthetic/home/'})}, local, '', false);
  assert.equal(opened.home, '/synthetic/home/');
});

test('legacy opening keeps the native result or failure after a readable saved key', () => {
  const {openLegacyBook} = load(fakeStorage(true));
  const calls = [];
  const local = {bookPath:()=>'/synthetic/default.fbbook', readSealedKey:()=> 'synthetic saved key'};
  const wrongKey = new Error('wrong built-in key');
  wrongKey.code = 'FB_BOOK_WRONG_KEY';
  const native = {openProtectedBook:()=>{throw wrongKey;}, openBook:(book,key)=>{calls.push([book,key]);return JSON.stringify({home:'/legacy/home/'});}};
  assert.equal(openLegacyBook(native, local).home, '/legacy/home/');
  assert.deepEqual(calls, [['/synthetic/default.fbbook', 'synthetic saved key']]);
  const opening = new Error('book is too new');
  native.openBook = ()=>{throw opening;};
  assert.throws(()=>openLegacyBook(native, local), error=>error===opening);
});


test('legacy opening never masks a native refusal with a readable stale key', () => {
  const {openLegacyBook} = load(fakeStorage(true));
  for (const message of ['book is too new', 'book is damaged', 'upgrade copy failed']) {
    const opening = new Error(message);
    opening.code = 'GenericFailure';
    let reads = 0;
    const local = {bookPath:()=>'/synthetic/default.fbbook', readSealedKey:()=>{reads++;return 'stale saved key';}};
    const native = {openProtectedBook:()=>{throw opening;}, openBook:()=>{throw new Error('wrong saved key');}};
    assert.throws(()=>openLegacyBook(native, local), error=>error===opening);
    assert.equal(reads, 0);
  }
});

for (const [name, storage] of [
  ['unavailable', {...fakeStorage(true), isEncryptionAvailable:()=>false}],
  ['basic_text', fakeStorage(false)],
]) {
  test(`bank credential admission rejects ${name} storage`,()=>fixture(root=>{
    assert.equal(load(storage).store(root).osStoreUsable(),false);
    assert.equal(load(fakeStorage(true)).store(root).osStoreUsable(),true);
  }));
}

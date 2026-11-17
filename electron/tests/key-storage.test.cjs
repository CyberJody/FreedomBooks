const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const vm = require('node:vm');
const crypto = require('node:crypto');

function load(storage) {
  const sandbox = {module:{exports:{}},process,Buffer,require:name=>name==='electron'?{safeStorage:storage}:require(name)};
  vm.runInNewContext(fs.readFileSync(path.join(__dirname,'../main/keys.cjs'),'utf8'),sandbox);
  return sandbox.module.exports;
}
function fakeStorage(usable, key = crypto.randomBytes(32)) {
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
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'fb-key-storage-'));
  try { return fn(root); } finally { fs.rmSync(root,{recursive:true,force:true}); }
}
const plain=o=>JSON.parse(JSON.stringify(o));
const files=root=>fs.readdirSync(root).filter(n=>n.startsWith('connection-key')).sort();
test('connection keys are independent per install and 0600 without a keyring',()=>fixture(root=>{
  const api=load(fakeStorage(false));
  const first=api.store(path.join(root,'first')), second=api.store(path.join(root,'second'));
  const a=first.connectionKey(), b=second.connectionKey();
  assert.equal(a.state,'created');
  assert.notEqual(a.key,b.key);
  assert.deepEqual(plain(first.connectionKey()),{key:a.key,state:'ready',alternates:[]});
  assert.equal(fs.statSync(path.join(root,'first/connection-key.private')).mode & 0o777,0o600);
  assert.equal(fs.statSync(path.join(root,'first')).mode & 0o777,0o700);
  fs.chmodSync(path.join(root,'first/connection-key.private'),0o644);
  // A key file others could read is not used, and nothing replaces it.
  assert.deepEqual(plain(first.connectionKey()),{key:null,state:'unavailable',reason:'fallback'});
  assert.deepEqual(files(path.join(root,'first')),['connection-key.private']);
}));
test('OS-sealed connection keys migrate the fallback without a plaintext residue',()=>fixture(root=>{
  const fallback=load(fakeStorage(false)).store(root);
  const expected=fallback.connectionKey().key;
  const storage=fakeStorage(true), api=load(storage), sealed=api.store(root);
  assert.deepEqual(plain(sealed.connectionKey()),{key:expected,state:'ready',alternates:[]});
  assert.ok(!fs.existsSync(path.join(root,'connection-key.private')));
  const bytes=fs.readFileSync(path.join(root,'connection-key.sealed'));
  assert.ok(!bytes.includes(Buffer.from(expected)));
  assert.deepEqual(plain(api.store(root).connectionKey()),{key:expected,state:'ready',alternates:[]});
}));
test('a connection key sealed by another keyring is set aside and replaced once, never deleted',()=>fixture(root=>{
  const before=fakeStorage(true), original=load(before).store(root).connectionKey().key;
  const sealedBytes=fs.readFileSync(path.join(root,'connection-key.sealed'));
  // The reinstall: a new, working keyring that cannot open the old file.
  const after=fakeStorage(true), api=load(after);
  const replaced=api.store(root).connectionKey();
  assert.equal(replaced.state,'replaced');
  assert.match(replaced.key,/^[0-9a-f]{64}$/);
  assert.notEqual(replaced.key,original);
  assert.match(replaced.setAside,/^connection-key\.sealed\.unreadable-\d{8}T\d{6}Z$/);
  assert.deepEqual(fs.readFileSync(path.join(root,replaced.setAside)),sealedBytes,'the old sealed bytes are kept exactly');
  assert.equal(fs.statSync(path.join(root,replaced.setAside)).mode & 0o777,0o600);
  // Next launch on the same keyring: the new key is simply ready; nothing else is set aside.
  assert.deepEqual(plain(api.store(root).connectionKey()),{key:replaced.key,state:'ready',alternates:[]});
  assert.deepEqual(files(root),['connection-key.sealed',replaced.setAside]);
  // Proof: the old keyring could still read what was set aside.
  assert.equal(before.decryptString(fs.readFileSync(path.join(root,replaced.setAside))),original);
}));
test('a keyring unavailable for one launch writes nothing, then the original key works again',()=>fixture(root=>{
  const storage=fakeStorage(true);
  const original=load(storage).store(root).connectionKey().key;
  const sealedBytes=fs.readFileSync(path.join(root,'connection-key.sealed'));
  for (const gone of [
    {...storage,getSelectedStorageBackend:()=>'basic_text'},
    {...storage,isEncryptionAvailable:()=>false},
    // Reports usable, but cannot seal and open a fresh value: not proof of anything.
    {...storage,decryptString:()=>{throw new Error('Error while decrypting the ciphertext provided to safeStorage.decryptString.');}},
    {...storage,encryptString:()=>{throw new Error('Encryption is not available.');}},
  ]) {
    const store=load(gone).store(root);
    assert.deepEqual(plain(store.connectionKey()),{key:null,state:'unavailable',reason:'keyring'});
    assert.equal(store.keyringWorks(),false);
    assert.deepEqual(files(root),['connection-key.sealed'],'no replacement key, no plaintext file, nothing set aside');
    assert.deepEqual(fs.readFileSync(path.join(root,'connection-key.sealed')),sealedBytes);
  }
  assert.deepEqual(plain(load(storage).store(root).connectionKey()),{key:original,state:'ready',alternates:[]});
}));
test('an older version\'s replacement key never overwrites a readable sealed key',()=>fixture(root=>{
  const storage=fakeStorage(true);
  const original=load(storage).store(root).connectionKey().key;
  // What earlier versions left after one launch without a keyring.
  const older=crypto.randomBytes(32).toString('hex');
  load(storage).writePrivate(path.join(root,'connection-key.private'),older);
  // Kept aside, and offered for reading anything an older version saved under it.
  assert.deepEqual(plain(load(storage).store(root).connectionKey()),{key:original,state:'ready',alternates:[older]});
  const left=files(root);
  assert.equal(left.length,2);assert.equal(left[0],'connection-key.private.superseded-'+left[0].split('superseded-')[1]);
  assert.equal(left[1],'connection-key.sealed');
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
  // Only fb-app's "Use this as my books" makes another book the main one; the shell then follows it.
  store.remember(book);
  assert.equal(api.store(root).bookPath(),store.defaultBook(),'opening a copy saves nothing');
  fs.writeFileSync(path.join(root,'active-book.json'),JSON.stringify({path:book}));
  store.adoptMain(book); store.sealBook('synthetic-book-key');
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
test('a damaged or unreadable sealed book key is set aside and replaced, never thrown at a correct unlock',()=>fixture(root=>{
  const storage=fakeStorage(true), api=load(storage), store=api.store(root);
  const book=path.join(root,'books/default.fbbook');
  const aside=()=>fs.readdirSync(path.dirname(book)).filter(n=>n.startsWith('default.fbbook.os-key.unreadable-'));
  fs.mkdirSync(path.dirname(book),{recursive:true}); fs.writeFileSync(book,'synthetic');
  assert.equal(store.bookKeyStatus(false),'missing');
  assert.equal(store.bookKeyStatus(true),'ask');
  fs.writeFileSync(book+'.os-key',Buffer.from('synthetic garbage, not ciphertext'));
  assert.equal(store.bookKeyStatus(false),'unreadable');
  assert.throws(()=>store.readSealedKey(),{message:api.SEALED_UNREADABLE});
  assert.equal(store.sealBook('synthetic-book-key'),true);
  assert.equal(store.readSealedKey(),'synthetic-book-key');
  assert.equal(store.bookKeyStatus(false),'present');
  assert.equal(store.sealBook('synthetic-book-key'),false);
  assert.equal(aside().length,1);
  assert.equal(fs.readFileSync(path.join(path.dirname(book),aside()[0]),'utf8'),'synthetic garbage, not ciphertext');
  // Sealed by a keyring this computer no longer has: same plain answer, same repair.
  fs.writeFileSync(book+'.os-key',fakeStorage(true).encryptString('another computer'));
  assert.throws(()=>store.readSealedKey(),{message:api.SEALED_UNREADABLE});
  assert.equal(store.sealBook('synthetic-book-key'),true);
  assert.equal(api.store(root).readSealedKey(),'synthetic-book-key');
  assert.ok(aside().length>=1);
}));
test('without a working keyring the saved book key is neither replaced nor removed',()=>fixture(root=>{
  const storage=fakeStorage(true), book=path.join(root,'books/default.fbbook');
  fs.mkdirSync(path.dirname(book),{recursive:true}); fs.writeFileSync(book,'synthetic');
  load(storage).store(root).sealBook('synthetic-book-key');
  const bytes=fs.readFileSync(book+'.os-key');
  const gone=load({...storage,getSelectedStorageBackend:()=>'basic_text'}).store(root);
  assert.equal(gone.bookKeyStatus(false),'keyring');
  assert.equal(gone.sealBook('another key'),false);
  assert.deepEqual(fs.readFileSync(book+'.os-key'),bytes);
  assert.equal(load(storage).store(root).readSealedKey(),'synthetic-book-key');
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
    const native = {openProtectedBook:()=>{throw opening;}, protectionState:()=>JSON.stringify({known:false}), openBook:()=>{throw new Error('must not open without a key');}};
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
  const native = {openProtectedBook:()=>{throw wrongKey;}, protectionState:()=>JSON.stringify({known:false}), openBook:(book,key)=>{calls.push([book,key]);return JSON.stringify({home:'/legacy/home/'});}};
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

test('legacy opening refuses a saved key after recovery restores a password record', () => {
  const {openLegacyBook} = load(fakeStorage(true));
  const opening = Object.assign(new Error('wrong book password'), {code:'FB_BOOK_WRONG_KEY'});
  let known = false, reads = 0, savedOpens = 0;
  const local = {bookPath:()=>'/synthetic/default.fbbook', readSealedKey:()=>{reads++;return 'stale saved key';}};
  const native = {
    // The protected-open call settles the interrupted move and restores the key record first.
    openProtectedBook() { known = true; throw opening; },
    protectionState:()=>JSON.stringify({known, builtin:false, ask_password:true}),
    openBook() { savedOpens++; return JSON.stringify({home:'/should-not-open/'}); },
  };
  assert.equal(JSON.parse(native.protectionState()).known, false, 'the caller initially saw no record');
  assert.throws(()=>openLegacyBook(native, local, 'wrong password'), error=>error===opening);
  assert.equal(reads, 0, 'no stale saved key is read');
  assert.equal(savedOpens, 0, 'the recovered password record remains authoritative');
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

test('setting aside never deletes the only copy, keeps one copy of identical bytes, and stops at the limit',()=>fixture(root=>{
  const api=load(fakeStorage(true)), file=path.join(root,'connection-key.sealed');
  const aside=()=>fs.readdirSync(root).filter(n=>n.startsWith('connection-key.sealed.unreadable-'));
  fs.writeFileSync(file,'synthetic one');
  const first=api.setAside(file);
  assert.ok(!fs.existsSync(file) && fs.readFileSync(first,'utf8')==='synthetic one');
  // The same bytes again: only unlinked, because an identical copy is already kept.
  fs.writeFileSync(file,'synthetic one');
  assert.equal(api.setAside(file),first);
  assert.equal(aside().length,1);
  // Distinct bytes up to the limit; past it nothing is moved.
  for(let i=2;i<=api.SET_ASIDE_LIMIT;i++){ fs.writeFileSync(file,`synthetic ${i}`); api.setAside(file); }
  assert.equal(aside().length,api.SET_ASIDE_LIMIT);
  fs.writeFileSync(file,'synthetic over the limit');
  assert.throws(()=>api.setAside(file),{code:'FB_SET_ASIDE_LIMIT'});
  assert.equal(fs.readFileSync(file,'utf8'),'synthetic over the limit','refused: the file stays where it is');
  // keep=true adds a second name and leaves the file in place.
  fs.writeFileSync(file,'synthetic one');
  assert.equal(api.setAside(file,'unreadable',true),first);
  assert.ok(fs.existsSync(file));
}));

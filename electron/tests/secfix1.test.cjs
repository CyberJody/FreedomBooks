'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const Module = require('node:module');
const original = Module._load;
const safeStorage = {
  isEncryptionAvailable: () => true,
  getSelectedStorageBackend: () => 'gnome_libsecret',
  encryptString: s => Buffer.from(s),
  decryptString: b => b.toString(),
};
Module._load = function(name, ...args) {
  return name === 'electron' ? {safeStorage} : original.call(this, name, ...args);
};
const keys = require('../main/keys.cjs');
const guards = require('../main/guards.cjs');
const dev = require('../main/development.cjs');

test('packaged builds ignore every shell development override', () => {
  const env = {FB_NATIVE_MODULE:'/untrusted.node', FB_DESKTOP_USER_DATA:'/untrusted', FB_DEVTOOLS:'1'};
  assert.equal(dev.nativeModule({isPackaged:true},'/bundle/main',env),'/bundle/native/fb-node.node');
  assert.equal(dev.userData({isPackaged:true},env),undefined);
  assert.equal(dev.devTools({isPackaged:true},env),false);
  assert.equal(dev.nativeModule({isPackaged:false},'/bundle/main',env),env.FB_NATIVE_MODULE);
  assert.equal(dev.userData({isPackaged:false},env),env.FB_DESKTOP_USER_DATA);
  assert.equal(dev.devTools({isPackaged:false},env),true);
});

test('password wrapping never leaves an OS-sealed bypass; legacy copy is removed on unlock', () => {
  const root=fs.mkdtempSync(path.join(process.env.TMPDIR || os.tmpdir(),'fb-secfix1-keys-'));
  try {
    const store=keys.store(root);
    store.saveKeys('synthetic-key',null);
    assert.equal(store.readSealedKey(),'synthetic-key');
    store.saveKeys('synthetic-key','synthetic-wrap');
    assert.equal(store.readWrap(),'synthetic-wrap');
    assert.equal(fs.existsSync(path.join(root,'book-key.sealed')),false);
    fs.writeFileSync(path.join(root,'book-key.sealed'),'legacy');
    store.removeRedundantSealedKey();
    assert.equal(fs.existsSync(path.join(root,'book-key.sealed')),false);
    assert.equal(fs.statSync(path.join(root,'book-key.lock.json')).mode & 0o777,0o600);
  } finally {fs.rmSync(root,{recursive:true,force:true});}
});

test('every historical mutation type is refused at the renderer guard', () => {
  for (const type of guards.appCommands) {
    if (!guards.rendererCommands.includes(type)) assert.throws(()=>guards.command({type}),/Unknown command/,type);
  }
  assert.deepEqual(guards.rendererCommands,['SubmitForm','ExportHistory','ExportReport','ExportBackup']);
  assert.equal(guards.command({type:'SubmitForm',path:'/a/',token:'t',fields:[]}).type,'SubmitForm');
  assert.doesNotMatch(fs.readFileSync(path.join(__dirname,'../preload/preload.cjs'),'utf8'),/exposeInMainWorld/);
});

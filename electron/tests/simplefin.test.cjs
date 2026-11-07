'use strict';
const {test} = require('node:test');
const assert = require('node:assert/strict');
const guards = require('../main/guards.cjs');
const id = '0195b4a4-55af-719e-96cb-a2f946b067bf';
const form = path => ({type:'SubmitForm',path,token:'test',fields:[]});

test('SimpleFIN-only capability admits bank forms and refuses AI forms', () => {
  const capabilities = {plugins:false,bankFeeds:true};
  for (const path of [`/c/${id}/bank-feeds/connect/`, `/c/${id}/bank-feeds/${id}/sync/`, `/c/${id}/bank-feeds/review/`]) {
    assert.equal(guards.command(form(path), capabilities).path, path);
    assert.throws(() => guards.command(form(path)), /Unknown command/);
  }
  for (const path of [`/c/${id}/ai/`, `/c/${id}/ai/receipt/upload/`, '/auth/openai/start']) {
    assert.throws(() => guards.command(form(path), capabilities), /Unknown command/);
  }
});

test('SimpleFIN setup opens only the exact public provider page', () => {
  const url = 'https://bridge.simplefin.org/simplefin/create';
  assert.equal(guards.external({url}), url);
  for (const other of [url+'?token=secret',url+'#token',url+'/',url.replace('bridge.simplefin.org','bridge.simplefin.org.evil.test'),url.replace('https://','http://'),url.replace('https://','https://user:secret@')]) {
    assert.throws(() => guards.external({url:other}), /link not allowed/);
  }
});

test('packaged helper resolves to executable unpacked resources and ignores overrides', () => {
  const development=require('../main/development.cjs');
  const path=require('node:path');
  const app={isPackaged:true,getAppPath:()=>'/synthetic/resources/app.asar'};
  const name=process.platform==='win32'?'fb-net.exe':'fb-net';
  assert.equal(development.netHelper(app,'/synthetic/resources/app.asar/main',{FB_NET_HELPER:'/wrong/helper'}),path.join(app.getAppPath()+'.unpacked','native',name));
});

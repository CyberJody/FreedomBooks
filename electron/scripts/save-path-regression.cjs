'use strict';
// Exercise the reviewed controller unchanged. Every destructive candidate is a review-only sentinel.
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const out = process.env.FB_SMOKE_SHOTS;
fs.mkdirSync(path.join(out,'tmp'),{recursive:true});
const dir = fs.mkdtempSync(path.join(out, 'tmp/save-path-'));
const book = path.join(dir, 'books/default.fbbook');
fs.mkdirSync(path.dirname(book));
const url = 'fb://app/c/0195b4a4-55af-719e-96cb-a2f946b067bf/invoices/0195b4a4-55af-719e-96cb-a2f946b067c0/print/';
let selected, options;
const native = {appState:()=>JSON.stringify({open:true,path:book}),render:()=>JSON.stringify({status:200,html:'<button id="print-now">'})};
const electron = {dialog:{showSaveDialog:async(_win,opts)=>{options=opts;return {canceled:false,filePath:selected};}},Menu:{}};
const sandbox = {URL, module:{exports:{}},require:name=>name==='electron'?electron:require('node:module').createRequire(path.resolve(__dirname,'../main/printing.cjs'))(name)};
const source = path.resolve(__dirname,'../main/printing.cjs');
const localRequire = require('node:module').createRequire(source);
vm.runInNewContext(fs.readFileSync(source,'utf8'), sandbox, {filename:source});
const controller = sandbox.module.exports.controller(native);
const win = {isDestroyed:()=>false,webContents:{getURL:()=>url,getTitle:()=> '../../books/default.fbbook',printToPDF:async()=>Buffer.from('%PDF-1.7\nreview sentinel\n')}};
(async()=>{
  const result=[];
  for (const kind of ['exact-book','pdf-symlink-to-book','book-directory']) {
    fs.writeFileSync(book,'SYNTHETIC BOOK SENTINEL');
    selected = kind==='exact-book' ? book : kind==='book-directory' ? path.dirname(book) : path.join(dir,'invoice.pdf');
    if (kind==='pdf-symlink-to-book') fs.symlinkSync(book,selected);
    let error=null;
    try {await controller.perform(win,'pdf',new URL(url).pathname);}catch(e){error=String(e.message);}
    assert.ok(error,kind+' must refuse');
    assert.equal(fs.readFileSync(book,'utf8'),'SYNTHETIC BOOK SENTINEL',kind+' book unchanged');
    result.push({kind, error, sentinelPreserved:fs.readFileSync(book,'utf8')==='SYNTHETIC BOOK SENTINEL',defaultPath:options.defaultPath});
  }
  assert.equal(options.defaultPath.includes('/'),false,'renderer title traversal must not survive');
  fs.writeFileSync(path.join(out,'save-path-results.json'),JSON.stringify(result,null,2)+'\n');
  console.log(JSON.stringify(result,null,2));
  fs.rmSync(dir,{recursive:true});
})().catch(e=>{console.error(e);process.exitCode=1;});

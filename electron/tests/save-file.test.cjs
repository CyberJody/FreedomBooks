'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const vm = require('node:vm');
const {create, MESSAGE} = require('../main/save-file.cjs');

function fixture(t) {
  const root = fs.mkdtempSync(path.join(process.env.TMPDIR || os.tmpdir(), 'fb-save-test-'));
  t.after(()=>fs.rmSync(root,{recursive:true,force:true}));
  const dataDir=path.join(root,'data');
  const book=path.join(dataDir,'books/default.fbbook');
  const output=path.join(root,'output');
  fs.mkdirSync(path.dirname(book),{recursive:true});fs.mkdirSync(output);
  fs.writeFileSync(book,'SYNTHETIC BOOK');
  const saver=create({dataDir,bookPath:()=>book});
  return {root,dataDir,book,output,saver};
}
const rejected = fn => assert.throws(fn,error=>error.message===MESSAGE);

test('new PDF/CSV and confirmed regular-file replacements are private atomic saves',t=>{
  const {output,saver}=fixture(t);
  for(const extension of ['pdf','csv','zip']) {
    const file=path.join(output,'export.'+extension);
    saver.write(file,'first');
    assert.equal(fs.readFileSync(file,'utf8'),'first');
    const before=fs.statSync(file);
    fs.chmodSync(file,0o644);
    saver.write(file,'second');
    assert.equal(fs.readFileSync(file,'utf8'),'second');
    assert.notEqual(fs.statSync(file).ino,before.ino,'replacement must use a new inode');
    assert.equal(fs.statSync(file).mode&0o777,0o600);
  }
  assert.deepEqual(fs.readdirSync(output).sort(),['export.csv','export.pdf','export.zip']);
});

test('book, sidecars, keys, all data files and new names under the data root refuse',t=>{
  const {dataDir,book,saver}=fixture(t);
  for(const file of [book,book+'-wal',book+'-shm',path.join(dataDir,'book-key.lock.json'),path.join(dataDir,'book-key.sealed'),path.join(dataDir,'innocent.pdf')]) {
    if(file!==book)fs.writeFileSync(file,'SENTINEL');
    const before=fs.readFileSync(file);
    rejected(()=>saver.write(file,'BAD'));
    assert.deepEqual(fs.readFileSync(file),before);
  }
  rejected(()=>saver.write(path.join(dataDir,'new.csv'),'BAD'));
  assert.equal(fs.existsSync(path.join(dataDir,'new.csv')),false);
});

test('arbitrary .fbbook* names and open books without an extension refuse anywhere',t=>{
  const {root,output,saver}=fixture(t);
  for(const name of ['other.fbbook','other.fbbook-wal','other.FBBOOK-shm','other.fbbook.key','other.fbbook.pdf']) {
    rejected(()=>saver.write(path.join(output,name),'BAD'));
    assert.equal(fs.existsSync(path.join(output,name)),false);
  }
  const book=path.join(root,'legacy-book');fs.writeFileSync(book,'BOOK');
  const external=create({bookPath:()=>book});
  for(const file of [book,book+'-wal',book+'-shm',book+'.key',book+'-key'])rejected(()=>external.write(file,'BAD'));
  assert.equal(fs.readFileSync(book,'utf8'),'BOOK');
});

test('leaf links, dangling links, parent aliases and a symlinked data root refuse',t=>{
  const {root,dataDir,book,output,saver}=fixture(t);
  const link=path.join(output,'invoice.pdf');fs.symlinkSync(book,link);
  rejected(()=>saver.write(link,'BAD'));
  assert.equal(fs.readFileSync(book,'utf8'),'SYNTHETIC BOOK');
  const dangling=path.join(output,'missing.pdf');fs.symlinkSync(path.join(output,'missing-target'),dangling);
  rejected(()=>saver.write(dangling,'BAD'));
  const alias=path.join(root,'alias');fs.symlinkSync(dataDir,alias);
  rejected(()=>saver.write(path.join(alias,'new.csv'),'BAD'));
  const aliasSaver=create({dataDir:alias});
  rejected(()=>aliasSaver.write(path.join(dataDir,'new.pdf'),'BAD'));
  // Ordinary parent symlinks outside protected data remain usable.
  const downloads=path.join(root,'downloads');fs.symlinkSync(output,downloads);
  saver.write(path.join(downloads,'normal.pdf'),'PDF');
  assert.equal(fs.readFileSync(path.join(output,'normal.pdf'),'utf8'),'PDF');
});

test('default XDG FreedomBooks folder and its canonical alias are protected',t=>{
  const {root,output,saver}=fixture(t);
  const before=process.env.XDG_DATA_HOME;
  process.env.XDG_DATA_HOME=path.join(root,'xdg');
  t.after(()=>{if(before===undefined)delete process.env.XDG_DATA_HOME;else process.env.XDG_DATA_HOME=before;});
  fs.mkdirSync(process.env.XDG_DATA_HOME);
  fs.symlinkSync(output,path.join(process.env.XDG_DATA_HOME,'FreedomBooks'));
  rejected(()=>saver.write(path.join(output,'alias.pdf'),'BAD'));
  rejected(()=>saver.write(path.join(process.env.XDG_DATA_HOME,'FreedomBooks/new.csv'),'BAD'));
});

test('hard links to open book, non-files and malformed destinations refuse',t=>{
  const {book,output,saver}=fixture(t);
  const alias=path.join(output,'invoice.pdf');fs.linkSync(book,alias);
  rejected(()=>saver.write(alias,'BAD'));
  for(const file of [output,'relative.pdf',null])rejected(()=>saver.write(file,'BAD'));
  assert.equal(fs.readFileSync(book,'utf8'),'SYNTHETIC BOOK');
});

test('hard links to the key files in the data directory or beside the book refuse from outside it',t=>{
  const {dataDir,book,output,saver}=fixture(t);
  const besideBook=['.keys.json','.os-key','.rekey.json','.rekey-before'].map(suffix=>path.basename(book)+suffix);
  for(const name of ['book-key.sealed','book-key.lock.json','connection-key.sealed','connection-key.private',...besideBook]) {
    const key=path.join(name.startsWith('default.fbbook')?path.dirname(book):dataDir,name);fs.writeFileSync(key,'SYNTHETIC KEY');
    const alias=path.join(output,name.replace('default.fbbook','alias')+'.pdf');fs.linkSync(key,alias);
    rejected(()=>saver.write(alias,'BAD'));
    assert.equal(fs.readFileSync(alias,'utf8'),'SYNTHETIC KEY');
  }
});

function instrument(overrides, platform = process.platform) {
  const source=path.join(__dirname,'../main/save-file.cjs');
  const sandbox={process:{platform,env:process.env},Buffer,module:{exports:{}},require:name=>name==='node:fs'?{...fs,...overrides}:require(name)};
  vm.runInNewContext(fs.readFileSync(source,'utf8'),sandbox,{filename:source});
  return sandbox.module.exports;
}

test('raced leaf symlink is refused after temp fsync without touching the book',t=>{
  const {book,output,dataDir}=fixture(t);
  const selected=path.join(output,'invoice.pdf');
  let fired=false;
  const module=instrument({fsyncSync:fd=>{fs.fsyncSync(fd);if(!fired){fired=true;fs.symlinkSync(book,selected);}}});
  rejected(()=>module.create({dataDir,bookPath:()=>book}).write(selected,'BAD'));
  assert.equal(fs.readFileSync(book,'utf8'),'SYNTHETIC BOOK');
  assert.deepEqual(fs.readdirSync(output),['invoice.pdf'],'temp is cleaned up');
});

test('rename never follows a destination symlink introduced after the last validation',t=>{
  const {book,output,dataDir}=fixture(t);
  const selected=path.join(output,'invoice.pdf');
  const module=instrument({renameSync:(src,dst)=>{fs.symlinkSync(book,selected);fs.renameSync(src,dst);}});
  module.create({dataDir,bookPath:()=>book}).write(selected,'PDF');
  assert.equal(fs.readFileSync(book,'utf8'),'SYNTHETIC BOOK');
  assert.equal(fs.lstatSync(selected).isSymbolicLink(),false);
  assert.equal(fs.readFileSync(selected,'utf8'),'PDF');
});

test('a raced parent symlink cannot redirect output into data',t=>{
  const {root,book,output,dataDir}=fixture(t);
  const moved=path.join(root,'moved');
  let fired=false;
  const module=instrument({writeFileSync:(...args)=>{
    if(!fired){fired=true;fs.renameSync(output,moved);fs.symlinkSync(dataDir,output);}
    fs.writeFileSync(...args);
  }});
  rejected(()=>module.create({dataDir,bookPath:()=>book}).write(path.join(output,'invoice.pdf'),'BAD'));
  assert.equal(fs.readFileSync(book,'utf8'),'SYNTHETIC BOOK');
  assert.equal(fs.existsSync(path.join(dataDir,'invoice.pdf')),false);
  assert.deepEqual(fs.readdirSync(moved),[],'temp cleaned in pinned directory');
});

test('exclusive temp creation and fsync failures preserve the existing destination',t=>{
  const {book,output,dataDir}=fixture(t);
  const selected=path.join(output,'invoice.pdf');fs.writeFileSync(selected,'ORIGINAL PDF');
  for(const failure of ['collision','fsync','write','rename']) {
    const overrides={};let collision;
    if(failure==='collision')overrides.openSync=(file,flags,...rest)=>{
      if(flags==='wx'){assert.equal(rest[0],0o600);fs.symlinkSync(book,file);collision=path.join(output,path.basename(file));}
      return fs.openSync(file,flags,...rest);
    };
    else overrides[failure==='fsync'?'fsyncSync':failure==='write'?'writeFileSync':'renameSync']=()=>{throw new Error('injected '+failure);};
    assert.throws(()=>instrument(overrides).create({dataDir,bookPath:()=>book}).write(selected,'BAD'));
    assert.equal(fs.readFileSync(selected,'utf8'),'ORIGINAL PDF');
    assert.equal(fs.readFileSync(book,'utf8'),'SYNTHETIC BOOK');
    if(collision)fs.unlinkSync(collision);
    assert.deepEqual(fs.readdirSync(output),['invoice.pdf']);
  }
});


test('Windows fallback replaces regular output without opening or fsyncing a directory',t=>{
  const {book,output,dataDir}=fixture(t);
  const selected=path.join(output,'invoice.pdf');
  const module=instrument({
    openSync:(file,flags,...rest)=>{assert.equal(flags,'wx');return fs.openSync(file,flags,...rest);},
    fsyncSync:fd=>{assert.equal(fs.fstatSync(fd).isFile(),true);fs.fsyncSync(fd);},
  },'win32');
  for(const content of ['NEW PDF','REPLACEMENT PDF']) {
    module.create({dataDir,bookPath:()=>book}).write(selected,content);
    assert.equal(fs.readFileSync(selected,'utf8'),content);
  }
});


test('parent symlink followed by dot-dot resolves before protected-location checks',t=>{
  const {dataDir,output,saver}=fixture(t);
  const key=path.join(dataDir,'book-key.sealed');fs.writeFileSync(key,'SYNTHETIC KEY');
  const alias=path.join(output,'alias');fs.symlinkSync(path.join(dataDir,'books'),alias);
  rejected(()=>saver.write(`${alias}/../book-key.sealed`,'BAD'));
  assert.equal(fs.readFileSync(key,'utf8'),'SYNTHETIC KEY');
  assert.equal(fs.existsSync(path.join(output,'book-key.sealed')),false,'never save to a different lexical location');
  const previous=process.env.XDG_DATA_HOME;
  process.env.XDG_DATA_HOME=`${alias}/..`;
  t.after(()=>{if(previous===undefined)delete process.env.XDG_DATA_HOME;else process.env.XDG_DATA_HOME=previous;});
  const xdg=path.join(dataDir,'FreedomBooks');fs.mkdirSync(xdg);
  rejected(()=>create().write(path.join(xdg,'new.csv'),'BAD'));
  assert.equal(fs.existsSync(path.join(xdg,'new.csv')),false,'XDG root dot-dot preserves filesystem semantics too');
});

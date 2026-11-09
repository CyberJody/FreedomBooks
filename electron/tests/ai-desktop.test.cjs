'use strict';
const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const os=require('node:os');
const path=require('node:path');
const {create}=require('../main/ai-desktop.cjs');
const id='12345678-1234-4234-8234-123456789abc';
const company='aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
const token='b'.repeat(64);
function fixture({usable=true,changeSession=false}={}) {
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'fb-ai-controls-'));
  let copied='',encrypted='',generation='1',grantCount=0,revoked=0,questions=0;
  const messages=[];
  const dialog={showMessageBox:async(_win,options)=>{
    messages.push(options);
    if(options.title==='Connect an AI assistant') {questions++;if(changeSession)generation='2';return {response:2};}
    if(options.title==='AI assistant connected')return {response:1};
    if(options.title==='AI assistant connections')return {response:1};
    return {response:0};
  }};
  const native={appState:()=>'{"open":true}',aiGeneration:()=>generation,
    aiGrant:(co,name,permission)=>{grantCount++;assert.equal(co,company);assert.equal(permission,'accounting_write');return JSON.stringify({id,token});},
    aiConnections:()=>JSON.stringify([{id,name:'Test agent',company_id:company,actor_id:'operator',permission:'accounting_write'}]),
    aiInvalidate:()=>revoked++,aiRevoke:()=>revoked++,aiRequestAsync:()=>Promise.resolve('{}')};
  const controller=create({app:{isPackaged:false},native,dialog,clipboard:{writeText:text=>copied=text},safeStorage:{isEncryptionAvailable:()=>usable,getSelectedStorageBackend:()=>usable?'gnome_libsecret':'basic_text',encryptString:text=>{encrypted=text;return Buffer.from('OS-sealed synthetic ciphertext');}},root,window:()=>({webContents:{getURL:()=>`fb://app/c/${company}/home/`}}),exclusive:work=>work(),busy:()=>false});
  return {controller,root,messages,state:()=>({copied,encrypted,grantCount,revoked,questions})};
}
test('native setup offers clear scopes and copies only secret-free launcher configuration; revoke removes grant files',async()=>{
  const f=fixture();
  try {
    await f.controller.connect();
    const s=f.state();assert.equal(s.grantCount,1);assert.equal(s.questions,1);assert.equal(JSON.parse(s.encrypted).token,token);
    assert.ok(!s.copied.includes(token));assert.ok(!JSON.stringify(f.messages).includes(token));
    const config=JSON.parse(s.copied);assert.ok(config.mcpServers.freedombooks.args.includes('--ai-mcp'));
    const file=path.join(f.root,'ai-connections',id+'.sealed');
    assert.equal(fs.statSync(file).mode&0o777,0o600);assert.ok(!fs.readFileSync(file).includes(Buffer.from(token)));
    await f.controller.manage();assert.equal(f.state().revoked,1);assert.equal(fs.existsSync(file),false);
  } finally {await f.controller.stop();fs.rmSync(f.root,{recursive:true});}
});
test('unavailable OS store refuses setup; session changes during permission dialog cannot grant stale authority',async()=>{
  for(const options of [{usable:false},{changeSession:true}]) {
    const f=fixture(options);
    try {await f.controller.connect();assert.equal(f.state().grantCount,0);assert.equal(f.state().copied,'');} finally {await f.controller.stop();fs.rmSync(f.root,{recursive:true});}
  }
});

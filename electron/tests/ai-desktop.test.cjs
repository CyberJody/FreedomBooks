'use strict';
const {test}=require('node:test'),assert=require('node:assert/strict');
const fs=require('node:fs'),os=require('node:os'),path=require('node:path'),crypto=require('node:crypto');
const {create}=require('../main/ai-desktop.cjs'),approval=require('../main/ai-approval.cjs');
const {api}=require('../agent/mcp.cjs');
const company='aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',actor='bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';
function fixture({usable=true,changeSession=false,choice=2,page=null}={}) {
 const root=fs.mkdtempSync(path.join(os.tmpdir(),'fb-ai-controls-')),book=path.join(root,'synthetic.fbbook');fs.writeFileSync(book,'Synthetic scope marker');
 const key=crypto.randomBytes(32),messages=[],grants=new Map();let copied='',generation=1,questions=0,grantCount=0,revoked=0,signedIn=true,view=company,role='administrator',who=actor,restored=false;
 const storage={isEncryptionAvailable:()=>usable,getSelectedStorageBackend:()=>usable?'gnome_libsecret':'basic_text'};
 storage.encryptString=text=>{const nonce=crypto.randomBytes(12),c=crypto.createCipheriv('aes-256-gcm',key,nonce),bytes=Buffer.concat([c.update(text),c.final()]);return Buffer.concat([nonce,c.getAuthTag(),bytes]);};
 storage.decryptString=bytes=>{const c=crypto.createDecipheriv('aes-256-gcm',key,bytes.subarray(0,12));c.setAuthTag(bytes.subarray(12,28));return Buffer.concat([c.update(bytes.subarray(28)),c.final()]).toString();};
 const invalidate=()=>{generation++;grants.clear();};
 const dialog={showMessageBox:async(_win,o)=>{messages.push(o);if(o.title==='Connect an AI assistant'){questions++;if(changeSession)invalidate();return {response:choice};}return {response:1};}};
 const native={appState:()=>JSON.stringify({open:true,signed_in:signedIn}),aiGeneration:()=>String(generation),
  aiCurrentCompany:()=>signedIn?view:null,
  aiContext:co=>{assert.ok(signedIn);assert.equal(co,view);return JSON.stringify({company_id:co,company_name:'Synthetic company',actor_id:who,actor_name:'Synthetic operator',role,book_path:book,restored,session_generation:generation});},
  aiGrant:(co,name,permission)=>{assert.ok(signedIn);assert.equal(co,view);assert.ok(permission==='read_only'||role!=='readonly');grantCount++;const g={id:crypto.randomUUID(),token:crypto.randomBytes(32).toString('hex'),company_id:co,actor_id:who,name,permission};grants.set(g.id,g);return JSON.stringify(g);},
  aiConnections:()=>JSON.stringify([...grants.values()].map(({token,...g})=>g)),aiInvalidate:()=>{revoked++;invalidate();},aiRevoke:id=>{revoked++;grants.delete(id);},
  aiRequestAsync:async token=>{assert.ok(signedIn);const g=[...grants.values()].find(g=>g.token===token);assert.ok(g);assert.equal(g.actor_id,who);assert.equal(g.company_id,view);return JSON.stringify({company_id:view,permission:g.permission});}};
 const controller=()=>create({app:{isPackaged:false},native,dialog,clipboard:{writeText:text=>copied=text},safeStorage:storage,root,window:()=>({webContents:{getURL:()=>signedIn?`fb://app${page || `/c/${view}/home/`}`:'fb://app/desktop/sign-in/'}}),exclusive:work=>work(),busy:()=>false});
 return {controller:controller(),restart:controller,root,book,storage,native,messages,invalidate,
  change:(values)=>{({signedIn= signedIn,view=view,role=role,who=who,restored=restored}=values);invalidate();},
  state:()=>({copied,grantCount,revoked,questions}),files:()=>fs.readdirSync(path.join(root,'ai-connections')).filter(n=>approval.UUID.test(n.replace(/\.sealed$/,''))&&n.endsWith('.sealed')).map(n=>path.join(root,'ai-connections',n))};
}
async function cleanup(f,extra){await extra?.stop();await f.controller.stop();fs.rmSync(f.root,{recursive:true});}
test('global book pages connect the authorized selected company without a company ID in the URL',async()=>{
 for(const page of ['/help/report/','/desktop/users/','/desktop/mfa/','/desktop/session/','/settings/book-protection/']){
  const f=fixture({page});try{
   await f.controller.connect();assert.equal(f.state().grantCount,1,page);
   assert.equal(approval.read(f.files()[0],f.storage).company_id,company);
   assert.ok(!f.messages.some(m=>m.type==='error'),page);
  }finally{await cleanup(f);}
 }
});
test('global book pages cannot invent a selection or reconnect a signed-out user',async()=>{
 for(const values of [{view:null},{signedIn:false}]){
  const f=fixture({page:'/help/report/'});try{
   f.change(values);await f.controller.connect();assert.equal(f.state().grantCount,0);
   assert.equal(f.state().questions,0);assert.equal(f.state().copied,'');
   assert.equal(f.messages.at(-1).detail,'Sign in and open a company before connecting an AI.');
  }finally{await cleanup(f);}
 }
});
test('setup remembers exact company/user/book scope and copies only secret-free configuration; forgetting revokes and removes approvals',async()=>{
 const f=fixture();try{
  await f.controller.connect();const file=f.files()[0],r=approval.read(file,f.storage);
  assert.equal(r.company_id,company);assert.equal(r.actor_id,actor);assert.equal(r.permission,'accounting_write');assert.equal(r.book_scope,approval.scope(f.book));assert.equal(fs.statSync(file).mode&0o777,0o600);
  assert.ok(!fs.readFileSync(file).includes(Buffer.from(r.token)));assert.ok(!f.state().copied.includes(r.token));assert.ok(!JSON.stringify(f.messages).includes(r.token));
  assert.ok(JSON.parse(f.state().copied).mcpServers.freedombooks.args.includes('--ai-mcp'));
  await f.controller.connect();assert.equal(f.state().questions,1,'connecting again reuses the existing approval');
  await f.controller.manage();assert.equal(f.files().length,0);assert.equal(f.state().revoked,1);
  f.invalidate();await f.controller.resume(company);assert.equal(f.state().grantCount,1);
 }finally{await cleanup(f);}
});
test('normal restart rotates endpoint and token in the same sealed file without asking for permission again',async()=>{
 const f=fixture();let restarted;try{
  await f.controller.connect();const file=f.files()[0],old=approval.read(file,f.storage);await f.controller.stop();f.invalidate();restarted=f.restart();
  await restarted.resume(company);const now=approval.read(file,f.storage);
  assert.equal(now.id,old.id);assert.equal(now.approved_at,old.approved_at);assert.notEqual(now.token,old.token);assert.notEqual(now.endpoint,old.endpoint);assert.equal(f.state().questions,1);
  assert.equal((await api(()=>approval.read(file,f.storage),'/v1/catalogue')).company_id,company);
  await assert.rejects(api({...now,token:old.token},'/v1/catalogue'));
  await restarted.resume(company);assert.equal(f.state().grantCount,2,'one live grant for repeated renders');
 }finally{await cleanup(f,restarted);}
});
test('sign-out/MFA challenge, another user or company, restored preview and readonly downgrade cannot restore write approval',async()=>{
 for(const values of [{signedIn:false},{who:'cccccccc-cccc-4ccc-8ccc-cccccccccccc'},{view:'dddddddd-dddd-4ddd-8ddd-dddddddddddd'},{restored:true},{role:'readonly'}]){
  const f=fixture();try{await f.controller.connect();f.change(values);await f.controller.resume(values.view||company);assert.equal(f.state().grantCount,1);assert.equal(f.state().questions,1);}finally{await cleanup(f);}
 }
 const f=fixture();try{await f.controller.connect();const file=f.files()[0],old=approval.read(file,f.storage);f.change({signedIn:false});await assert.rejects(api(old,'/v1/catalogue'));f.change({signedIn:true});await f.controller.resume(company);assert.equal(f.state().grantCount,2);assert.equal(f.state().questions,1);}finally{await cleanup(f);}
});
test('copying/replacing the book does not inherit approval even with identical company and actor IDs',async()=>{
 const f=fixture();try{await f.controller.connect();fs.renameSync(f.book,f.book+'.retained');fs.writeFileSync(f.book,'Different synthetic book');f.invalidate();await f.controller.resume(company);assert.equal(f.state().grantCount,1);}finally{await cleanup(f);}
});
test('read-only approval stays read-only after restart and role changes',async()=>{
 const f=fixture({choice:1});try{await f.controller.connect();f.change({role:'administrator'});await f.controller.resume(company);assert.equal(approval.read(f.files()[0],f.storage).permission,'read_only');assert.equal(f.state().questions,1);}finally{await cleanup(f);}
});
test('unavailable keyring and changing sessions during setup or resume refuse stale authority',async()=>{
 for(const options of [{usable:false},{changeSession:true}]){const f=fixture(options);try{await f.controller.connect();assert.equal(f.state().grantCount,0);assert.equal(f.state().copied,'');if(options.usable===false)assert.equal(f.messages.at(-1).detail,'Unlock your desktop keyring before connecting an AI.');}finally{await cleanup(f);}}
 const f=fixture();try{await f.controller.connect();f.invalidate();await f.controller.stop();const p=f.controller.resume(company);f.invalidate();await p;assert.equal(f.state().grantCount,1);}finally{await cleanup(f);}
});
test('legacy session-only files, public files, symlinks and damaged ciphertext never become remembered approvals',async()=>{
 for(const mode of ['legacy','public','symlink','damaged']){
  const f=fixture();try{await f.controller.connect();const file=f.files()[0],r=approval.read(file,f.storage);f.invalidate();
   if(mode==='legacy')fs.writeFileSync(file,f.storage.encryptString(JSON.stringify({endpoint:r.endpoint,token:r.token})));
   if(mode==='public')fs.chmodSync(file,0o644);
   if(mode==='damaged')fs.writeFileSync(file,Buffer.from('invalid'));
   if(mode==='symlink'){fs.renameSync(file,file+'.retained');fs.symlinkSync(file+'.retained',file);}
   await f.controller.resume(company);assert.equal(f.state().grantCount,1);assert.equal(f.state().questions,1);
  }finally{await cleanup(f);}
 }
});
test('forgetting is durable against replayed approval files and removes more than 33 saved files',async()=>{
 const f=fixture();let restarted;try{
  await f.controller.connect();const file=f.files()[0],saved=fs.readFileSync(file),epoch=approval.epoch(f.root,f.storage);
  for(let i=0;i<40;i++)fs.writeFileSync(path.join(f.root,'ai-connections',crypto.randomUUID()+'.sealed'),'retired legacy file',{mode:0o600});
  await f.controller.manage();assert.equal(f.files().length,0);assert.notEqual(approval.epoch(f.root,f.storage),epoch);
  fs.writeFileSync(file,saved,{mode:0o600});await f.controller.stop();f.invalidate();restarted=f.restart();await restarted.resume(company);assert.equal(f.state().grantCount,1,'restored revoked file does not mint new authority');
 }finally{await cleanup(f,restarted);}
});
test('a failed durable Forget remains paused instead of reconnecting on the next company render',async()=>{
 const f=fixture();try{
  await f.controller.connect();const encrypt=f.storage.encryptString;f.storage.encryptString=()=>{throw Error('Synthetic keyring locked');};
  await f.controller.manage();f.storage.encryptString=encrypt;
  await f.controller.resume(company);assert.equal(f.state().grantCount,1);assert.ok(f.messages.some(m=>m.message==='Access stopped, but forgetting approval could not be saved'));
 }finally{await cleanup(f);}
});

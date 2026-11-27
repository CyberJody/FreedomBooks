'use strict';
const {test}=require('node:test'),assert=require('node:assert/strict');
const fs=require('node:fs'),os=require('node:os'),path=require('node:path'),crypto=require('node:crypto');
const {create}=require('../main/ai-desktop.cjs'),approval=require('../main/ai-approval.cjs');
const {api}=require('../agent/mcp.cjs');
const company='aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',actor='bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';
// optIn defaults to false here so existing attended-mode tests keep exercising the unticked flow.
function fixture({usable=true,changeSession=false,choice=2,page=null,optIn=false,exclusive=async work=>work(),duringConnect=null}={}) {
 const root=fs.mkdtempSync(path.join(os.tmpdir(),'fb-ai-controls-')),book=path.join(root,'synthetic.fbbook');fs.writeFileSync(book,'Synthetic scope marker');
 const key=crypto.randomBytes(32),messages=[],grants=new Map(),proofs=new Map();let copied='',generation=1,questions=0,grantCount=0,revoked=0,signedIn=true,view=company,role='administrator',who=actor,restored=false,open=true,busy=false,client=false,selected={},optInState=optIn;
 const storage={isEncryptionAvailable:()=>usable,getSelectedStorageBackend:()=>usable?'gnome_libsecret':'basic_text'};
 storage.encryptString=text=>{const nonce=crypto.randomBytes(12),c=crypto.createCipheriv('aes-256-gcm',key,nonce),bytes=Buffer.concat([c.update(text),c.final()]);return Buffer.concat([nonce,c.getAuthTag(),bytes]);};
 storage.decryptString=bytes=>{const c=crypto.createDecipheriv('aes-256-gcm',key,bytes.subarray(0,12));c.setAuthTag(bytes.subarray(12,28));return Buffer.concat([c.update(bytes.subarray(28)),c.final()]).toString();};
 const invalidate=()=>{generation++;grants.clear();};
 const change=(values)=>{({signedIn=signedIn,view=view,role=role,who=who,restored=restored,open=open,busy=busy,client=client}=values);invalidate();};
 const dialog={showMessageBox:async(_win,o)=>{messages.push(o);if(o.title==='Connect an AI assistant'){questions++;if(changeSession)invalidate();if(duringConnect)duringConnect({change,invalidate});return {response:choice,checkboxChecked:optInState};}return {response:selected[o.title] ?? 1};}};
 const native={appState:()=>JSON.stringify({open,signed_in:signedIn,path:book,viewing_restored:restored}),aiGeneration:()=>String(generation),
  aiCurrentCompany:()=>signedIn?view:null,
  aiContext:co=>{assert.ok(signedIn);assert.equal(co,view);return JSON.stringify({company_id:co,company_name:'Synthetic company',actor_id:who,actor_name:'Synthetic operator',role,book_path:book,restored,session_generation:generation});},
  aiGrant:(co,name,permission,id=crypto.randomUUID())=>{assert.ok(signedIn);assert.equal(co,view);assert.ok(permission==='read_only'||role!=='readonly');grantCount++;const g={id,token:crypto.randomBytes(32).toString('hex'),company_id:co,actor_id:who,name,permission};grants.set(g.id,g);return JSON.stringify(g);},
  aiConnections:()=>JSON.stringify([...grants.values()].map(({token,...g})=>g)),aiInvalidate:()=>{revoked++;invalidate();},aiForgetAll:()=>{revoked++;proofs.clear();invalidate();},aiRevoke:id=>{revoked++;grants.delete(id);},
  aiUnattendedStatus:raw=>{const p=JSON.parse(raw);if(!proofs.has(p.id))throw Error('approval_revoked');if(proofs.get(p.id)!==raw)throw Error('approval_gone');},
  aiSetUnattended:(id,epoch,enabled)=>{
   const g=grants.get(id);assert.ok(g && signedIn && role==='administrator');
   if(!enabled){proofs.delete(id);return 'null';}
   const raw=JSON.stringify({id,epoch,actor:g.actor_id,company:g.company_id,name:g.name,permission:g.permission,secret:crypto.randomBytes(32).toString('hex')});
   proofs.set(id,raw);return raw;
  },
  aiRequestAsync:async (token,_body,proof)=>{if(proof){native.aiUnattendedStatus(proof);const p=JSON.parse(proof);return JSON.stringify({company_id:p.company,permission:p.permission,unattended:true});}assert.ok(signedIn);const g=[...grants.values()].find(g=>g.token===token);assert.ok(g);assert.equal(g.actor_id,who);assert.equal(g.company_id,view);return JSON.stringify({company_id:view,permission:g.permission});}};
 const controller=()=>create({app:{isPackaged:false},native,dialog,clipboard:{writeText:text=>copied=text},safeStorage:storage,root,window:()=>({webContents:{getURL:()=>signedIn?`fb://app${page || `/c/${view}/home/`}`:'fb://app/desktop/sign-in/'}}),exclusive,busy:()=>busy,client:()=>client});
 return {controller:controller(),restart:controller,root,book,storage,native,messages,invalidate,change,setOptIn:v=>{optInState=v;},
  state:()=>({copied,grantCount,revoked,questions}),choose:choices=>selected=choices,proofs,files:()=>fs.readdirSync(path.join(root,'ai-connections')).filter(n=>approval.UUID.test(n.replace(/\.sealed$/,''))&&n.endsWith('.sealed')).map(n=>path.join(root,'ai-connections',n))};
}
async function cleanup(f,extra){await extra?.stop();await f.controller.stop();fs.rmSync(f.root,{recursive:true});}
test('contention before native work is busy, but a completed POST without delivery is unknown',async()=>{
 let phase='before',writes=0;
 const f=fixture({exclusive:async work=>{
  if(phase==='before')throw Error('BUSY');
  await work();throw Error('BUSY');
 }});
 try{
  await f.controller.connect();
  const record=approval.read(f.files()[0],f.storage);
  f.native.aiRequestAsync=async()=>{writes++;return JSON.stringify({outcome:'done'});};
  const request={operation:'command_SaveCustomer',arguments:{display_name:'Synthetic delivery test'}};
  await assert.rejects(api(record,'/v1/call',request),{code:'busy'});
  assert.equal(writes,0);
  phase='after';
  await assert.rejects(api(record,'/v1/call',request),{code:'outcome_unknown'});
  assert.equal(writes,1);
 }finally{await cleanup(f);}
});
test('an attended lock suppresses a completed query without implying it committed a change',async()=>{
 const f=fixture({exclusive:async work=>{
  await work();throw Object.assign(new Error('Signed out'),{code:'SESSION_CHANGED'});
 }});
 try{
  await f.controller.connect();
  const record=approval.read(f.files()[0],f.storage);
  await assert.rejects(api(record,'/v1/call',{operation:'query_Accounts',arguments:{}}),{code:'paused_signed_out'});
  await assert.rejects(api(record,'/v1/call',{operation:'command_SaveCustomer',arguments:{display_name:'Synthetic lock test'}}),{code:'outcome_unknown'});
 }finally{await cleanup(f);}
});
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
test('an Administrator is offered locked access ticked; the box is not offered to anyone else',async()=>{
 const admin=fixture({optIn:true});try{
  await admin.controller.connect();const [asked]=admin.messages.filter(m=>m.title==='Connect an AI assistant');
  const [confirmed]=admin.messages.filter(m=>m.title==='AI assistant connected');
  assert.equal(asked.checkboxLabel,'Keep working while FreedomBooks is locked');assert.equal(asked.checkboxChecked,true);assert.equal(asked.defaultId,0);
  assert.match(asked.detail,/starts ticked/);assert.match(asked.detail,/Untick it to pause access on lock, sign-out or company change/);
  assert.match(confirmed.detail,/With locked access enabled/);
  assert.ok(approval.read(admin.files()[0],admin.storage).unattended,'kept ticked, the approval works while locked');
 }finally{await cleanup(admin);}
 const unticked=fixture({optIn:false});try{
  await unticked.controller.connect();const [confirmed]=unticked.messages.filter(m=>m.title==='AI assistant connected');
  assert.match(confirmed.detail,/After a restart, sign in and open this company/);
  assert.equal(approval.read(unticked.files()[0],unticked.storage).unattended,undefined,'unticked, the approval pauses on lock');
 }finally{await cleanup(unticked);}
 for(const [nonAdminRole,choice] of [['readonly',1],['bookkeeper',2]]){
  const other=fixture({optIn:true,choice});try{
   other.change({role:nonAdminRole});await other.controller.connect();const [asked]=other.messages.filter(m=>m.title==='Connect an AI assistant');
   const [confirmed]=other.messages.filter(m=>m.title==='AI assistant connected');
   assert.equal(asked.checkboxLabel,undefined);assert.equal(asked.checkboxChecked,undefined);assert.equal(asked.defaultId,1);
   assert.match(asked.detail,/Access pauses on lock, sign-out or company change/);
   assert.match(confirmed.detail,/After a restart, sign in and open this company/);
   const saved=approval.read(other.files()[0],other.storage);
   assert.equal(saved.permission,choice===1?'read_only':'accounting_write');
   assert.equal(saved.unattended,undefined,'a ticked reply cannot enable locked access for '+nonAdminRole);
  }finally{await cleanup(other);}
 }
});
test('Administrator Read only and Cancel respect the ticked checkbox and cancel leaves nothing',async()=>{
 const ro=fixture({optIn:true,choice:1});try{
  await ro.controller.connect();const [confirmed]=ro.messages.filter(m=>m.title==='AI assistant connected');
  const saved=approval.read(ro.files()[0],ro.storage);
  assert.equal(saved.permission,'read_only');assert.ok(saved.unattended);
  assert.match(confirmed.detail,/With locked access enabled/);
 }finally{await cleanup(ro);}
 const cancel=fixture({optIn:true,choice:0});try{
  await cancel.controller.connect();
  assert.equal(cancel.state().grantCount,0);assert.equal(cancel.proofs.size,0);assert.equal(cancel.files().length,0);
 }finally{await cleanup(cancel);}
});
test('repeating connect on an existing attended approval keeps it attended even when ticked is default',async()=>{
 const f=fixture({optIn:false});try{
  await f.controller.connect();const file=f.files()[0],first=approval.read(file,f.storage);
  assert.equal(first.unattended,undefined);
  f.setOptIn(true);await f.controller.connect();
  assert.equal(f.state().questions,1,'existing approval reuses without showing the connect dialog');
  assert.equal(approval.read(file,f.storage).unattended,undefined);
  assert.match(f.messages.at(-1).detail,/After a restart, sign in and open this company/);
 }finally{await cleanup(f);}
});
test('mid-dialog session, role, restored preview or client changes refuse ticked setup cleanly',async()=>{
 for(const values of [{who:'cccccccc-cccc-4ccc-8ccc-cccccccccccc'},{role:'bookkeeper'},{role:'readonly'},{restored:true},{client:true}]){
  const f=fixture({optIn:true,duringConnect:({change})=>change(values)});try{
   await f.controller.connect();
   assert.equal(f.state().grantCount,0,JSON.stringify(values));
   assert.equal(f.proofs.size,0,JSON.stringify(values));
   assert.equal(f.files().length,0,JSON.stringify(values));
   assert.equal(f.messages.at(-1).type,'error',JSON.stringify(values));
  }finally{await cleanup(f);}
 }
});
test('ticked connect handles half-way setup failures, rollbacks and busy post-write revoke',async()=>{
 const failProof=fixture({optIn:true});try{
  failProof.native.aiSetUnattended=()=>{throw Error('Synthetic book write failed');};
  await failProof.controller.connect();
  assert.equal(failProof.state().grantCount,1);assert.equal(failProof.state().revoked,1);
  assert.equal(failProof.files().length,0);assert.equal(failProof.proofs.size,0);
  assert.match(failProof.messages.at(-1).detail,/data folder and books.*untick locked access/);
 }finally{await cleanup(failProof);}
 const failWrite=fixture({optIn:true});try{
  const encrypt=failWrite.storage.encryptString;
  failWrite.storage.encryptString=value=>{if(value.includes('"unattended"'))throw Error('Synthetic keyring locked');return encrypt.call(failWrite.storage,value);};
  await failWrite.controller.connect();
  assert.equal(failWrite.state().grantCount,1);assert.ok(failWrite.state().revoked>=1);
  assert.equal(failWrite.files().length,0);assert.equal(failWrite.proofs.size,0);
  assert.equal(failWrite.messages.at(-1).type,'error');
 }finally{await cleanup(failWrite);}
 const doubleFail=fixture({optIn:true});try{
  const encrypt=doubleFail.storage.encryptString,setUnattended=doubleFail.native.aiSetUnattended;
  doubleFail.storage.encryptString=value=>{if(value.includes('"unattended"'))throw Error('Synthetic keyring locked');return encrypt.call(doubleFail.storage,value);};
  doubleFail.native.aiSetUnattended=(id,epoch,enabled)=>{if(!enabled)throw Error('BUSY');return setUnattended(id,epoch,enabled);};
  doubleFail.native.aiRevoke=()=>{throw Error('BUSY');};
  await doubleFail.controller.connect();
  assert.equal(doubleFail.files().length,0);assert.equal(doubleFail.messages.at(-1).type,'error');
 }finally{await cleanup(doubleFail);}
 const busyRevoke=fixture({optIn:true});try{
  busyRevoke.native.aiRevoke=()=>{throw Error('BUSY');};
  await busyRevoke.controller.connect();
  assert.ok(!busyRevoke.messages.some(m=>m.type==='error'));
  assert.match(busyRevoke.messages.at(-1).detail,/With locked access enabled/);
  assert.equal(busyRevoke.files().length,1);
  busyRevoke.change({signedIn:false,view:null});
  assert.equal((await api(approval.read(busyRevoke.files()[0],busyRevoke.storage),'/v1/catalogue')).unattended,true);
 }finally{await cleanup(busyRevoke);}
});
test('read-only approval stays read-only after restart and role changes',async()=>{
 const f=fixture({choice:1});try{await f.controller.connect();f.change({role:'administrator'});await f.controller.resume(company);assert.equal(approval.read(f.files()[0],f.storage).permission,'read_only');assert.equal(f.state().questions,1);}finally{await cleanup(f);}
});
test('unavailable keyring and changing sessions during setup or resume refuse stale authority',async()=>{
 for(const options of [{usable:false},{changeSession:true}]){const f=fixture(options);try{await f.controller.connect();assert.equal(f.state().grantCount,0);assert.equal(f.state().copied,'');if(options.usable===false)assert.equal(f.messages.at(-1).detail,'Unlock your desktop keyring before connecting an AI.');}finally{await cleanup(f);}}
 const f=fixture();try{await f.controller.connect();f.invalidate();await f.controller.stop();const p=f.controller.resume(company);f.change({signedIn:false});await p;assert.equal(f.state().grantCount,1);}finally{await cleanup(f);}
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

test('Manage toggles unattended access in place and the attended token cannot acquire it',async()=>{
 const f=fixture();try{
  await f.controller.connect();const file=f.files()[0],old=approval.read(file,f.storage);
  f.choose({'AI assistant connections':2,'Change AI locked access':1});
  await f.controller.manage();
  const awake=approval.read(file,f.storage);assert.ok(awake.unattended);
  assert.notEqual(awake.token,old.token);
  assert.deepEqual(f.files(),[file]);
  assert.equal(f.state().questions,1);
  assert.ok(!JSON.stringify(f.messages).includes(awake.unattended.secret));
  await assert.rejects(api(old,'/v1/catalogue'),{code:'approval_gone'});
  f.change({signedIn:false,view:null});
  assert.equal((await api(awake,'/v1/catalogue')).unattended,true);
  f.change({signedIn:true,view:company});
  await f.controller.manage();
  assert.match(f.messages.findLast(m=>m.title==='AI assistant connections').detail,/Works while locked/);
  const attended=approval.read(file,f.storage);
  assert.equal(attended.unattended,undefined);assert.notEqual(attended.token,awake.token);
  assert.equal(attended.locked_access_off.reason,'disabled');
  f.choose({'AI assistant connections':0});await f.controller.manage();
  assert.match(f.messages.at(-1).detail,/turned off by the approver on/);
  f.change({signedIn:false,view:null});
  await assert.rejects(api(attended,'/v1/catalogue'),{code:'paused_signed_out'});
 }finally{await cleanup(f);}
});
test('ticked-by-default locked access launches signed out after restart and status codes distinguish pauses',async()=>{
 const f=fixture({optIn:true});let restarted;try{
  await f.controller.connect();
  const file=f.files()[0],r=approval.read(file,f.storage);assert.ok(r.unattended);
  await f.controller.stop();f.change({signedIn:false,view:null});restarted=f.restart();await restarted.start();
  const connection=()=>({...approval.read(file,f.storage),...approval.runtime(f.root,f.storage)});
  assert.equal((await api(connection,'/v1/catalogue')).unattended,true);
  for(const values of [{open:false},{open:true,restored:true},{restored:false,client:true}]){
   f.change(values);await assert.rejects(api(connection,'/v1/catalogue'),{code:'books_closed'});
  }
  f.change({client:false,busy:true});await assert.rejects(api(connection,'/v1/catalogue'),{code:'busy'});
  f.change({busy:false});f.proofs.clear();
  await assert.rejects(api(connection,'/v1/catalogue'),{code:'approval_gone'});
  f.change({signedIn:true,view:company});
  await assert.rejects(api(connection,'/v1/catalogue'),{code:'approval_gone'});
  assert.deepEqual(approval.read(file,f.storage).unattended,r.unattended);
  f.choose({'AI assistant connections':0});await restarted.manage();
  assert.match(f.messages.at(-1).detail,/Approval can no longer be used here; connect again to approve access/);
  fs.unlinkSync(file);await assert.rejects(api({...r,...approval.runtime(f.root,f.storage)},'/v1/catalogue'),{code:'approval_gone'});
 }finally{await cleanup(f,restarted);}
});

test('replaying the sealed approval and epoch after Forget never falls back to attended',async()=>{
 const f=fixture({optIn:true});let restarted;try{
  await f.controller.connect();
  const file=f.files()[0],epoch=path.join(path.dirname(file),'.approval-epoch.sealed');
  const copies=[file,epoch].map(name=>[name,fs.readFileSync(name)]);
  await f.controller.manage();
  for(const [name,bytes] of copies)fs.writeFileSync(name,bytes,{mode:0o600});
  restarted=f.restart();await restarted.start();
  const restored={...approval.read(file,f.storage),...approval.runtime(f.root,f.storage)};
  f.change({signedIn:false});await assert.rejects(api(restored,'/v1/catalogue'),{code:'approval_gone'});
  f.change({signedIn:true});await restarted.resume(company);
  await assert.rejects(api(restored,'/v1/catalogue'),{code:'approval_gone'});
  assert.equal(f.state().grantCount,1);
 }finally{await cleanup(f,restarted);}
});

test('transient anchor failure preserves proof; Manage distinguishes other books',async()=>{
 const f=fixture({optIn:true});try{
  await f.controller.connect();const file=f.files()[0],bytes=fs.readFileSync(file);
  const status=f.native.aiUnattendedStatus;
  f.native.aiUnattendedStatus=()=>{throw Error('anchor_unavailable');};
  await assert.rejects(api(approval.read(file,f.storage),'/v1/catalogue'),{code:'busy'});
  f.choose({'AI assistant connections':0});await f.controller.manage();
  assert.match(f.messages.at(-1).detail,/approval unchanged/);
  await f.controller.resume(company);assert.deepEqual(fs.readFileSync(file),bytes);
  f.native.aiUnattendedStatus=status;
  assert.equal((await api(approval.read(file,f.storage),'/v1/catalogue')).unattended,true);
  fs.renameSync(f.book,f.book+'.original');fs.writeFileSync(f.book,'Other synthetic books');
  await f.controller.manage();assert.match(f.messages.at(-1).detail,/Belongs to other books/);
  assert.deepEqual(fs.readFileSync(file),bytes);
 }finally{await cleanup(f);}
});

test('account revocation falls back to the approving user and re-enables in the same file',async()=>{
 const f=fixture({optIn:true});try{
  await f.controller.connect();const file=f.files()[0],record=()=>approval.read(file,f.storage),id=record().id;
  const status=f.native.aiUnattendedStatus,proof=record().unattended;
  f.native.aiUnattendedStatus=raw=>raw===JSON.stringify(proof)?
   JSON.stringify({at:'2026-11-20T12:00:00Z',reason:'account_changed'}):status(raw);
  for(const state of [{signedIn:false},{signedIn:true,who:crypto.randomUUID()},{who:actor,role:'readonly'}]){
   f.change(state);await assert.rejects(api(record,'/v1/catalogue'),{code:'paused_signed_out'});
  }
  f.change({role:'administrator'});assert.equal((await api(record,'/v1/catalogue')).unattended,undefined);
  f.choose({'AI assistant connections':2,'Change AI locked access':1});await f.controller.manage();
  assert.match(f.messages.find(m=>m.title==='AI assistant connections').detail,/Requires sign-in.*\n.*account changed/i);
  assert.equal(record().id,id);assert.notDeepEqual(record().unattended,proof);assert.deepEqual(f.files(),[file]);
  f.change({signedIn:false});assert.equal((await api(record,'/v1/catalogue')).unattended,true);
 }finally{await cleanup(f);}
});

// Third review, N14: after an account change switched locked access off, a save that fails while the
// approver turns it on again must not leave a remembered approval that only a new connection replaces.
test('a failed save while turning locked access on again leaves an approval that can be retried',async()=>{
 const f=fixture({optIn:true});try{
  await f.controller.connect();const file=f.files()[0],record=()=>approval.read(file,f.storage),id=record().id;
  const status=f.native.aiUnattendedStatus,proof=record().unattended;
  f.native.aiUnattendedStatus=raw=>raw===JSON.stringify(proof)?
   JSON.stringify({at:'2026-11-20T12:00:00Z',reason:'account_changed'}):status(raw);
  // The first save of the new proof fails; later saves work.
  const encrypt=f.storage.encryptString;let failed=false;
  f.storage.encryptString=value=>{
   if(!failed && value.includes('"unattended"') && !value.includes(proof.secret)){failed=true;throw Error('Synthetic disk full');}
   return encrypt.call(f.storage,value);
  };
  f.choose({'AI assistant connections':2,'Change AI locked access':1});
  await f.controller.manage();
  assert.equal(failed,true,'the save of the new proof was attempted and failed');
  assert.equal(record().id,id);assert.equal(record().unattended,undefined,'no proof without a record in the book stays behind');
  assert.equal(record().locked_access_off.reason,'disabled');
  assert.deepEqual(f.files(),[file]);
  // Signed out it needs the approver, as any approval that requires sign-in does; it is not gone.
  f.change({signedIn:false});await assert.rejects(api(record,'/v1/catalogue'),{code:'paused_signed_out'});
  // The approver turns it on again in the same file.
  f.change({signedIn:true});
  f.choose({'AI assistant connections':2,'Change AI locked access':1});await f.controller.manage();
  assert.ok(record().unattended);assert.equal(record().id,id);
  f.change({signedIn:false});assert.equal((await api(record,'/v1/catalogue')).unattended,true);
 }finally{await cleanup(f);}
});

test('Forget rotates the epoch even if book cleanup fails',async()=>{
 const f=fixture();let restarted;try{
  await f.controller.connect();const file=f.files()[0],bytes=fs.readFileSync(file),prior=approval.epoch(f.root,f.storage);
  f.native.aiForgetAll=()=>{throw Error('synthetic cleanup failure');};
  await f.controller.manage();
  assert.notEqual(approval.epoch(f.root,f.storage),prior);
  assert.match(f.messages.at(-1).detail,/cleanup/i);
  assert.match(f.messages.at(-1).detail,/may still be recorded in the books\. Quit FreedomBooks to stop access/);
  fs.writeFileSync(file,bytes,{mode:0o600});restarted=f.restart();await restarted.start();
  await assert.rejects(api({...approval.read(file,f.storage),...approval.runtime(f.root,f.storage)},'/v1/catalogue'),{code:'approval_gone'});
 }finally{await cleanup(f,restarted);}
});

test('Forget reports a locked-access file it could not remove',async()=>{
 for(const left of [true,false]){
  const f=fixture();try{
   await f.controller.connect();
   const forgetAll=f.native.aiForgetAll;f.native.aiForgetAll=()=>{forgetAll();return left;};
   await f.controller.manage();
   assert.equal(f.messages.some(m=>/locked-access file could not be removed/.test(m.detail||'')),left);
   assert.equal(f.files().length,0);
  }finally{await cleanup(f);}
 }
});

test('Forget explains cleanup when approved books were not open',async()=>{
 for(const condition of ['closed','preview','other']){
  const f=fixture({optIn:true});try{
   await f.controller.connect();
   if(condition==='closed')f.change({open:false});
   if(condition==='preview')f.change({restored:true});
   if(condition==='other'){fs.renameSync(f.book,f.book+'.original');fs.writeFileSync(f.book,'Other synthetic books');}
   await f.controller.manage();
   assert.match(f.messages.at(-1).detail,/approved books were not open/i,condition);
  }finally{await cleanup(f);}
 }
});

test('wrong bearers give the same refusal for existing, unknown and malformed IDs',async()=>{
 const f=fixture({optIn:true});try{
  await f.controller.connect();const r=approval.read(f.files()[0],f.storage);
  for(const id of [r.id,crypto.randomUUID(),'invalid',undefined]){
   await assert.rejects(api({...r,id,token:'0'.repeat(64)},'/v1/catalogue'),{code:'approval_gone'});
  }
 }finally{await cleanup(f);}
});

test('copied configuration prefers an executable stable symlink and rejects invalid overrides',async()=>{
 const f=fixture();const prior=process.env.FB_AI_LAUNCHER;
 try{
  const target=path.join(f.root,'versioned'),stable=path.join(f.root,'stable');
  fs.writeFileSync(target,'synthetic launcher',{mode:0o700});fs.symlinkSync(target,stable);
  const writable=path.join(f.root,'writable');fs.writeFileSync(writable,'synthetic unsafe launcher');fs.chmodSync(writable,0o772);
  const cases=[[stable,stable],[f.root,process.execPath],[f.book,process.execPath],['relative',process.execPath],[writable,process.execPath]];
  if(process.getuid && fs.existsSync('/bin/sh') && fs.statSync('/bin/sh').uid!==process.getuid())cases.push(['/bin/sh',process.execPath]);
  for(const [override,command] of cases){
   process.env.FB_AI_LAUNCHER=override;await f.controller.connect();
   const config=JSON.parse(f.state().copied).mcpServers.freedombooks;
   assert.equal(config.command,command);assert.deepEqual(config.args.slice(-2),['--ai-mcp',f.files()[0]]);
   assert.equal(config.args.length,command===stable?2:3);
  }
 }finally{
  if(prior===undefined)delete process.env.FB_AI_LAUNCHER;else process.env.FB_AI_LAUNCHER=prior;
  await cleanup(f);
 }
});

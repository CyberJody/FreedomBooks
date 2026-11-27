'use strict';
const fs=require('node:fs'),path=require('node:path'),crypto=require('node:crypto');
const approval=require('./ai-approval.cjs');
const {refusal,nativeError}=require('../agent/refusal.cjs');

const SIGN_IN='Sign in and open a company before connecting an AI.';
const KEYRING='Unlock your desktop keyring before connecting an AI.';
const RESTORED='AI assistants are unavailable in a restored-book preview. Reopen your main books to connect.';
const CLIENT='AI connections are available on the computer hosting these books.';
const EXPLAIN_LOCKED='Any program running as this computer’s user can use this connection as you while the desktop keyring is unlocked. With locked access enabled, locking FreedomBooks no longer stops that access, even if another person signs in. After a reboot, the desktop session must be running, the keyring unlocked, and FreedomBooks and the original books open. Forget stops local access. If Forget removes the approval from the open books, restoring profile files alone cannot revive it. If cleanup did not finish, restoring matching profile files before sign-in removes the missing approval can bring it back. Restoring matching older book and profile data can also bring access back.';
const ZERO_BEARER=Buffer.alloc(32);
const sameBearer=(saved,supplied)=>crypto.timingSafeEqual(saved?Buffer.from(saved,'hex'):ZERO_BEARER,supplied);

function mayHaveWritten(body) {
  if(body===undefined)return false;
  try{return !JSON.parse(body)?.operation?.startsWith('query_');}
  catch{return true;}
}

function launcher() {
  const file=process.env.FB_AI_LAUNCHER;
  if(file && path.isAbsolute(file))try{
    const target=fs.statSync(file);
    if(target.isFile() && typeof process.getuid==='function' && target.uid===process.getuid() && !(target.mode&0o022)){
      fs.accessSync(file,fs.constants.X_OK);
      return file;
    }
  }catch{/* Fall back to this build's launcher. */}
  return null;
}

function create({app,native,dialog,clipboard,safeStorage,root,window,exclusive,busy,client=()=>false}) {
  const directory=path.join(root,'ai-connections');
  let working=false,suspended=false,endpoint=null,publishing=null,publishTimer=null;
  const usable=()=>approval.usable(safeStorage);
  function files() {
    if(!fs.existsSync(directory))return [];
    return fs.readdirSync(directory).filter(n=>n.endsWith('.sealed') && approval.UUID.test(n.slice(0,-7))).sort();
  }
  function company() {return native.aiCurrentCompany();}
  function context(co) {
    if(client())throw Error(CLIENT);
    const state=JSON.parse(native.appState());
    if(state.viewing_restored)throw Error(RESTORED);
    if(!co || !state.open || !state.signed_in)throw Error(SIGN_IN);
    if(!usable())throw Error(KEYRING);
    const c=JSON.parse(native.aiContext(co));
    if(c.restored)throw Error(RESTORED);
    return {...c,book_scope:approval.scope(c.book_path)};
  }
  function matches(r,c) {
    return r.book_scope===c.book_scope && r.company_id===c.company_id && r.actor_id===c.actor_id &&
      (r.permission==='read_only' || ['administrator','bookkeeper'].includes(c.role));
  }
  function saved() {
    return files().slice(0,32).flatMap(name=>{
      const file=path.join(directory,name);
      try{return [{file,record:approval.read(file,safeStorage)}];}catch{return [];}
    });
  }

  async function start(required=false) {
    if(endpoint || (!required && !files().length))return endpoint;
    if(publishing)return publishing;
    const publish=async()=>{
      if(!usable())throw Error(KEYRING);
      const url=await transport.start();
      approval.publish(root,safeStorage,{endpoint:url,cookie:transport.cookie});
      endpoint=url;
      clearTimeout(publishTimer);publishTimer=null;
      return endpoint;
    };
    publishing=publish().catch(error=>{
      if(!required && !publishTimer){
        publishTimer=setTimeout(()=>{publishTimer=null;void start();},2000);
        publishTimer.unref();
      }
      if(required)throw error;
      return null;
    }).finally(()=>{publishing=null;});
    return publishing;
  }
  async function stop() {
    if(publishing)try{await publishing;}catch{}
    clearTimeout(publishTimer);publishTimer=null;
    approval.unpublish(root,safeStorage,transport.cookie);
    endpoint=null;
    await transport.stop();
  }
  // Only the local desktop has a path match and a fully authenticated, selected company.
  function attended(r,{renew=false}={}) {
    let c;
    try{
      if(company()!==r.company_id)throw refusal('paused_signed_out');
      c=context(r.company_id);
    }catch(error){
      if(error.message==='BUSY')throw refusal('busy');
      throw refusal('paused_signed_out');
    }
    if(!matches(r,c))throw refusal('paused_signed_out');
    const live=JSON.parse(native.aiConnections()).some(g=>g.id===r.id);
    if(live && !renew)return r;
    const grant=JSON.parse(native.aiGrant(r.company_id,r.name,r.permission,r.id));
    if(grant.id!==r.id || grant.actor_id!==r.actor_id || grant.company_id!==r.company_id || grant.permission!==r.permission){
      native.aiRevoke(grant.id);
      throw refusal('paused_signed_out');
    }
    return {...r,endpoint,token:grant.token};
  }
  function ensureAttended(r,file) {
    const ready=attended(r);
    if(ready!==r){
      try{approval.write(file,ready,safeStorage);}catch(error){native.aiRevoke(ready.id);throw error;}
    }
    return ready;
  }
  function lockedStatus(r,state) {
    if(!state.open || state.viewing_restored || !state.path)return {r,status:'closed'};
    try{if(approval.scope(state.path)!==r.book_scope)return {r,status:'other'};}
    catch{return {r,status:'unavailable'};}
    if(r.locked_access_off?.reason==='revoked')return {r,status:'gone'};
    if(!r.unattended)return {r,status:'attended'};
    try{
      const off=native.aiUnattendedStatus(JSON.stringify(r.unattended));
      return off?{r,status:'attended',off:JSON.parse(off)}:{r,status:'enabled'};
    }
    catch(error){
      if(error.message==='BUSY')throw error;
      if(error.message==='anchor_unavailable')return {r,status:'retry'};
      if(error.message==='approval_gone' || error.message==='approval_revoked')return {r,status:'gone'};
      return {r,status:'unavailable'};
    }
  }
  async function resume(co) {
    if(busy() || suspended || !co || client())return;
    try {
      if(!await start() || busy() || suspended || client() || company()!==co)return;
      const c=context(co),epoch=approval.epoch(root,safeStorage),state=JSON.parse(native.appState());
      for(const {file,record:r} of saved()){
        if(r.epoch!==epoch || !matches(r,c))continue;
        const checked=lockedStatus(r,state);
        if(checked.status==='attended')ensureAttended(checked.r,file);
      }
    }catch{/* Optional AI storage must not hide a book page. */}
  }

  function request(token,body,id) {
    if(suspended)throw refusal('approval_gone');
    if(!usable())throw refusal('keyring_unavailable');
    let epoch;
    try{epoch=approval.epoch(root,safeStorage);}catch(error){throw refusal(error.code==='ENOENT'?'approval_gone':'keyring_unavailable');}
    let entry;
    const supplied=Buffer.from(token,'hex');
    if(typeof id==='string' && approval.UUID.test(id)){
      const file=path.join(directory,id+'.sealed');
      try{entry={file,record:approval.read(file,safeStorage)};}catch{/* No matching approval. */}
    }else if(id===undefined){
      for(const savedEntry of saved())if(sameBearer(savedEntry.record.token,supplied))entry=savedEntry;
    }
    if(!sameBearer(entry?.record.token,supplied) || !entry || entry.record.epoch!==epoch)throw refusal('approval_gone');
    const {file,record:r}=entry;
    if(busy())throw refusal('busy');
    try{
      const state=JSON.parse(native.appState());
      if(client() || !state.open || state.viewing_restored)throw refusal('books_closed');
      const checked=lockedStatus(r,state);
      if(checked.status==='retry')throw nativeError(Error('anchor_unavailable'));
      if(checked.status==='unavailable')throw refusal('operation_refused');
      if(!['enabled','attended'].includes(checked.status))throw refusal('approval_gone');
      const enabled=checked.status==='enabled';
      const grant=enabled?r:ensureAttended(checked.r,file);
      let completed=false;
      return exclusive(async()=>{
        const generation=enabled?null:native.aiGeneration();
        const value=await native.aiRequestAsync(grant.token,body,enabled?JSON.stringify(r.unattended):undefined);
        completed=true;
        if(!enabled && generation!==native.aiGeneration())throw refusal('paused_signed_out');
        return value;
      },{bindSession:!enabled}).catch(error=>{
        if(completed && mayHaveWritten(body))throw refusal('outcome_unknown');
        throw nativeError(!enabled && error?.message==='approval_gone'?refusal('paused_signed_out'):error);
      });
    }catch(error){throw error.code && error.code!=='GenericFailure'?error:nativeError(error);}
  }
  const transport=require('./ai-transport.cjs').create({request});
  const config=file=>{
    const stable=launcher();
    return {mcpServers:{freedombooks:{command:stable || process.execPath,args:stable || app.isPackaged?['--ai-mcp',file]:[path.resolve(__dirname,'..'),'--ai-mcp',file]}}};
  };

  async function showConfiguration(file,status) {
    const text=JSON.stringify(config(file),null,2);
    const instructions=status==='enabled'?EXPLAIN_LOCKED:status==='attended'?'After a restart, sign in and open this company. Lock or sign-out pauses access. Manage AI connections can forget the approval.':'Locked access could not be checked. See Manage AI connections.';
    const choice=await dialog.showMessageBox(window(),{type:'info',title:'AI assistant connected',message:'Company approval saved on this computer',detail:text+'\n\nAdd this configuration to your AI client once. '+instructions,buttons:['Done','Copy MCP configuration'],defaultId:0});
    if(choice.response===1)await clipboard.writeText(text);
  }
  function setLockedAccess(r,file,enabled,{create=false}={}) {
    // A fresh attended grant proves the exact approval and binds its original ID to this session.
    const current=attended(r,{renew:!create});
    const proof=JSON.parse(native.aiSetUnattended(r.id,r.epoch,enabled));
    const next={...current};
    delete next.unattended;
    delete next.locked_access_off;
    if(enabled){
      next.unattended=proof;
      // A previously copied attended bearer must not gain unattended access.
      next.token=crypto.randomBytes(32).toString('hex');
    }else next.locked_access_off={at:new Date().toISOString(),reason:'disabled'};
    try{approval.write(file,next,safeStorage,{create});}
    catch(error){
      if(enabled){
        try{native.aiSetUnattended(r.id,r.epoch,false);}catch{}
        // The earlier proof now has no record in the book. Leave an approval that needs sign-in, which
        // its approver can turn on again in Manage, when the file can be written after all.
        if(!create)try{
          const off={...current};delete off.unattended;
          off.locked_access_off={at:new Date().toISOString(),reason:'disabled'};
          approval.write(file,off,safeStorage);
        }catch{/* the first error is the one to report */}
      }
      try{native.aiRevoke(r.id);}catch{}
      throw error;
    }
    if(enabled)try{native.aiRevoke(r.id);}catch{}
    return next;
  }
  async function connect() {
    if(working || busy() || client())return;
    working=true;let grant;
    try {
      const co=company(),c=context(co),generation=native.aiGeneration();
      const epoch=suspended?approval.rotateEpoch(root,safeStorage):approval.epoch(root,safeStorage,{create:true});
      for(const {file,record:r} of saved()){
        if(r.epoch===epoch && matches(r,c)){
          const checked=lockedStatus(r,JSON.parse(native.appState()));
          if(checked.status==='gone')continue;
          await start(true);
          await showConfiguration(file,checked.status);
          return;
        }
      }
      if(files().length>=32)throw Error('Forget unused AI connections before adding another.');
      const canChoose=c.role==='administrator';
      const selected=await dialog.showMessageBox(window(),{type:'question',title:'Connect an AI assistant',message:`Allow an AI assistant to work in ${c.company_name}?`,detail:`This approval is for ${c.actor_name} in this company and these books on this computer. Read only allows queries and previews. Accounting write also allows supported posting and imports.${canChoose?'\n\n“Keep working while FreedomBooks is locked” starts ticked. '+EXPLAIN_LOCKED+'\n\nUntick it to pause access on lock, sign-out or company change. You can change this later in Settings → Manage AI connections.':' Access pauses on lock, sign-out or company change.'}`,buttons:['Cancel','Read only','Accounting write'],defaultId:canChoose?0:1,cancelId:0,...(canChoose?{checkboxLabel:'Keep working while FreedomBooks is locked',checkboxChecked:true}:{})});
      if(selected.response===0)return;
      await start(true);
      if(busy() || generation!==native.aiGeneration() || co!==company() || approval.epoch(root,safeStorage)!==epoch)throw Error('Session changed');
      const now=context(co);
      if(now.actor_id!==c.actor_id || now.book_scope!==c.book_scope || now.session_generation!==c.session_generation)throw Error('Session changed');
      const name=`AI assistant ${new Date().toISOString()}`;
      const permission=selected.response===1?'read_only':'accounting_write';
      grant=JSON.parse(native.aiGrant(co,name,permission));
      const file=path.join(directory,grant.id+'.sealed');
      const r={version:2,id:grant.id,epoch,book_scope:c.book_scope,company_id:co,actor_id:grant.actor_id,name,permission,approved_at:new Date().toISOString(),endpoint,token:grant.token};
      const enabled=Boolean(canChoose && selected.checkboxChecked);
      if(enabled)setLockedAccess(r,file,true,{create:true});
      else approval.write(file,r,safeStorage,{create:true});
      suspended=false;
      grant=null;
      await showConfiguration(file,enabled?'enabled':'attended');
    } catch(error) {
      if(grant)try{native.aiRevoke(grant.id);}catch{}
      await dialog.showMessageBox(window(),{type:'error',message:'AI connection could not be set up',detail:[SIGN_IN,KEYRING,RESTORED,CLIENT,'Forget unused AI connections before adding another.'].includes(error.message)?error.message:'Check that you are signed in, the company is open, the desktop keyring is unlocked and FreedomBooks can write to its data folder and books. You can also try again or untick locked access.'});
    } finally {working=false;}
  }
  async function changeLockedAccess(eligible,c,epoch) {
    const generation=native.aiGeneration();
    for(const {file,record:r,enabled} of eligible){
      const choice=await dialog.showMessageBox(window(),{type:'question',title:'Change AI locked access',message:r.name,detail:`${c.company_name} · ${r.permission==='read_only'?'Read only':'Accounting write'}\n\n${EXPLAIN_LOCKED}`,buttons:['Cancel',enabled?'Require sign-in again':'Keep working while FreedomBooks is locked',...(eligible.length>1?['Next connection']:[])],defaultId:0,cancelId:0});
      if(choice.response===0)return;
      if(choice.response===2)continue;
      if(busy() || suspended || generation!==native.aiGeneration() || approval.epoch(root,safeStorage)!==epoch || !matches(r,context(company())))throw Error('Session changed');
      const current=approval.read(file,safeStorage);
      if(current.epoch!==epoch || current.id!==r.id || current.permission!==r.permission)throw Error('Approval changed');
      const checked=lockedStatus(current,JSON.parse(native.appState()));
      if(checked.status!==(enabled?'enabled':'attended'))throw Error('Approval changed');
      setLockedAccess(checked.r,file,!enabled);
      return;
    }
  }
  async function forget(approvedBooksClosed) {
    suspended=true;
    let cleanupFailed=false,anchorLeft=false;
    try{anchorLeft=native.aiForgetAll()===true;}catch{
      cleanupFailed=true;
      try{native.aiInvalidate();}catch{}
    }
    try{
      approval.rotateEpoch(root,safeStorage);
    }catch{
      try{native.aiInvalidate();}catch{}
      await dialog.showMessageBox(window(),{type:'error',message:'Access stopped, but forgetting approval could not be saved',detail:'Unlock the desktop keyring, check access to the application data folder and try Forget again before restarting.'});
      return;
    }
    let removalFailed=false;
    for(const name of files())try{fs.unlinkSync(path.join(directory,name));}catch{removalFailed=true;}
    await stop();
    suspended=false;
    const warnings=[];
    if(approvedBooksClosed)warnings.push('The approved books were not open. Any remaining locked-access rows with missing anchors are removed when someone signs in to those books and cleanup succeeds.');
    if(cleanupFailed)warnings.push('Book cleanup did not finish, so the approval may still be recorded in the books. Quit FreedomBooks to stop access. Then start it again and choose Forget straight away, until this warning no longer appears.');
    if(anchorLeft)warnings.push('A locked-access file could not be removed, or its folder could not be checked. Make the application data folder writable, open the approved books and choose Forget again.');
    if(removalFailed)warnings.push('Some retired files could not be removed. You can remove them later when the folder is writable.');
    if(warnings.length)await dialog.showMessageBox(window(),{type:'warning',message:'Local AI approvals forgotten',detail:warnings.join('\n\n')});
  }
  async function manage() {
    if(busy() || working || client())return;
    working=true;
    try{
      const entries=saved();
      let c,epoch;
      try{c=context(company());}catch{}
      try{epoch=approval.epoch(root,safeStorage);}catch{}
      const state=JSON.parse(native.appState());
      const records=entries.filter(({record})=>record.epoch===epoch).map(({file,record})=>{
        let checked;
        try{checked=lockedStatus(record,state);}catch{checked={r:record,status:'unavailable'};}
        return {file,record:checked.r,status:checked.status,off:checked.off,enabled:checked.status==='enabled'};
      });
      const eligible=c?.role==='administrator'?records.filter(({record,status})=>['enabled','attended'].includes(status) && matches(record,c)):[];
      const unavailable=files().length-records.length;
      const descriptions={enabled:'Works while locked',other:'Belongs to other books',closed:'Open the approved books to check locked access',unavailable:'Locked access could not be verified; approval unchanged',retry:'Could not read the local approval now; try again, approval unchanged',attended:'Requires sign-in',gone:'Approval can no longer be used here; connect again to approve access (Forget all approvals also removes this entry)'};
      const listing=records.map(({record:r,status,off:event})=>{
        const off=event || r.locked_access_off;
        const reason={account_changed:'turned off because the account changed',mfa_changed:'turned off because two-factor sign-in changed',revoked:'first found revoked',disabled:'turned off by the approver'};
        const when=off?`\nLocked access ${reason[off.reason]} on ${new Date(off.at).toLocaleString()}.`:'';
        return `${r.name}\n${r.permission==='read_only'?'Read only':'Accounting write'} · ${descriptions[status]}${when}\nCompany ${r.company_id} · User ${r.actor_id}`;
      }).join('\n\n') || 'No remembered AI approvals.';
      const choice=await dialog.showMessageBox(window(),{type:'info',title:'AI assistant connections',message:`${records.length} remembered approval(s)`,detail:listing+(unavailable?`\n\n${unavailable} older or unavailable connection file(s).`:'')+'\n\nTo change locked access, sign in as the approving Administrator and open the approved company. '+EXPLAIN_LOCKED,buttons:['Done','Forget all approvals and stop local API',...(eligible.length?['Change locked access…']:[])],defaultId:0,cancelId:0});
      if(choice.response===1 && !busy()){
        const now=JSON.parse(native.appState());
        let scope;
        try{if(now.open && !now.viewing_restored && now.path)scope=approval.scope(now.path);}catch{}
        await forget(records.some(({record})=>record.book_scope!==scope));
      }
      if(choice.response===2 && !busy())await changeLockedAccess(eligible,c,epoch);
    }catch{
      await dialog.showMessageBox(window(),{type:'error',message:'AI connections could not be updated',detail:'Open the approved company, sign in as the approving Administrator and check that the desktop keyring is unlocked.'});
    }finally{working=false;}
  }
  return {connect,manage,resume,start,stop,menuItems:[{id:'ai-connect',label:'Connect an AI assistant…',click:()=>void connect()},{id:'ai-manage',label:'Manage AI connections…',click:()=>void manage()}]};
}
module.exports={create};

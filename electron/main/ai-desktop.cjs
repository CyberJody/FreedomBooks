'use strict';
// Remember consent in the OS store; every live capability still belongs to one authenticated session.
const fs=require('node:fs'),path=require('node:path');
const approval=require('./ai-approval.cjs');
function create({app,native,dialog,clipboard,safeStorage,root,window,exclusive,busy}) {
  const directory=path.join(root,'ai-connections');
  const active=new Map();let working=false,restoring=null,suspended=false;
  const usable=()=>safeStorage.isEncryptionAvailable() && (process.platform!=='linux' || safeStorage.getSelectedStorageBackend()!=='basic_text');
  function files() {
    if(!fs.existsSync(directory))return [];
    return fs.readdirSync(directory).filter(n=>approval.UUID.test(n.replace(/\.sealed$/,'')) && n.endsWith('.sealed')).sort();
  }
  // Global pages retain the authorized company frame even though their URL has no company ID.
  function company() {return native.aiCurrentCompany();}
  function context(co) {
    const state=JSON.parse(native.appState());
    if(!co || !state.open || !state.signed_in)throw Error('Sign in and open a company before connecting an AI.');
    if(!usable())throw Error('Unlock your desktop keyring before connecting an AI.');
    const c=JSON.parse(native.aiContext(co));
    if(c.restored)throw Error('AI assistants are unavailable in a restored-book preview. Reopen your main books to connect.');
    return {...c,book_scope:approval.scope(c.book_path)};
  }
  function matches(r,c) {return r.book_scope===c.book_scope && r.company_id===c.company_id && r.actor_id===c.actor_id && (r.permission==='read_only' || ['administrator','bookkeeper'].includes(c.role));}
  const config=file=>({mcpServers:{freedombooks:{command:process.execPath,args:app.isPackaged?['--ai-mcp',file]:[path.resolve(__dirname,'..'),'--ai-mcp',file]}}});
  async function showConfiguration(file) {
    const text=JSON.stringify(config(file),null,2);
    const choice=await dialog.showMessageBox(window(),{type:'info',title:'AI assistant connected',message:'Company approval saved on this computer',detail:text+'\n\nAdd this configuration to your AI client once. The model can run locally or in the cloud. After a normal restart, sign in and open this company; FreedomBooks reconnects without another permission prompt. Lock or sign-out stops access. Manage AI connections can forget the approval.',buttons:['Done','Copy MCP configuration'],defaultId:0});
    if(choice.response===1)await clipboard.writeText(text);
  }
  async function resume(co) {
    if(working || busy() || suspended)return;
    if(restoring)return restoring;
    restoring=(async()=>{
      let c,epoch;try{c=context(co);epoch=approval.epoch(root,safeStorage);}catch{return;}
      const generation=native.aiGeneration();
      const live=new Set(JSON.parse(native.aiConnections()).map(g=>g.id));
      for(const name of files().slice(0,32)) {
        const file=path.join(directory,name);let r;
        try{r=approval.read(file,safeStorage);}catch{continue;} // Legacy session files never invent remembered consent.
        if(r.epoch!==epoch || !matches(r,c))continue;
        const previous=active.get(r.id);
        if(previous?.generation===generation && live.has(previous.grant))continue;
        let grant;
        try {
          const endpoint=await transport.start();
          if(busy() || generation!==native.aiGeneration())return;
          const now=context(co);if(!matches(r,now) || now.session_generation!==c.session_generation || approval.epoch(root,safeStorage)!==r.epoch)return;
          if(previous)native.aiRevoke(previous.grant);
          grant=JSON.parse(native.aiGrant(co,r.name,r.permission));
          if(grant.company_id!==r.company_id || grant.actor_id!==r.actor_id || grant.permission!==r.permission)throw Error('Session changed');
          approval.write(file,{...r,endpoint,token:grant.token},safeStorage);
          active.set(r.id,{generation:native.aiGeneration(),grant:grant.id});
        } catch {if(grant)native.aiRevoke(grant.id);} finally {if(grant)grant.token='';}
      }
    })().catch(()=>{}).finally(()=>{restoring=null;}); // Optional AI storage must not hide a book page.
    return restoring;
  }
  const transport=require('./ai-transport.cjs').create({
    generation:()=>native.aiGeneration(),
    request:async(token,body)=>{await resume(company());return exclusive(()=>native.aiRequestAsync(token,body));},
  });
  async function connect() {
    if(working || busy())return;
    await resume(company());if(working || busy())return;
    working=true;let grant,file,created=false;
    try {
      const co=company(),c=context(co),generation=native.aiGeneration(),epoch=suspended?approval.rotateEpoch(root,safeStorage):approval.epoch(root,safeStorage,{create:true});
      for(const name of files()) {
        const existing=path.join(directory,name);let r;try{r=approval.read(existing,safeStorage);}catch{continue;}
        if(r.epoch===epoch && matches(r,c) && active.has(r.id)){await showConfiguration(existing);return;}
      }
      if(files().length>=32)throw Error('Forget unused AI connections before adding another.');
      const selected=await dialog.showMessageBox(window(),{type:'question',title:'Connect an AI assistant',message:`Allow an AI assistant to work in ${c.company_name}?`,detail:`This approval is for ${c.actor_name} in this company and these books on this computer. It is remembered until you forget it in Manage AI connections.\n\nThe model can run locally or in the cloud; its connector runs on this computer. Read only permits queries and previews. Accounting write also permits supported posting and imports. Access requires your completed sign-in, including MFA, and pauses on lock, sign-out or company change. A normal restart does not require approval again.`,buttons:['Cancel','Read only','Accounting write'],defaultId:1,cancelId:0});
      if(selected.response===0)return;
      if(busy() || generation!==native.aiGeneration() || co!==company() || approval.epoch(root,safeStorage)!==epoch)throw Error('Session changed');
      const now=context(co);if(now.actor_id!==c.actor_id || now.book_scope!==c.book_scope || now.session_generation!==c.session_generation)throw Error('Session changed');
      const endpoint=await transport.start();
      if(busy() || generation!==native.aiGeneration() || co!==company())throw Error('Session changed');
      grant=JSON.parse(native.aiGrant(co,`AI assistant ${new Date().toISOString()}`,selected.response===1?'read_only':'accounting_write'));
      file=path.join(directory,grant.id+'.sealed');
      approval.write(file,{version:2,id:grant.id,epoch,book_scope:c.book_scope,company_id:co,actor_id:grant.actor_id,name:grant.name || `AI assistant ${new Date().toISOString()}`,permission:grant.permission,approved_at:new Date().toISOString(),endpoint,token:grant.token},safeStorage,{create:true});created=true;
      active.set(grant.id,{generation:native.aiGeneration(),grant:grant.id});
      suspended=false;
      await showConfiguration(file);
    } catch(error) {
      if(grant){native.aiRevoke(grant.id);active.delete(grant.id);if(created)try{fs.unlinkSync(file);}catch{}}
      await dialog.showMessageBox(window(),{type:'error',message:'AI connection was not created',detail:['Sign in and open a company before connecting an AI.','Unlock your desktop keyring before connecting an AI.','Forget unused AI connections before adding another.','AI assistants are unavailable in a restored-book preview. Reopen your main books to connect.'].includes(error.message)?error.message:'Check that you are signed in, the company is open and the desktop keyring is unlocked.'});
    } finally {if(grant)grant.token='';working=false;}
  }
  async function manage() {
    if(busy() || working)return;
    const saved=[];let legacy=0;
    const names=files();legacy=Math.max(0,names.length-32);
    for(const name of names.slice(0,32))try{saved.push(approval.read(path.join(directory,name),safeStorage));}catch{legacy++;}
    const text=saved.map(r=>`${r.name}\n${r.id} · ${r.permission}\nCompany ${r.company_id} · User ${r.actor_id}`).join('\n\n') || 'No remembered AI approvals.';
    const choice=await dialog.showMessageBox(window(),{type:'info',title:'AI assistant connections',message:`${saved.length} remembered approval(s)`,detail:text+(legacy?`\n\n${legacy} older or unavailable connection file(s). Older session-only connections are not automatically approved.`:'')+'\n\nLock and sign-out pause access. Signing back in to the same company resumes it. Forgetting approvals stops reconnection.',buttons:['Done','Forget all approvals and stop local API'],defaultId:0,cancelId:0});
    if(choice.response===1 && !busy()){
      suspended=true;
      // Rotate durably before deleting files so restoring an old connection file cannot undo Forget.
      try{approval.rotateEpoch(root,safeStorage);}catch{native.aiInvalidate();active.clear();await transport.stop();await dialog.showMessageBox(window(),{type:'error',message:'Access stopped, but forgetting approval could not be saved',detail:'Unlock the desktop keyring and try Forget all approvals again before restarting.'});return;}
      native.aiInvalidate();let removalFailed=false;
      try{for(const name of files())try{fs.unlinkSync(path.join(directory,name));}catch{removalFailed=true;}}
      finally{active.clear();await transport.stop();}
      suspended=false;
      if(removalFailed)await dialog.showMessageBox(window(),{type:'warning',message:'Approvals forgotten; some retired files could not be removed',detail:'Those files cannot restore approval. You can remove them later when the folder is writable.'});
    }
  }
  return {connect,manage,resume,stop:()=>transport.stop(),menuItems:[{label:'Connect an AI assistant…',click:()=>void connect()},{label:'Manage AI connections…',click:()=>void manage()}]};
}
module.exports={create};

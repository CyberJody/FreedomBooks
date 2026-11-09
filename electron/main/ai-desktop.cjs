'use strict';
// Native setup controls only: no secret is passed to a page, preload, generic IPC, or clipboard.
const fs=require('node:fs');
const path=require('node:path');
function create({app,native,dialog,clipboard,safeStorage,root,window,exclusive,busy}) {
  const transport=require('./ai-transport.cjs').create({
    generation:()=>native.aiGeneration(),
    request:(token,body)=>exclusive(()=>native.aiRequestAsync(token,body)),
  });
  const directory=path.join(root,'ai-connections');
  let working=false;
  function usable() {return safeStorage.isEncryptionAvailable() && (process.platform!=='linux' || safeStorage.getSelectedStorageBackend()!=='basic_text');}
  function files() {
    if(!fs.existsSync(directory))return [];
    return fs.readdirSync(directory).filter(n=>/^[0-9a-f-]{36}\.sealed$/.test(n));
  }
  function removeFiles() {for(const f of files())fs.unlinkSync(path.join(directory,f));}
  function config(file) {
    return {mcpServers:{freedombooks:{command:process.execPath,args:app.isPackaged?['--ai-mcp',file]:[path.resolve(__dirname,'..'),'--ai-mcp',file]}}};
  }
  async function connect() {
    if(working || busy())return;
    working=true;
    let grant;
    try {
      const match=/^fb:\/\/app\/c\/([0-9a-f-]{36})\//.exec(window()?.webContents.getURL() || '');
      if(!match || !JSON.parse(native.appState()).open) throw new Error('Open a company page before connecting an AI.');
      if(!usable())throw new Error('An OS key store is required for AI connections. Unlock your desktop keyring, then try again.');
      const generation=native.aiGeneration();
      const selected=await dialog.showMessageBox(window(),{type:'question',title:'Connect an AI assistant',message:'Choose permission for this company',detail:'The AI model can run on this computer or in the cloud. Its client or connector must run on this computer.\n\nThe connection acts as the signed-in operator in the company currently displayed. Read only permits queries, reports and import previews. Accounting write also permits supported accounting operations, including posting and imports, without a prompt for each call. Lock, sign-out or company change revokes the connection. It expires after eight hours.',buttons:['Cancel','Read only','Accounting write'],defaultId:1,cancelId:0});
      if(selected.response===0)return;
      if(busy())throw new Error('Wait for the current operation to finish.');
      if(generation!==native.aiGeneration() || !window()?.webContents.getURL().startsWith(`fb://app/c/${match[1]}/`)) throw new Error('Session changed.');
      const endpoint=await transport.start();
      if(generation!==native.aiGeneration()) throw new Error('Session changed.');
      grant=JSON.parse(native.aiGrant(match[1],`AI assistant ${new Date().toISOString()}`,selected.response===1?'read_only':'accounting_write'));
      fs.mkdirSync(directory,{recursive:true,mode:0o700});
      const file=path.join(directory,grant.id+'.sealed');
      const fd=fs.openSync(file,'wx',0o600);
      try {fs.writeFileSync(fd,safeStorage.encryptString(JSON.stringify({endpoint,token:grant.token})));fs.fsyncSync(fd);} finally {fs.closeSync(fd);}
      // Clipboard contains ONLY the launcher command and the OS-sealed file path.
      const text=JSON.stringify(config(file),null,2);
      const choice=await dialog.showMessageBox(window(),{type:'info',title:'AI assistant connected',message:'Add this MCP connection to your AI client',detail:text+'\n\nUse an AI client or connector running on this computer. Its model can run locally or in the cloud. Keep FreedomBooks open in this company. Reconnect after a session change. See docs/local-ai.md for local API usage and the operation coverage.',buttons:['Done','Copy MCP configuration'],defaultId:1});
      if(choice.response===1)await clipboard.writeText(text);
    } catch(error) {
      if(grant){native.aiRevoke(grant.id);try{fs.unlinkSync(path.join(directory,grant.id+'.sealed'));}catch{}}
      await dialog.showMessageBox(window(),{type:'error',message:'AI connection was not created',detail:['Open a company page before connecting an AI.','An OS key store is required for AI connections. Unlock your desktop keyring, then try again.','Wait for the current operation to finish.'].includes(error.message)?error.message:'The connection could not be saved securely. Try again after unlocking the OS key store.'});
    } finally {if(grant)grant.token='';working=false;}
  }
  async function manage() {
    if(busy() || working)return;
    const grants=JSON.parse(native.aiConnections());
    const text=grants.map(g=>`${g.name}\n${g.id} · ${g.permission}\nCompany ${g.company_id} · Operator ${g.actor_id}`).join('\n\n') || 'No active AI connections.';
    const choice=await dialog.showMessageBox(window(),{type:'info',title:'AI assistant connections',message:`${grants.length} active connection(s)`,detail:text,buttons:['Done','Revoke all and stop local API'],defaultId:0,cancelId:0});
    if(choice.response===1 && !busy()){native.aiInvalidate();removeFiles();await transport.stop();}
  }
  return {connect,manage,stop:()=>transport.stop(),menuItems:[{label:'Connect an AI assistant…',click:()=>void connect()},{label:'Manage AI connections…',click:()=>void manage()}]};
}
module.exports={create};

'use strict';
// The launcher shares the desktop's OS key store and reads its live endpoint without opening books.
function start(file,apiMode=false) {
  const {app,safeStorage}=require('electron');
  const fs=require('node:fs'),path=require('node:path');
  const identity=require('./identity.cjs');
  const approval=require('./ai-approval.cjs');
  const {refusal}=require('../agent/refusal.cjs');
  const override=require('./development.cjs').userData(app);
  identity.configure(app);
  app.setPath('userData',override || identity.settingsDir(app));
  // A tool launched by a service may have a different XDG data home. The configured file names
  // the profile whose sealed epoch and live endpoint it uses.
  const sealed=typeof file==='string'?path.resolve(file):null;
  const directory=sealed && path.dirname(sealed);
  const root=directory && path.dirname(directory);
  app.whenReady().then(()=>{
    const load=()=>{
      if (!approval.usable(safeStorage)) throw refusal('keyring_unavailable');
      if (!root || path.basename(directory)!=='ai-connections' || !fs.existsSync(sealed)) throw refusal('approval_gone');
      let epoch;
      try {epoch=approval.epoch(root,safeStorage);}
      catch (e) {throw refusal(e.code==='ENOENT'?'approval_gone':'keyring_unavailable');}
      let saved;
      try {saved=approval.read(sealed,safeStorage);}
      catch {throw refusal('approval_gone');}
      if (saved.epoch!==epoch) throw refusal('approval_gone');
      const live=approval.runtime(root,safeStorage);
      return {id:saved.id,token:saved.token,...live};
    };
    const server=require('../agent/mcp.cjs').serve(load,process.stdin,process.stdout,{apiMode});
    const finish=()=>{server.done().finally(()=>app.quit());};
    process.stdin.on('end',finish);
    process.stdin.on('close',finish);
    process.stdin.resume();
  }).catch(()=>{app.exit(1);});
}
module.exports={start};

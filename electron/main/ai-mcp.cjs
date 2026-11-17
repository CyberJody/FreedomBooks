'use strict';
// Runs as an Electron utility invocation so the same OS key store can unseal the grant.
function start(file,apiMode=false) {
  const {app,safeStorage}=require('electron');
  const identity=require('./identity.cjs');
  identity.configure(app);
  // Match the desktop's OSCrypt/keyring profile without taking its instance lock.
  app.setPath('userData',require('./development.cjs').userData(app) || identity.settingsDir(app));
  app.whenReady().then(()=>{
    if (!safeStorage.isEncryptionAvailable() || (process.platform==='linux' && safeStorage.getSelectedStorageBackend()==='basic_text')) throw new Error('OS store unavailable');
    // Reload before each call: a normal restart rotates the endpoint and session token in this file.
    const load=()=>require('./ai-approval.cjs').read(file,safeStorage,{legacy:true});
    load();
    const server=require('../agent/mcp.cjs').serve(load,process.stdin,process.stdout,{apiMode});
    const finish=()=>{server.done().finally(()=>app.quit());};
    process.stdin.on('end',finish);
    process.stdin.on('close',finish);
    process.stdin.resume();
  }).catch(()=>{process.stderr.write('FreedomBooks connection unavailable. Open FreedomBooks, sign in to the approved company and unlock the desktop keyring. Forgotten or older session-only connections require setup in Settings.\n');app.exit(1);});
}
module.exports={start};

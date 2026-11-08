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
    const fs=require('node:fs');
    const fd=fs.openSync(file,fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW || 0));
    let sealed;
    try {const s=fs.fstatSync(fd);if(!s.isFile() || s.size>16384 || (process.platform!=='win32' && ((s.mode&0o077)!==0 || s.uid!==process.getuid()))) throw new Error('Invalid connection file');sealed=fs.readFileSync(fd);} finally {fs.closeSync(fd);}
    const config=JSON.parse(safeStorage.decryptString(sealed));
    const server=require('../agent/mcp.cjs').serve(config,process.stdin,process.stdout,{apiMode});
    const finish=()=>{server.done().finally(()=>app.quit());};
    process.stdin.on('end',finish);
    process.stdin.on('close',finish);
    process.stdin.resume();
  }).catch(()=>{process.stderr.write('FreedomBooks connection unavailable. Reconnect from the desktop.\n');app.exit(1);});
}
module.exports={start};

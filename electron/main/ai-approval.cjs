'use strict';
// Trusted main/utility code only. No page, preload, generic IPC or agent operation exposes this store.
const fs=require('node:fs'),path=require('node:path'),crypto=require('node:crypto');
const {endpoint}=require('../agent/mcp.cjs');
const {refusal}=require('../agent/refusal.cjs');
const UUID=/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const HEX=/^[0-9a-f]{64}$/;
const fields=['version','id','epoch','book_scope','company_id','actor_id','name','permission','approved_at','endpoint','token'];
function scope(book) {
  const real=fs.realpathSync(book),s=fs.statSync(real,{bigint:true});
  if(!s.isFile())throw Error('Invalid book scope');
  // A copied/restored/replaced file is a new approval scope, even when its company UUIDs match.
  return crypto.createHash('sha256').update(JSON.stringify(['FreedomBooks AI approval v1',real,String(s.dev),String(s.ino)])).digest('hex');
}
function validate(v,file,legacy=false) {
  if(!v || typeof v!=='object' || Array.isArray(v) || !HEX.test(v.token))throw Error('Invalid connection');
  endpoint(v.endpoint);
  if(legacy && v.version===undefined && Object.keys(v).every(k=>['endpoint','token'].includes(k)))return v;
  if(Object.keys(v).length!==fields.length+Number(Object.hasOwn(v,'unattended'))+Number(Object.hasOwn(v,'locked_access_off')) || fields.some(k=>!Object.hasOwn(v,k)) || v.version!==2 || !UUID.test(v.id) || !UUID.test(v.epoch) || path.basename(file)!==v.id+'.sealed' || !HEX.test(v.book_scope) || !UUID.test(v.company_id) || !UUID.test(v.actor_id) || !['read_only','accounting_write'].includes(v.permission) || typeof v.name!=='string' || !v.name.trim() || v.name.length>80 || /[\x00-\x1f\x7f]/.test(v.name) || typeof v.approved_at!=='string' || v.approved_at.length>40 || !Number.isFinite(Date.parse(v.approved_at)))throw Error('Invalid saved approval');
  if(Object.hasOwn(v,'locked_access_off')){
    const off=v.locked_access_off;
    if(v.unattended || !off || Object.keys(off).length!==2 || !['disabled','revoked'].includes(off.reason) || typeof off.at!=='string' || off.at.length>40 || !Number.isFinite(Date.parse(off.at)))throw Error('Invalid locked access status');
  }
  if (Object.hasOwn(v,'unattended')) {
    const p = v.unattended;
    if (!p || Object.keys(p).length!==7 || p.id!==v.id || p.epoch!==v.epoch || p.actor!==v.actor_id || p.company!==v.company_id ||
        p.name!==v.name || p.permission!==v.permission || !HEX.test(p.secret)) throw Error('Invalid unattended proof');
  }
  return v;
}
function unseal(file,storage) {
  const fd=fs.openSync(file,fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW || 0));
  let bytes;
  try {
    const s=fs.fstatSync(fd);
    if(!s.isFile() || s.size>16384 || (process.platform!=='win32' && ((s.mode&0o077)!==0 || s.uid!==process.getuid())))throw Error('Invalid connection file');
    bytes=fs.readFileSync(fd);
  } finally {fs.closeSync(fd);}
  return JSON.parse(storage.decryptString(bytes));
}
function read(file,storage,{legacy=false}={}) {return validate(unseal(file,storage),file,legacy);}
function atomic(file,value,storage) {
  fs.mkdirSync(path.dirname(file),{recursive:true,mode:0o700});
  const temp=file+'.tmp-'+crypto.randomUUID();let fd;
  try {
    fd=fs.openSync(temp,'wx',0o600);fs.writeFileSync(fd,storage.encryptString(JSON.stringify(value)));fs.fsyncSync(fd);fs.closeSync(fd);fd=undefined;
    fs.renameSync(temp,file);
    if(process.platform!=='win32'){const d=fs.openSync(path.dirname(file),fs.constants.O_RDONLY);try{fs.fsyncSync(d);}finally{fs.closeSync(d);}}
  } finally {if(fd!==undefined)fs.closeSync(fd);fs.rmSync(temp,{force:true});}
}
function write(file,value,storage,{create=false}={}) {
  validate(value,file);
  if(create && fs.existsSync(file))throw Error('Connection already exists');
  if(fs.existsSync(file))read(file,storage,{legacy:true});
  atomic(file,value,storage);
}
const epochFile=root=>path.join(root,'ai-connections','.approval-epoch.sealed');
function rotateEpoch(root,storage) {
  const value=crypto.randomUUID();atomic(epochFile(root),{version:1,epoch:value},storage);return value;
}
function epoch(root,storage,{create=false}={}) {
  let v;try{v=unseal(epochFile(root),storage);}catch(e){if(create && e.code==='ENOENT')return rotateEpoch(root,storage);throw e;}
  if(!v || Object.keys(v).length!==2 || v.version!==1 || !UUID.test(v.epoch))throw Error('Invalid approval epoch');
  return v.epoch;
}
const runtimeFile=root=>path.join(root,'ai-connections','.endpoint.sealed');
function runtime(root,storage) {
  let value;
  try { value=unseal(runtimeFile(root),storage); endpoint(value.endpoint); }
  catch { throw refusal('app_not_running'); }
  if (!HEX.test(value.cookie)) throw refusal('app_not_running');
  return value;
}
function publish(root,storage,value) {atomic(runtimeFile(root),value,storage);}
function unpublish(root,storage,cookie) {
  try {if(runtime(root,storage).cookie===cookie)fs.rmSync(runtimeFile(root),{force:true});}catch{/* A stopped endpoint also fails its challenge. */}
}
function usable(storage) {
  try{return storage.isEncryptionAvailable() && (process.platform!=='linux' || storage.getSelectedStorageBackend()!=='basic_text');}
  catch{return false;}
}
module.exports={scope,read,write,validate,epoch,rotateEpoch,runtime,publish,unpublish,usable,UUID};

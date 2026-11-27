'use strict';
const {randomBytes,createCipheriv}=require('node:crypto');
const fs=require('node:fs'),path=require('node:path');

function seedConnectionKey(profile, seed, fixture) {
  const nonce=randomBytes(12),cipher=createCipheriv('aes-256-gcm',Buffer.alloc(32,seed),nonce);
  const ciphertext=Buffer.concat([cipher.update(fixture.device_hex,'utf8'),cipher.final()]);
  fs.writeFileSync(path.join(profile,'connection-key.sealed'),Buffer.concat([nonce,cipher.getAuthTag(),ciphertext]),{mode:0o600,flag:'wx'});
}
module.exports={seedConnectionKey};

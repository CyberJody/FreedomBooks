// Real Electron flows, synthetic books, two independent HOME directories per restore.
'use strict';
const {_electron:electron}=require('playwright');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const {spawnSync}=require('node:child_process');
const root=path.resolve(__dirname,'..');
const PASSWORD='Synthetic smoke owner password';

async function main() {
  assert.ok(process.env.DISPLAY && ![':0',':20',':0.0',':20.0'].includes(process.env.DISPLAY),'isolated xvfb display required');
  const base=fs.mkdtempSync(path.join(process.env.TMPDIR,'fb-keys-smoke-'));
  const shots=process.env.FB_SMOKE_SHOTS || path.join(base,'screenshots');
  fs.mkdirSync(shots,{recursive:true});
  let checks=0;
  const pass=s=>console.log(`PASS KEYS ${String(++checks).padStart(2,'0')}: ${s}`);
  const sessions=new Set();
  async function launch(name, keyring=false) {
    const home=path.join(base,name),data=path.join(home,'data');
    fs.mkdirSync(home,{recursive:true,mode:0o700});
    let entry=root;
    if (keyring) {
      entry=path.join(home,'test-keyring.cjs');
      fs.writeFileSync(entry,`const e=require(${JSON.stringify(path.join(root,'node_modules/electron'))});\n`,{mode:0o600});
      // This wrapper runs only in the test, with an intentionally synthetic keyring key.
      fs.writeFileSync(entry,`const {safeStorage}=require('electron');const c=require('node:crypto');\nObject.assign(safeStorage,{isEncryptionAvailable:()=>true,getSelectedStorageBackend:()=> 'gnome_libsecret',encryptString:s=>{const n=c.randomBytes(12),x=c.createCipheriv('aes-256-gcm',Buffer.alloc(32,19),n);const b=Buffer.concat([x.update(s,'utf8'),x.final()]);return Buffer.concat([n,x.getAuthTag(),b]);},decryptString:b=>{const x=c.createDecipheriv('aes-256-gcm',Buffer.alloc(32,19),b.subarray(0,12));x.setAuthTag(b.subarray(12,28));return Buffer.concat([x.update(b.subarray(28)),x.final()]).toString('utf8');}});require(${JSON.stringify(path.join(root,'main/main.cjs'))});\n`,{mode:0o600});
    }
    const env={...process.env,HOME:home,XDG_CONFIG_HOME:path.join(home,'config'),XDG_DATA_HOME:path.join(home,'share'),XDG_CACHE_HOME:path.join(home,'cache'),FB_DESKTOP_USER_DATA:data,ELECTRON_ENABLE_LOGGING:'0'};
    const app=await electron.launch({args:[entry,'--password-store=basic'],cwd:root,env,timeout:180000});
    sessions.add(app);
    // Turn native error boxes into test failures instead of leaving an unattended modal open.
    await app.evaluate(({dialog})=>{dialog.showErrorBox=(_title,message)=>{throw new Error(message);};});
    const page=await app.firstWindow();page.setDefaultTimeout(180000);
    await page.setViewportSize({width:1440,height:1000});
    await page.waitForSelector('h1');
    const native=async(method,args=[])=>app.evaluate((_e,{modulePath,method,args})=>process.mainModule.require(modulePath)[method](...args),{modulePath:path.join(root,'native/fb-node.node'),method,args});
    return {app,page,data,home,native};
  }
  async function close(s) { await s.app.close();sessions.delete(s.app); }
  async function shot(s,name,mask=[]) { await s.page.screenshot({path:path.join(shots,'keys-'+name+'.png'),mask}); }
  async function create(s,own) {
    await s.page.fill('#id_owner_name','Synthetic owner');
    await s.page.fill('#id_company_name',own?'Synthetic protected books':'Synthetic built-in books');
    if(own){await s.page.check('#id_protect');await s.page.fill('#id_lock_password',PASSWORD);await s.page.fill('#id_lock_password2',PASSWORD);}
    await s.page.getByRole('button',{name:'Create books',exact:true}).click();
    await s.page.waitForURL(own?/\/settings\/recovery-key\/$/:/\/home\/$/);
  }
  async function goHome(s) {
    const companies=JSON.parse(await s.native('query',[JSON.stringify({type:'Companies'})]));
    await s.page.goto('fb://app/c/'+companies[0].id+'/home/');
  }
  async function makeBackup(s) {
    const companies=JSON.parse(await s.native('query',[JSON.stringify({type:'Companies'})]));
    await s.page.goto('fb://app/c/'+companies[0].id+'/backups/');
    await s.page.getByRole('button',{name:'Create backup now',exact:true}).click();
    await s.page.waitForURL(/\/backups\/\?created=/);
    const file=path.join(s.data,'backups',new URL(s.page.url()).searchParams.get('created'));
    assert.ok(fs.existsSync(file+'.verified.json'));
    return file;
  }
  async function stub(s,method,result) { await s.app.evaluate(({dialog},[m,r])=>{dialog[m]=async()=>r;},[method,result]); }
  async function gateRestore(s,file,secret,name) {
    await s.page.locator('summary').filter({hasText:'Restore a backup'}).click();
    await s.page.fill('#id_restore_name',name);
    await s.page.fill('#id_restore_secret',secret);
    await stub(s,'showOpenDialog',{canceled:false,filePaths:[file]});
    await s.page.getByRole('button',{name:'Choose a backup to restore…'}).click();
  }
  try {
    let builtin=await launch('builtin-source');
    assert.equal(await builtin.page.isChecked('#id_protect'),false);
    assert.match(await builtin.page.locator('main').innerText(),/Anyone who gets a copy of this file can open it\./);
    assert.doesNotMatch(await builtin.page.locator('main').innerText(),/encrypted/i);
    await shot(builtin,'01-default-setup');
    await create(builtin,false);
    assert.equal(JSON.parse(await builtin.native('appState')).can_lock,false);
    assert.equal(await builtin.app.evaluate(({Menu})=>Menu.getApplicationMenu().getMenuItemById('lock-books').enabled),false);
    await builtin.page.goto('fb://app/settings/book-protection/');
    assert.doesNotMatch(await builtin.page.locator('main').innerText(),/encrypted/i);
    await shot(builtin,'02-builtin-settings');
    await close(builtin);builtin=await launch('builtin-source');
    await builtin.page.waitForURL(/\/home\/$/);
    pass('unchecked default opens straight to Home after restart; File and idle locks do not apply');
    const builtinBackup=await makeBackup(builtin);
    assert.doesNotMatch(await builtin.page.locator('main').innerText(),/encrypted/i);
    await shot(builtin,'03-builtin-backups');
    let other=await launch('builtin-second');
    await gateRestore(other,builtinBackup,'','Portable built-in');
    await other.page.waitForURL(/\/home\/$/);
    assert.equal(JSON.parse(await other.native('appState')).builtin,true);
    await close(other);other=await launch('builtin-second');
    await other.page.waitForURL(/\/home\/$/);await close(other);
    pass('built-in backup restores and reopens in a second isolated HOME without a key store');

    let own=await launch('own-source');await create(own,true);
    const recovery=(await own.page.locator('.recovery-key').innerText()).trim();
    assert.match(recovery,/^[0-9A-Z]{5}(?:-[0-9A-Z]{5}){6}$/);
    await shot(own,'04-recovery-dialog',[own.page.locator('.recovery-key')]);
    const textFile=path.join(base,'Emergency Kit.txt'),pdfFile=path.join(base,'Emergency Kit.pdf');
    await stub(own,'showSaveDialog',{canceled:false,filePath:textFile});
    await Promise.all([own.page.waitForEvent('load'), own.page.getByRole('button',{name:'Save to USB / file…'}).click()]);
    assert.equal(fs.statSync(textFile).mode & 0o777,0o600);
    assert.ok(fs.readFileSync(textFile,'utf8').includes(recovery));
    await own.page.getByRole('button',{name:'Print',exact:true}).click();
    await own.page.waitForURL(/\/settings\/emergency-kit\/$/);
    await shot(own,'05-emergency-kit',[own.page.locator('.recovery-key')]);
    await stub(own,'showSaveDialog',{canceled:false,filePath:pdfFile});
    await own.app.evaluate(async({Menu,BrowserWindow})=>{const file=Menu.getApplicationMenu().items.find(i=>i.label==='File');await file.submenu.items.find(i=>i.label==='Save as PDF…').click(null,BrowserWindow.getAllWindows()[0]);});
    // The menu handler is fire-and-forget; wait for the atomic rename to publish the PDF.
    for(let i=0;i<600&&!fs.existsSync(pdfFile);i++)await new Promise(r=>setTimeout(r,100));
    assert.equal(fs.statSync(pdfFile).mode & 0o777,0o600);
    const pdfText=spawnSync('pdftotext',[pdfFile,'-'],{encoding:'utf8'});
    assert.equal(pdfText.status,0);assert.ok(pdfText.stdout.includes('FreedomBooks Emergency Kit'));assert.ok(pdfText.stdout.includes(recovery));
    pass('recovery key has 150 random bits; Emergency Kit prints to PDF and saves atomically as 0600');
    await goHome(own);
    await own.app.evaluate(({Menu})=>Menu.getApplicationMenu().getMenuItemById('lock-books').click());
    await own.page.waitForSelector('#id_password');await shot(own,'06-locked');
    await own.page.fill('#id_password','wrong password');await own.page.getByRole('button',{name:'Unlock',exact:true}).click();
    await own.page.waitForSelector('.errorlist');assert.equal(JSON.parse(await own.native('appState')).open,false);
    assert.equal((await own.page.locator('.errorlist').innerText()).trim(),"That password or key doesn't open these books. Check it and try again, or use your recovery key.");
    const wrongKeyCategory = await own.app.evaluate((_electron, {nativePath, storePath, storeRoot}) => {
      const native = process.mainModule.require(nativePath);
      try { native.openProtectedBook(process.mainModule.require(storePath).store(storeRoot).bookPath(), 'wrong password'); }
      catch (error) { return {code: error.code, message: error.message}; }
    }, {nativePath:path.join(root,'native/fb-node.node'),storePath:path.join(root,'main/keys.cjs'),storeRoot:own.data});
    assert.deepEqual(wrongKeyCategory,{code:'FB_BOOK_WRONG_KEY',message:"That password or key doesn't open these books. Check it and try again, or use your recovery key."});
    await shot(own,'06a-wrong-password');
    await own.page.fill('#id_password',PASSWORD);await own.page.getByRole('button',{name:'Unlock',exact:true}).click();
    await own.page.waitForURL(/\/home\/$/);
    const ownBackup=await makeBackup(own);await shot(own,'07-protected-backups');
    for(const file of fs.readdirSync(path.dirname(ownBackup)).map(n=>path.join(path.dirname(ownBackup),n))){const b=fs.readFileSync(file);assert.ok(!b.includes(Buffer.from(PASSWORD)));assert.ok(!b.includes(Buffer.from(recovery)));}
    pass('own-password lock rejects a wrong password; backup and verification sidecar contain no password or recovery text');
    for(const [name,secret] of [['own-password-second',PASSWORD],['own-recovery-second',recovery]]) {
      const target=await launch(name);
      await gateRestore(target,ownBackup,'wrong password','Portable protected');
      await target.page.waitForSelector('.errorlist');assert.ok(!fs.existsSync(path.join(target.data,'books/restored-portable-protected')));
      await target.page.locator('summary').filter({hasText:'Restore a backup'}).click();
      await target.page.fill('#id_restore_name','Portable protected');await target.page.fill('#id_restore_secret',secret);
      await target.page.getByRole('button',{name:'Choose a backup to restore…'}).click();
      await target.page.waitForURL(/\/home\/$/);
      assert.equal(JSON.parse(await target.native('appState')).builtin,false);
      await close(target);
    }
    pass('password and recovery key independently restore in fresh HOMEs; wrong secrets create no book');
    await builtin.page.goto('fb://app/settings/book-protection/');
    await builtin.page.fill('#id_new_password',PASSWORD);await builtin.page.fill('#id_repeat_password',PASSWORD);
    await builtin.page.getByRole('button',{name:'Add my own password',exact:true}).click();
    await builtin.page.waitForURL(/\/settings\/recovery-key\/$/);
    await builtin.page.getByRole('button',{name:'Remind me later'}).click();
    await builtin.page.waitForURL(/\/settings\/book-protection\/$/);
    await builtin.page.fill('#id_remove_password',PASSWORD);await builtin.page.check('input[name=understand]');
    await builtin.page.getByRole('button',{name:'Remove my own password',exact:true}).click();
    await builtin.page.waitForSelector('#id_new_password');
    assert.equal(JSON.parse(await builtin.native('appState')).builtin,true);
    pass('Settings switches built-in → own password → built-in after verified safety backups');
    await own.page.goto('fb://app/settings/book-protection/');
    await own.page.uncheck('input[name=ask_password]');await own.page.fill('#id_current_password',PASSWORD);
    await own.page.getByRole('button',{name:'Save setting'}).click();await own.page.waitForSelector('.errorlist');
    assert.match(await own.page.locator('.errorlist').innerText(),/no system keyring/);
    await shot(own,'08-no-keyring-refusal');
    let auto=await launch('own-auto-open',true);await create(auto,true);
    await auto.page.getByRole('button',{name:'Remind me later'}).click();await auto.page.waitForURL(/\/settings\/book-protection\/$/);
    await auto.page.uncheck('input[name=ask_password]');await auto.page.fill('#id_current_password',PASSWORD);
    await Promise.all([auto.page.waitForEvent('load'),auto.page.getByRole('button',{name:'Save setting'}).click()]);
    assert.equal(JSON.parse(await auto.native('appState')).can_lock,false);
    await close(auto);auto=await launch('own-auto-open',true);await auto.page.waitForURL(/\/home\/$/);
    await close(auto);
    pass('own-password auto-open works with an isolated test keyring; basic_text refuses disabling the password prompt');
    fs.writeFileSync(path.join(auto.data,'books/default.fbbook.os-key'),Buffer.from('synthetic damaged sealed key'));
    auto=await launch('own-auto-open',true);await auto.page.waitForSelector('#id_password');
    const damaged=await auto.page.locator('.errorlist').innerText();
    assert.match(damaged,/could not be read[\s\S]*saves a new key/);assert.doesNotMatch(damaged,/Unsupported state|authenticate|decrypt/i);
    await shot(auto,'09-damaged-sealed-key');
    await auto.page.fill('#id_password',PASSWORD);await auto.page.getByRole('button',{name:'Unlock',exact:true}).click();
    await auto.page.waitForURL(/\/home\/$/);
    await close(auto);auto=await launch('own-auto-open',true);await auto.page.waitForURL(/\/home\/$/);
    await close(auto);
    pass('a damaged OS-sealed key asks plainly for the password, opens with it and is sealed again');
    await close(own);await close(builtin);
    console.log(`SMOKE KEYS OK: ${checks} checks; evidence ${base}`);
  } finally { for(const s of sessions)await s.close().catch(()=>{}); }
}
main().catch(e=>{console.error('SMOKE KEYS FAILED:',e.message);process.exitCode=1;});

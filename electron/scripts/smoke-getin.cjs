// Getting in, end to end in the real app on synthetic books: first run, a reinstalled keyring, a
// keyring that is away for one launch, the owner's "ask every time" choice, a manual lock, and a
// backup restored from the unlock screen and made the working books. The keyring is a test stand-in
// whose secret each launch chooses, so "another keyring" is simply another secret.
'use strict';
const {_electron:electron}=require('playwright');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const crypto=require('node:crypto');
const root=path.resolve(__dirname,'..');
const BOOK_PASSWORD='Synthetic book password 1';
const USER_PASSWORD='Synthetic user password 1';
const OWNER='Riley Sample';

async function main() {
  assert.ok(process.env.DISPLAY && ![':0',':20',':0.0',':20.0'].includes(process.env.DISPLAY),'isolated xvfb display required');
  const base=fs.mkdtempSync(path.join(process.env.TMPDIR,'fb-getin-smoke-'));
  const shots=process.env.FB_SMOKE_SHOTS || path.join(base,'screenshots');
  fs.mkdirSync(shots,{recursive:true});
  let checks=0;
  const pass=s=>console.log(`PASS GETIN ${String(++checks).padStart(2,'0')}: ${s}`);
  const sessions=new Set();
  const home=path.join(base,'home'),data=path.join(home,'data');
  fs.mkdirSync(home,{recursive:true,mode:0o700});
  // keyring: a hex secret (a working keyring), or null (no usable keyring: basic_text).
  async function launch(keyring) {
    let entry=root;
    if (keyring) {
      // A folder with its own package.json, so the app reports FreedomBooks' version, not Electron's.
      const dir=path.join(home,`test-keyring-${keyring.slice(0,6)}`);fs.mkdirSync(dir,{recursive:true});
      const pkg=JSON.parse(fs.readFileSync(path.join(root,'package.json'),'utf8'));
      fs.writeFileSync(path.join(dir,'package.json'),JSON.stringify({name:pkg.name,productName:pkg.productName,version:pkg.version,main:'main.cjs'}));
      entry=dir;
      fs.writeFileSync(path.join(dir,'main.cjs'),`const {safeStorage}=require('electron');const c=require('node:crypto');const k=Buffer.from(${JSON.stringify(keyring)},'hex');\nObject.assign(safeStorage,{isEncryptionAvailable:()=>true,getSelectedStorageBackend:()=> 'gnome_libsecret',encryptString:s=>{const n=c.randomBytes(12),x=c.createCipheriv('aes-256-gcm',k,n);const b=Buffer.concat([x.update(s,'utf8'),x.final()]);return Buffer.concat([n,x.getAuthTag(),b]);},decryptString:b=>{try{const x=c.createDecipheriv('aes-256-gcm',k,b.subarray(0,12));x.setAuthTag(b.subarray(12,28));return Buffer.concat([x.update(b.subarray(28)),x.final()]).toString('utf8');}catch{throw new Error('Error while decrypting the ciphertext provided to safeStorage.decryptString.');}}});require(${JSON.stringify(path.join(root,'main/main.cjs'))});\n`,{mode:0o600});
    }
    const env={...process.env,HOME:home,XDG_CONFIG_HOME:path.join(home,'config'),XDG_DATA_HOME:path.join(home,'share'),XDG_CACHE_HOME:path.join(home,'cache'),FB_DESKTOP_USER_DATA:data,ELECTRON_ENABLE_LOGGING:'0'};
    const app=await electron.launch({args:[entry,'--password-store=basic'],cwd:root,env,timeout:180000});
    sessions.add(app);
    await app.evaluate(({dialog})=>{dialog.showErrorBox=(_title,message)=>{throw new Error(message);};});
    // Xvfb never sees input from the test driver, so on a slow machine its idle time would pass the
    // 15-minute idle lock mid-run. Locks are driven explicitly here.
    await app.evaluate(({powerMonitor})=>{powerMonitor.getSystemIdleTime=()=>0;});
    const page=await app.firstWindow();page.setDefaultTimeout(Number(process.env.FB_SMOKE_TIMEOUT_MS)||120000); // a loaded machine may need longer
    await page.setViewportSize({width:1280,height:900});
    await page.waitForSelector('h1');
    const native=async(method,args=[])=>app.evaluate((_e,{modulePath,method,args})=>process.mainModule.require(modulePath)[method](...args),{modulePath:path.join(root,'native/fb-node.node'),method,args});
    return {app,page,native};
  }
  async function close(s) { await s.app.close();sessions.delete(s.app); }
  async function shot(s,name) { await s.page.screenshot({path:path.join(shots,name+'.png'),fullPage:true}); }
  const text=async s=>s.page.locator('main').innerText();
  async function signIn(s) {
    await s.page.waitForURL(/\/desktop\/sign-in\/$/);
    await s.page.fill('#id_username',OWNER);await s.page.fill('#id_password',USER_PASSWORD);
    await s.page.getByRole('button',{name:'Sign in',exact:true}).click();
    await s.page.waitForURL(/\/(home|settings\/[a-z-]+)\/$/);
  }
  async function unlock(s,secret) {
    await s.page.fill('#id_password',secret);
    await s.page.getByRole('button',{name:'Unlock books',exact:true}).click();
  }
  async function quitWithKey(s) { await close(s); }
  const keyringA=crypto.randomBytes(32).toString('hex'), keyringB=crypto.randomBytes(32).toString('hex');
  const book=path.join(data,'books','default.fbbook');
  try {
    // First run.
    let s=await launch(keyringA);
    assert.match(await text(s),/Step 1 of 2/i);
    assert.match(await text(s),/also your user name for signing in/);
    assert.equal(await s.page.locator('nav').count(),0,'no sidebar on the first-run screen');
    await shot(s,'01-first-run-step1');
    await s.page.fill('#id_owner_name',OWNER);await s.page.fill('#id_company_name','Harbor Street Bakery');
    await s.page.check('#id_protect');await s.page.fill('#id_lock_password',BOOK_PASSWORD);await s.page.fill('#id_lock_password2',BOOK_PASSWORD);
    assert.match(await text(s),/Step 1 of 3/i,'a book password adds the recovery-key step');
    await s.page.getByRole('button',{name:'Create books',exact:true}).click();
    await s.page.waitForURL(/\/desktop\/sign-in\/$/);
    const step2=await text(s);
    assert.match(step2,/Step 2 of 3/i);assert.match(step2,new RegExp(`User name: ${OWNER}`));
    assert.equal(await s.page.locator('input[name=confirm_owner]').count(),0,'no "existing owner" box for a brand-new book');
    await shot(s,'02-first-run-step2-sign-in');
    await s.page.fill('#id_password',USER_PASSWORD);await s.page.fill('#id_password_repeat',USER_PASSWORD);
    await s.page.getByRole('button',{name:'Set password and continue',exact:true}).click();
    await s.page.waitForURL(/\/settings\/recovery-key\/$/);
    assert.match(await text(s),/Step 3 of 3/i);
    await shot(s,'03-first-run-step3-recovery');
    const recovery=(await s.page.locator('.recovery-key').innerText()).trim();
    await s.page.getByRole('button',{name:'Remind me later'}).click();
    await s.page.waitForURL(/\/settings\/book-protection\/$/);
    await s.page.uncheck('input[name=ask_password]');await s.page.fill('#id_current_password',BOOK_PASSWORD);
    await Promise.all([s.page.waitForEvent('load'),s.page.getByRole('button',{name:'Save setting'}).click()]);
    await close(s);
    pass('first run names the user name, shows steps 1-3 and has no existing-owner checkbox');

    s=await launch(keyringA);await signIn(s);await close(s);
    assert.ok(fs.existsSync(book+'.os-key'));
    pass('with the same keyring the books open by themselves to sign-in');

    // The reinstall: a new keyring that cannot read either saved key.
    s=await launch(keyringB);
    await s.page.waitForSelector('#id_password');
    let gate=await text(s);
    assert.match(gate,/Unlock your books/);assert.match(gate,/Nothing is wrong with your copy/);
    assert.match(gate,/key saved on this computer can’t be read/);assert.match(gate,/Not your sign-in password/);
    assert.match(gate,/default\.fbbook/);assert.doesNotMatch(gate,/safeStorage|decrypt|Error/);
    assert.equal(await s.page.locator('nav').count(),0,'no sidebar on the unlock screen');
    assert.match(await s.page.locator('header').innerText(),/Books locked/);
    await shot(s,'04-unlock-after-reinstall');
    assert.equal(fs.readdirSync(data).filter(n=>n.startsWith('connection-key.sealed.unreadable-')).length,1);
    await unlock(s,USER_PASSWORD);
    await s.page.waitForSelector('.errorlist');
    assert.match(await s.page.locator('.errorlist').innerText(),/didn’t unlock the books[\s\S]*not your sign-in password/);
    await shot(s,'05-unlock-wrong-entry');
    await unlock(s,BOOK_PASSWORD);await signIn(s);
    await close(s);
    s=await launch(keyringB);await s.page.waitForURL(/\/desktop\/sign-in\/$/);await close(s);
    pass('after a reinstall: plain reason, wrong-entry wording, one unlock, then auto-open again; old connection key set aside');

    // The recovery key alone also unlocks (Emergency Kit only), after the saved key goes missing.
    fs.renameSync(book+'.os-key',path.join(base,'moved-os-key'));
    s=await launch(keyringB);await s.page.waitForSelector('#id_password');
    assert.match(await text(s),/doesn’t have the saved key/);
    await shot(s,'06-unlock-saved-key-missing');
    await unlock(s,recovery);await signIn(s);await close(s);
    pass('a missing saved key says so; the recovery key unlocks and the key is saved again');

    // No usable keyring for one launch.
    const before=fs.readdirSync(data).filter(n=>n.startsWith('connection-key')).sort();
    s=await launch(null);await s.page.waitForSelector('#id_password');
    assert.match(await text(s),/keyring isn’t available right now/);
    await shot(s,'07-unlock-keyring-unavailable');
    await unlock(s,BOOK_PASSWORD);await signIn(s);await close(s);
    assert.deepEqual(fs.readdirSync(data).filter(n=>n.startsWith('connection-key')).sort(),before,'nothing minted or replaced');
    s=await launch(keyringB);await s.page.waitForURL(/\/desktop\/sign-in\/$/);await signIn(s);
    pass('a keyring away for one launch changes no key file, and the next launch opens by itself');

    // Lock, then "ask every time".
    await s.page.goto('fb://app/settings/book-protection/');
    await s.page.check('input[name=ask_password]');await s.page.fill('#id_current_password',BOOK_PASSWORD);
    await Promise.all([s.page.waitForEvent('load'),s.page.getByRole('button',{name:'Save setting'}).click()]);
    await s.app.evaluate(({Menu})=>Menu.getApplicationMenu().getMenuItemById('lock-books').click());
    await s.page.waitForSelector('#id_password');
    assert.match(await text(s),/You locked FreedomBooks/);
    await shot(s,'08-unlock-after-lock');
    await close(s);
    s=await launch(keyringB);await s.page.waitForSelector('#id_password');
    assert.match(await text(s),/ask for the book password each time/);
    await shot(s,'09-unlock-ask-every-time');
    await unlock(s,BOOK_PASSWORD);await signIn(s);
    await s.page.goto('fb://app/settings/book-protection/');
    await s.page.uncheck('input[name=ask_password]');await s.page.fill('#id_current_password',BOOK_PASSWORD);
    await Promise.all([s.page.waitForEvent('load'),s.page.getByRole('button',{name:'Save setting'}).click()]);
    pass('lock and ask-every-time each give their own reason');

    if (process.env.FB_GETIN_SKIP_RESTORE!=='1') await require('./smoke-getin-restore.cjs').run({s,launch,close,shot,text,signIn,unlock,pass,data,book,keyring:keyringB,BOOK_PASSWORD,base});
    else await close(s);
    console.log(`SMOKE GETIN OK: ${checks} checks; screenshots ${shots}`);
  } catch (e) {
    // What was on screen when it stopped, for the log and the screenshots folder.
    for (const a of sessions) {
      try { const p=(await a.windows())[0]; console.error('ON SCREEN',p.url(),'\n',(await p.locator('body').innerText()).slice(0,1500)); await p.screenshot({path:path.join(shots,'zz-failure.png'),fullPage:true}); } catch { /* window gone */ }
    }
    throw e;
  } finally { for(const a of sessions)await a.close().catch(()=>{}); }
}
main().catch(e=>{console.error('SMOKE GETIN FAILED:',e.stack||e.message);process.exitCode=1;});

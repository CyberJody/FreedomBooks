'use strict';
// Synthetic real desktop/preload/native MFA flow. Run on Xvfb with isolated HOME/XDG paths.
const {_electron:electron}=require('playwright');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const crypto=require('node:crypto');
const root=path.resolve(__dirname,'..');
const PASSWORD='Synthetic authenticator password';
function code(secret,offset=0){
  const alphabet='ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';let bits='';
  for(const c of secret)bits+=alphabet.indexOf(c).toString(2).padStart(5,'0');
  const key=Buffer.from(bits.match(/.{8}/g).map(b=>parseInt(b,2)));
  const counter=Buffer.alloc(8);counter.writeBigUInt64BE(BigInt(Math.floor(Date.now()/30000)+offset));
  const mac=crypto.createHmac('sha1',key).update(counter).digest();const at=mac[19]&15;
  return String((mac.readUInt32BE(at)&0x7fffffff)%1000000).padStart(6,'0');
}
async function main(){
  assert.ok(process.env.DISPLAY && ![':0',':20',':0.0',':20.0'].includes(process.env.DISPLAY),'isolated Xvfb display required');
  const base=fs.mkdtempSync(path.join(process.env.TMPDIR,'fb-mfa-smoke-'));
  const home=path.join(base,'home'),data=path.join(base,'data');fs.mkdirSync(home,{recursive:true});
  const shots=process.env.FB_SMOKE_SHOTS || path.join(base,'screenshots');fs.mkdirSync(shots,{recursive:true});
  let app,page,checks=0;
  const pass=s=>console.log(`PASS MFA ${++checks}: ${s}`);
  const native=async(method,args=[])=>app.evaluate((_e,{modulePath,method,args})=>process.mainModule.require(modulePath)[method](...args),{modulePath:path.join(root,'native/fb-node.node'),method,args});
  const state=async()=>JSON.parse(await native('appState'));
  const shot=async n=>page.screenshot({path:path.join(shots,n+'.png')});
  async function launch(){
    app=await electron.launch({args:[root,'--password-store=basic'],cwd:root,env:{...process.env,HOME:home,XDG_CONFIG_HOME:path.join(home,'config'),XDG_DATA_HOME:path.join(home,'share'),XDG_CACHE_HOME:path.join(home,'cache'),FB_DESKTOP_USER_DATA:data,ELECTRON_ENABLE_LOGGING:'0'},timeout:180000});
    await app.evaluate(({dialog})=>{dialog.showErrorBox=(_t,m)=>{throw new Error(m);};});
    page=await app.firstWindow();page.setDefaultTimeout(45000);await page.setViewportSize({width:1440,height:1000});await page.waitForSelector('h1');
  }
  async function signin(){await page.waitForSelector('#id_username');await page.fill('#id_username','Morgan MFA');await page.fill('#id_password',PASSWORD);await page.getByRole('button',{name:'Sign in',exact:true}).click();await page.getByRole('heading',{name:'Verify your sign-in',exact:true}).waitFor();assert.equal((await state()).signed_in,false);}
  async function signout(){await page.goto('fb://app/desktop/session/');await page.getByRole('button',{name:'Sign out / switch user',exact:true}).click();await page.waitForSelector('#id_username');}
  async function verify(value){await page.fill('#id_code',value);await page.getByRole('button',{name:'Verify and sign in',exact:true}).click();}
  try{
    await launch();await page.fill('#id_owner_name','Morgan MFA');await page.fill('#id_company_name','Synthetic MFA books');await page.selectOption('#id_starter_chart','empty');await page.getByRole('button',{name:'Create books',exact:true}).click();
    await page.waitForURL(/\/desktop\/sign-in\/$/);await page.fill('#id_password',PASSWORD);await page.fill('#id_password_repeat',PASSWORD);await page.check('input[name="confirm_owner"]');await page.getByRole('button',{name:'Set password and sign in',exact:true}).click();await page.waitForURL(/\/home\/$/);
    const company=JSON.parse(await native('query',[JSON.stringify({type:'Companies'})]))[0].id;
    const grant=JSON.parse(await native('aiGrant',[company,'Synthetic prior connection','read_only']));
    await page.goto('fb://app/desktop/session/');await page.getByRole('link',{name:'Multi-factor authentication',exact:true}).click();
    await page.fill('#id_password',PASSWORD);await page.getByRole('button',{name:'Continue',exact:true}).click();await page.getByRole('button',{name:'Set up authenticator',exact:true}).click();
    assert.ok(await page.locator('#mfa_setup_qr').isVisible());
    assert.equal(await page.locator('#setup_key').isVisible(),false,'manual entry is a fallback');
    await page.locator('.mfa-manual summary').click();
    const secret=await page.locator('#setup_key').inputValue();assert.match(secret,/^[A-Z2-7]{32}$/);
    // Secret-bearing setup/recovery screens are deliberately excluded from saved screenshots.
    await page.fill('#id_code',code(secret,-1));await page.getByRole('button',{name:'Verify and enable MFA',exact:true}).click();
    const codes=(await page.locator('#recovery_codes').inputValue()).trim().split(/\s+/);assert.equal(codes.length,10);
    await assert.rejects(()=>native('aiRequestAsync',[grant.token]),/unauthorized/);
    await page.getByRole('button',{name:'I saved my codes',exact:true}).click();await page.locator('#recovery_codes').waitFor({state:'detached'});await shot('mfa-01-enabled');
    pass('authenticator enrollment confirms a generated code, returns recovery codes and revokes old AI authority');
    await signout();await signin();
    await assert.rejects(()=>native('query',[JSON.stringify({type:'Companies'})]),/Sign in/);
    await assert.rejects(()=>native('aiGrant',[company,'Refused before MFA','read_only']),/Sign in/);
    await shot('mfa-02-challenge');pass('password-only session cannot read books or grant API/MCP access');
    await verify('invalid');await page.locator('.errorlist').waitFor();assert.equal((await state()).signed_in,false);
    const current=code(secret);await verify(current);await page.waitForURL(/\/home\/$/);assert.equal((await state()).signed_in,true);
    pass('incorrect factor refused; valid authenticator code completes the actual desktop sign-in');
    await signout();await signin();await verify(current);await page.locator('.errorlist').waitFor();assert.equal((await state()).signed_in,false);
    await verify(codes[0]);await page.waitForURL(/\/home\/$/);pass('replayed authenticator code refused; unused recovery code permits sign-in');
    await app.close();app=null;await launch();await signin();await verify(codes[0]);await page.locator('.errorlist').waitFor();
    await verify(codes[1]);await page.waitForURL(/\/home\/$/);pass('recovery consumption persists across a real desktop restart');
    await page.goto('fb://app/desktop/mfa/');await page.fill('#id_password',PASSWORD);await page.fill('#id_code',codes[2]);await page.getByRole('button',{name:'Continue',exact:true}).click();
    await page.check('input[name="confirm_disable"]');await page.getByRole('button',{name:'Disable MFA',exact:true}).click();await page.getByText('Status: Not enabled',{exact:true}).waitFor();
    await signout();await page.fill('#id_username','Morgan MFA');await page.fill('#id_password',PASSWORD);await page.getByRole('button',{name:'Sign in',exact:true}).click();await page.waitForURL(/\/home\/$/);
    pass('fresh password and recovery proof authorize optional disable; ordinary sign-in works afterwards');
    console.log(JSON.stringify({checks,base,shots},null,2));
  }finally{if(app)await app.close();}
}
main().catch(e=>{console.error(e);process.exitCode=1;});

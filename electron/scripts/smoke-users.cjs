// Real isolated Electron UI, synthetic names/books only. Run through the users shared runner.
'use strict';
const {_electron:electron}=require('playwright');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const root=path.resolve(__dirname,'..');
const PASSWORD='Synthetic individual UI password';
async function main(){
  assert.ok(process.env.DISPLAY && ![':0',':20',':0.0',':20.0'].includes(process.env.DISPLAY),'isolated Xvfb display required');
  const base=fs.mkdtempSync(path.join(process.env.TMPDIR,'fb-users-smoke-'));
  const home=path.join(base,'home'),data=path.join(base,'data');fs.mkdirSync(home,{recursive:true});
  const shots=process.env.FB_SMOKE_SHOTS || path.join(base,'screenshots');fs.mkdirSync(shots,{recursive:true});
  let app,page,checks=0;
  const pass=s=>console.log(`PASS USERS ${++checks}: ${s}`);
  const native=async(method,args=[])=>app.evaluate((_e,{modulePath,method,args})=>process.mainModule.require(modulePath)[method](...args),{modulePath:path.join(root,'native/fb-node.node'),method,args});
  const state=async()=>JSON.parse(await native('appState'));
  const shot=async n=>page.screenshot({path:path.join(shots,n+'.png')});
  async function launch(){
    app=await electron.launch({args:[root,'--password-store=basic'],cwd:root,env:{...process.env,HOME:home,XDG_CONFIG_HOME:path.join(home,'config'),XDG_DATA_HOME:path.join(home,'share'),XDG_CACHE_HOME:path.join(home,'cache'),FB_DESKTOP_USER_DATA:data,ELECTRON_ENABLE_LOGGING:'0',UV_THREADPOOL_SIZE:'1'},timeout:180000});
    await app.evaluate(({dialog})=>{dialog.showErrorBox=(_t,m)=>{throw new Error(m);};});
    page=await app.firstWindow();page.setDefaultTimeout(45000);await page.setViewportSize({width:1440,height:1000});await page.waitForSelector('h1');
  }
  async function signin(name,password=PASSWORD){
    await page.waitForURL(/\/desktop\/sign-in\/$/);await page.fill('#id_username',name);await page.fill('#id_password',password);await page.getByRole('button',{name:'Sign in',exact:true}).click();
  }
  async function signout(){await page.getByRole('link',{name:'Switch user / sign out',exact:true}).click();await page.getByRole('button',{name:'Sign out / switch user',exact:true}).click();await page.waitForURL(/\/desktop\/sign-in\/$/);assert.equal((await state()).signed_in,false);}
  async function users(){await page.getByRole('link',{name:'Switch user / sign out',exact:true}).click();await page.getByRole('link',{name:'Manage users',exact:true}).click();await page.waitForSelector('#id_role');}
  async function add(name,role){
    await users();await page.fill('#id_username',name);await page.selectOption('#id_role',role);await page.check('input[name="company"]');await page.fill('#id_password',PASSWORD);await page.fill('#id_password_repeat',PASSWORD);await page.getByRole('button',{name:'Save user and sign out',exact:true}).click();await page.waitForURL(/\/desktop\/sign-in\/$/);
  }
  try{
    await launch();await page.fill('#id_owner_name','Avery Owner');await page.fill('#id_company_name','Synthetic desktop team');await page.selectOption('#id_starter_chart','empty');await page.getByRole('button',{name:'Create books',exact:true}).click();
    await page.waitForURL(/\/desktop\/sign-in\/$/);assert.equal((await state()).signed_in,false);assert.match(await page.locator('h1').innerText(),/owner/);
    await shot('users-01-owner-setup');
    await page.fill('#id_password',PASSWORD);await page.fill('#id_password_repeat',PASSWORD);await page.check('input[name="confirm_owner"]');await page.getByRole('button',{name:'Set password and sign in',exact:true}).click();await page.waitForURL(/\/home\/$/);
    assert.match(await page.locator('.operator').innerText(),/Avery Owner/);pass('first run requires explicit owner password enrollment before accounting access');
    await add('Pat Bookkeeper','bookkeeper');await signin('Pat Bookkeeper','wrong UI password');await page.waitForSelector('.errorlist');assert.equal((await state()).signed_in,false);pass('wrong per-user password refuses authentication');
    await signin('Pat Bookkeeper');await page.waitForURL(/\/home\/$/);assert.match(await page.locator('.operator').innerText(),/Pat Bookkeeper/);await shot('users-02-bookkeeper');
    await page.getByRole('link',{name:'Switch user / sign out',exact:true}).click();assert.equal(await page.getByRole('link',{name:'Manage users',exact:true}).count(),0);pass('bookkeeper session shows actual name and lacks administration access');
    await page.getByRole('button',{name:'Sign out / switch user',exact:true}).click();await signin('Avery Owner');await page.waitForURL(/\/home\/$/);
    await add('Riley Reader','readonly');await signin('Riley Reader');await page.waitForURL(/\/home\/$/);assert.match(await page.locator('body').innerText(),/Read-only access/);await shot('users-03-readonly');
    const companies=JSON.parse(await native('query',[JSON.stringify({type:'Companies'})]));const company=companies[0].id;
    const tokenPage=JSON.parse(await native('render',[`/c/${company}/lists/customers/new/`]));
    const token=/name="fb_command_token" value="([0-9a-f]{64})"/.exec(tokenPage.html)?.[1];assert.ok(token);
    await assert.rejects(()=>native('run',[JSON.stringify({type:'SubmitForm',path:`/c/${company}/lists/customers/new/`,token,fields:[['display_name','Must not save']]})]),/Read-only users/);
    await assert.rejects(()=>native('render',['/desktop/users/']),/administrator/);
    await assert.rejects(()=>native('aiGrant',[company,'Forbidden read-only writer','accounting_write']),/Read-only users/);
    const readerGrant=JSON.parse(await native('aiGrant',[company,'Synthetic reader','read_only']));
    assert.ok(JSON.parse(await native('aiRequestAsync',[readerGrant.token])).operations.length>0);
    pass('native boundary refuses read-only writes, write grants and non-admin user management while allowing read-only AI');
    await signout();await signin('Avery Owner');await page.waitForURL(/\/home\/$/);
    await assert.rejects(()=>native('aiRequestAsync',[readerGrant.token]),/unauthorized/);
    const old=JSON.parse(await native('render',[`/c/${company}/lists/customers/new/`]));const stale=/name="fb_command_token" value="([0-9a-f]{64})"/.exec(old.html)[1];
    await signout();await signin('Pat Bookkeeper');await page.waitForURL(/\/home\/$/);
    await assert.rejects(()=>native('run',[JSON.stringify({type:'SubmitForm',path:`/c/${company}/lists/customers/new/`,token:stale,fields:[['display_name','Stale owner form']]})]),/expired/);pass('switching to another person invalidates the previous person’s form');
    const queuedError=await app.evaluate(async (_e,{modulePath,target})=>{
      const n=process.mainModule.require(modulePath),crypto=process.mainModule.require('node:crypto');
      const busy=new Promise((resolve,reject)=>crypto.pbkdf2('synthetic','fixture-salt',1000000,32,'sha256',error=>error?reject(error):resolve()));
      const queued=n.renderAsync(target);n.signOut();
      let error='';try{await queued;}catch(e){error=e.message;}await busy;return error;
    },{modulePath:path.join(root,'native/fb-node.node'),target:`/c/${company}/home/`});
    assert.match(queuedError,/active user changed/);await page.goto('fb://app/desktop/sign-in/');
    pass('native queued work rejects the accepting session after a user change');
    await signin('Avery Owner');await page.waitForURL(/\/home\/$/);await users();
    await page.locator('tr').filter({hasText:'Pat Bookkeeper'}).getByRole('button',{name:'Edit user',exact:true}).click();await page.waitForSelector('h2');await page.uncheck('input[name="active"]');await page.getByRole('button',{name:'Save user and sign out',exact:true}).click();await signin('Pat Bookkeeper');await page.waitForSelector('.errorlist');assert.equal((await state()).signed_in,false);pass('administrator can disable a named person; their password no longer signs in');
    await signin('Avery Owner');await page.waitForURL(/\/home\/$/);await users();await page.locator('tr').filter({hasText:'Avery Owner'}).getByRole('button',{name:'Edit user',exact:true}).click();await page.uncheck('input[name="active"]');await page.getByRole('button',{name:'Save user and sign out',exact:true}).click();await page.waitForSelector('.errorlist');assert.match(await page.locator('.errorlist').innerText(),/administrator/);assert.equal((await state()).signed_in,true);await shot('users-04-last-admin');pass('last-administrator refusal is visible and preserves the owner’s live session');
    await app.close();app=null;await launch();await page.waitForURL(/\/desktop\/sign-in\/$/);assert.equal((await state()).open,true);assert.equal((await state()).signed_in,false);await shot('users-05-reopen-signin');await signin('Avery Owner');await page.waitForURL(/\/home\/$/);pass('built-in-key book reopens safely and still requires named-user authentication');
    console.log(JSON.stringify({checks,base,shots},null,2));
  }finally{if(app)await app.close();}
}
main().catch(e=>{console.error(e);process.exitCode=1;});

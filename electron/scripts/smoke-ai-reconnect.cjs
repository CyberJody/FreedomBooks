'use strict';
// Packaged Electron + isolated D-Bus/keyring. Run under a private Xvfb, never on the user display.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),net=require('node:net');
const {spawn,execFile}=require('node:child_process'),{promisify}=require('node:util'),{createInterface}=require('node:readline'),{setTimeout:delay}=require('node:timers/promises');
const {chromium}=require('playwright'),run=promisify(execFile);
const [binary,out]=process.argv.slice(2),password='SyntheticReconnectOwner42';
assert.ok(binary&&path.isAbsolute(binary)&&out&&path.isAbsolute(out)&&!fs.existsSync(out));
assert.ok(process.env.HOME && path.isAbsolute(process.env.HOME) &&
  path.resolve(process.env.HOME)!==require('node:os').userInfo().homedir,
  'Use a disposable HOME, XDG directories, session bus and unlocked test keyring for the packaged reconnect smoke');
assert.ok(process.env.DISPLAY&&![':0',':0.0',':20',':20.0'].includes(process.env.DISPLAY));
fs.mkdirSync(out,{recursive:true,mode:0o700});let child,browser,page,client,launches=0;const checks=[],errors=[],env={...process.env};
for(const k of ['FB_NATIVE_MODULE','FB_DESKTOP_USER_DATA','ELECTRON_RUN_AS_NODE','NODE_OPTIONS'])delete env[k];
async function start(){
 const server=net.createServer();await new Promise(r=>server.listen(0,'127.0.0.1',r));const port=server.address().port;await new Promise(r=>server.close(r));
 const log=fs.openSync(path.join(out,`launch-${++launches}.log`),'wx');child=spawn(binary,[`--remote-debugging-port=${port}`,'--remote-debugging-address=127.0.0.1','--password-store=gnome-libsecret','--xdg-portal-required-version=999'],{env,stdio:['ignore',log,log]});fs.closeSync(log);
 const until=Date.now()+60000;while(Date.now()<until){assert.equal(child.exitCode,null);try{browser=await chromium.connectOverCDP(`http://127.0.0.1:${port}`,{timeout:1000});break;}catch{await delay(150);}}
 assert.ok(browser,'Packaged app did not start');const context=browser.contexts()[0];page=context.pages().find(p=>p.url().startsWith('fb://app/'))||await context.waitForEvent('page');page.setDefaultTimeout(30000);page.on('pageerror',e=>errors.push(e.message));await page.waitForSelector('main h1');await page.setViewportSize({width:1280,height:900});
}
async function focusMain(){const {stdout}=await run('xdotool',['search','--onlyvisible','--class','FreedomBooks']);const ids=stdout.trim().split('\n');assert.equal(ids.length,1);await run('xdotool',['windowfocus','--sync',ids[0]]);}
async function stop(){if(!child)return;await focusMain();await run('xdotool',['key','--clearmodifiers','ctrl+q']);if(browser){await browser.close();browser=null;}const until=Date.now()+10000;while(child.exitCode===null&&Date.now()<until)await delay(100);assert.equal(child.exitCode,0,'Normal File/Quit closes synthetic app');child=null;}
async function home(){await page.waitForURL(/\/(?:home|settings\/recovery-key)\/$/);if(page.url().endsWith('/settings/recovery-key/'))await page.locator('a[href$="/home/"]').first().click();await page.waitForURL(/\/home\/$/);}
async function signin(){await page.fill('#id_password',password);await page.getByRole('button',{name:'Sign in',exact:true}).click();await home();}
async function nativeDialog(title,keys){
 const until=Date.now()+30000;let ids=[];
 while(Date.now()<until){try{const {stdout}=await run('xdotool',['search','--onlyvisible','--name','^'+title+'$']);ids=stdout.trim().split('\n').filter(Boolean);if(ids.length===1)break;}catch{}await delay(75);}
 assert.equal(ids.length,1,'Expected native dialog: '+title);await run('xdotool',['windowfocus','--sync',ids[0],'key','--clearmodifiers',...keys],{timeout:10000});
}
async function settings(which){await focusMain();await run('xdotool',['key','--clearmodifiers','Alt_L','Right','Down',...(which==='manage'?['Down']:[]),'Return']);}
async function until(check,msg){const end=Date.now()+10000;while(!check()){if(Date.now()>=end)throw Error(msg);await delay(50);}}
function clientFor(file){
 const child=spawn(binary,['--ai-api',file,'--password-store=gnome-libsecret'],{env,stdio:['pipe','pipe','pipe']});const lines=[],waiting=[];let stderr='';child.stderr.on('data',c=>stderr+=c);
 createInterface({input:child.stdout}).on('line',line=>{const r=JSON.parse(line),p=waiting.shift();if(p)p(r);else lines.push(r);});
 const call=async request=>{child.stdin.write(JSON.stringify(request)+'\n');if(lines.length)return lines.shift();return new Promise((resolve,reject)=>{const timer=setTimeout(()=>reject(Error('Synthetic API timeout')),20000);waiting.push(v=>{clearTimeout(timer);resolve(v);});});};
 return {child,call,stderr:()=>stderr};
}
(async()=>{try{
 await start();await page.fill('#id_owner_name','Synthetic reconnect owner');await page.fill('#id_company_name','Synthetic reconnect company');await page.getByRole('button',{name:'Create books',exact:true}).click();await page.waitForURL(/\/desktop\/sign-in\/$/);
 await page.fill('#id_password',password);await page.fill('#id_password_repeat',password);if(await page.locator('[name="confirm_owner"]').count())await page.check('[name="confirm_owner"]');await page.getByRole('button',{name:'Set password and continue',exact:true}).click();await home();
 const company=/\/c\/([^/]+)\//.exec(page.url())[1],base=`fb://app/c/${company}`;
 await page.goto('fb://app/help/report/');await page.waitForSelector('main h1');assert.match(await page.locator('main h1').innerText(),/Report a problem/);assert.match(await page.locator('.menu-name').first().innerText(),/Synthetic reconnect company/);
 await settings('connect');
 // GTK focuses the locked-access checkbox first. Tab reaches Cancel, then Right advances through the buttons.
 await nativeDialog('Connect an AI assistant',['Tab','Right','Right','Return']);await nativeDialog('AI assistant connected',['Return']);
 const directory=path.join(env.XDG_DATA_HOME,'FreedomBooks/ai-connections'),files=fs.readdirSync(directory).filter(n=>/^[0-9a-f-]{36}\.sealed$/.test(n));assert.equal(files.length,1);const file=path.join(directory,files[0]),sealed=fs.readFileSync(file);assert.equal(fs.statSync(file).mode&0o777,0o600);
 client=clientFor(file);const cat=await client.call({catalogue:true});assert.equal(cat.ok,true);assert.equal(cat.data.company_id,company);assert.equal(cat.data.permission,'accounting_write');checks.push('Report a problem page connects its authorized company using isolated OS keyring and private sealed file');
 await page.screenshot({path:path.join(out,'connected-from-report.png'),fullPage:true});
 await stop();assert.equal((await client.call({catalogue:true})).ok,false);checks.push('normal File/Quit immediately stops API access');
 await start();assert.match(page.url(),/\/desktop\/sign-in\/$/);await until(()=>fs.existsSync(path.join(directory,'.endpoint.sealed')),'AI endpoint did not publish after restart');assert.equal((await client.call({catalogue:true})).ok,true);await signin();
 const restored=await client.call({operation:'app_ConnectionInfo',arguments:{}});assert.equal(restored.ok,true,JSON.stringify(restored));assert.equal(restored.data.company_id,company);assert.ok(fs.readFileSync(file).equals(sealed));assert.deepEqual(fs.readdirSync(directory).filter(n=>/^[0-9a-f-]{36}\.sealed$/.test(n)),files);checks.push('ticked-by-default locked access reconnects across full restart before and after sign-in without rewriting the sealed file');
 const created=await client.call({operation:'command_SaveCustomer',arguments:{display_name:'Synthetic reconnect customer'}});assert.equal(created.ok,true,JSON.stringify(created));checks.push('reconnected typed write succeeds without another permission prompt');
 await focusMain();await run('xdotool',['key','--clearmodifiers','ctrl+l']);await page.waitForURL(/\/desktop\/sign-in\/$/);assert.equal((await client.call({catalogue:true})).ok,true);await signin();assert.equal((await client.call({catalogue:true})).ok,true);checks.push('ticked-by-default locked access stays active while locked');
 await settings('manage');await nativeDialog('AI assistant connections',['Right','Right','Return']);await nativeDialog('Change AI locked access',['Right','Return']);
 await until(()=>fs.existsSync(file)&&!fs.readFileSync(file).equals(sealed),'Manage did not rewrite approval to Require sign-in');
 const attended=fs.readFileSync(file);assert.ok(!attended.equals(sealed));
 await focusMain();await run('xdotool',['key','--clearmodifiers','ctrl+l']);await page.waitForURL(/\/desktop\/sign-in\/$/);assert.equal((await client.call({catalogue:true})).ok,false);await signin();assert.equal((await client.call({catalogue:true})).ok,true);checks.push('Manage switches to Require sign-in so lock pauses access and sign-in resumes it');
 await page.screenshot({path:path.join(out,'reconnected-home.png'),fullPage:true});
 assert.deepEqual(await page.evaluate(()=>({require:typeof window.require,process:typeof window.process,fb:typeof window.fb})),{require:'undefined',process:'undefined',fb:'undefined'});checks.push('packaged renderer has no Node or unrestricted native bridge');
 await settings('manage');await nativeDialog('AI assistant connections',['Right','Return']);await until(()=>!fs.existsSync(file),'Forget did not remove approval file');assert.equal((await client.call({catalogue:true})).ok,false);
 fs.writeFileSync(file,sealed,{mode:0o600});await page.goto(base+'/home/');assert.equal((await client.call({catalogue:true})).ok,false);checks.push('Forget remains revoked after an old sealed file is replayed');
 assert.deepEqual(errors,[]);fs.writeFileSync(path.join(out,'result.json'),JSON.stringify({binary,synthetic:true,real_profile_access:false,provider_calls:false,checks},null,2));console.log('PASS '+checks.join('; '));
 }catch(e){if(page&&!page.isClosed()){console.error(page.url(),await page.locator('body').innerText());await page.screenshot({path:path.join(out,'failure.png'),fullPage:true});}throw e;}finally{if(client){client.child.stdin.end();client.child.kill();}if(child){try{await stop();}catch{child.kill();}}}
})().catch(e=>{console.error(e);process.exitCode=1;});

'use strict';
// Real main-process heartbeat and deferred lock/quit checks. Synthetic data only; requires
// an explicitly built key-test-hooks native module and an isolated Xvfb display.
const {_electron:electron}=require('playwright');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const root=path.resolve(__dirname,'..');
const password='Synthetic recovery worker password';
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));

async function main(){
  assert.ok(process.env.DISPLAY&&![':0',':0.0',':20',':20.0'].includes(process.env.DISPLAY));
  assert.ok(path.isAbsolute(process.env.FB_NATIVE_MODULE||''));
  const base=process.env.FB_BACKUPS_SMOKE_DIR;
  assert.ok(base&&!fs.existsSync(base),'use a new isolated test directory');
  fs.mkdirSync(base,{recursive:true,mode:0o700});
  const shots=process.env.FB_SMOKE_SHOTS;fs.mkdirSync(shots,{recursive:true});
  const sessions=new Set();
  try{
    for(const mode of ['text-lock','pdf-lock','text-quit','stale-dialog']){
      const home=path.join(base,mode),data=path.join(home,'data');
      fs.mkdirSync(home,{mode:0o700});
      const ready=path.join(home,'worker-ready'),resume=path.join(home,'worker-resume');
      const output=path.join(home,mode.startsWith('pdf')?'Synthetic Kit.pdf':'Synthetic Kit.txt');
      const env={...process.env,HOME:home,XDG_CONFIG_HOME:path.join(home,'config'),XDG_DATA_HOME:path.join(home,'share'),XDG_CACHE_HOME:path.join(home,'cache'),FB_DESKTOP_USER_DATA:data,FB_KEY_STOP_AT:'protection-before-snapshot',FB_KEY_STOP_MARKER:ready,FB_KEY_RESUME_MARKER:resume};
      const app=await electron.launch({args:[root,'--password-store=basic'],cwd:root,env,timeout:180000});
      sessions.add(app);
      const page=await app.firstWindow();page.setDefaultTimeout(180000);
      await page.setViewportSize({width:1280,height:900});
      await page.fill('#id_owner_name','Synthetic worker owner');
      await page.fill('#id_company_name','Synthetic worker books');
      await page.check('#id_protect');
      await page.fill('#id_lock_password',password);await page.fill('#id_lock_password2',password);
      await page.getByRole('button',{name:'Create books',exact:true}).click();
      await page.waitForURL(/\/settings\/recovery-key\/$/);
      await app.evaluate(({dialog},{output,stale})=>{
        global.recoveryTicks=0;global.recoveryTimer=setInterval(()=>global.recoveryTicks++,10);
        dialog.showErrorBox=(_title,message)=>{global.recoveryError=message;};
        dialog.showSaveDialog=async()=>{
          if(stale) await new Promise(resolve=>{global.releaseRecoveryDialog=resolve;});
          return {canceled:false,filePath:output};
        };
      },{output,stale:mode==='stale-dialog'});
      if(mode==='pdf-lock'){
        await page.getByRole('button',{name:'Print',exact:true}).click();
        await page.waitForURL(/\/settings\/emergency-kit\/$/);
        await app.evaluate(({Menu,BrowserWindow})=>{
          const file=Menu.getApplicationMenu().items.find(i=>i.label==='File');
          global.recoveryPrint=file.submenu.items.find(i=>i.label==='Save as PDF…').click(null,BrowserWindow.getAllWindows()[0]);
        });
      }else{
        await page.getByRole('button',{name:'Save to USB / file…'}).click();
      }
      if(mode==='stale-dialog'){
        const started=Date.now();
        while(!await app.evaluate(()=>!!global.releaseRecoveryDialog)){
          assert.ok(Date.now()-started<30000);await delay(20);
        }
        await app.evaluate(({powerMonitor})=>{powerMonitor.emit('lock-screen');global.releaseRecoveryDialog();});
        await page.waitForSelector('#id_password');await delay(100);
        assert.equal(fs.existsSync(output),false,'a locked dialog must not export');
        assert.equal(fs.existsSync(ready),false,'a locked dialog must not start acknowledgment');
      }else{
        const started=Date.now();
        while(!fs.existsSync(ready)){
          assert.ok(Date.now()-started<60000,`${mode}: worker checkpoint not reached`);await delay(20);
        }
        const ticks=await app.evaluate(()=>global.recoveryTicks);
        await delay(150);
        assert.ok(await app.evaluate(()=>global.recoveryTicks)>ticks+3,'main event loop must run while snapshot worker is held');
        await page.screenshot({path:path.join(shots,mode+'-busy.png'),mask:[page.locator('.recovery-key')]});
        if(mode==='text-quit'){
          const closed=app.waitForEvent('close');
          await app.evaluate(({app})=>{app.quit();});
          await delay(100);
          assert.ok(await app.evaluate(()=>global.recoveryTicks)>ticks,'quit must defer while acknowledgment is running');
          fs.writeFileSync(resume,'resume');await closed;sessions.delete(app);
        }else{
          await app.evaluate(({powerMonitor})=>{powerMonitor.emit('lock-screen');});
          assert.equal(fs.existsSync(resume),false);
          fs.writeFileSync(resume,'resume');
          await page.waitForSelector('#id_password');
          await page.screenshot({path:path.join(shots,mode+'-locked.png')});
        }
        const record=JSON.parse(fs.readFileSync(path.join(data,'books/default.fbbook.keys.json'),'utf8'));
        assert.equal(record.recovery_saved,true,'lock/quit must follow durable acknowledgment');
        assert.ok(fs.statSync(output).size>0);
        assert.equal(fs.statSync(output).mode&0o777,0o600);
        assert.equal(fs.existsSync(path.join(data,'books/default.fbbook.rekey.json')),false);
      }
      if(sessions.has(app)){await app.close();sessions.delete(app);}
      console.log('PASS recovery worker:',mode);
    }
  }finally{
    // Release only this test's checkpoints before closing its own apps, even after failure.
    for(const mode of ['text-lock','pdf-lock','text-quit','stale-dialog']){
      const home=path.join(base,mode);if(fs.existsSync(home))fs.writeFileSync(path.join(home,'worker-resume'),'resume');
    }
    for(const app of sessions)await app.close();
  }
}
main().catch(error=>{console.error(error);process.exitCode=1;});

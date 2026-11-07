'use strict';
// Ordinary GET responsiveness, with a held real Rust render (test-hook build only).
// The independent test process always releases the checkpoint, including on failure.
const { _electron: electron } = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const password = 'Synthetic render test password';

async function main() {
  const base = process.env.FB_BACKUPS_SMOKE_DIR;
  assert.ok(base && !fs.existsSync(base), 'use a new isolated synthetic fixture');
  assert.ok(path.isAbsolute(process.env.FB_NATIVE_MODULE || ''), 'explicit test-hook native module');
  fs.mkdirSync(base, { recursive:true, mode:0o700 });
  fs.mkdirSync(process.env.FB_SMOKE_SHOTS, { recursive:true });
  const env = { ...process.env, HOME:path.join(base,'home'), XDG_CONFIG_HOME:path.join(base,'config'),
    XDG_DATA_HOME:path.join(base,'share'), XDG_CACHE_HOME:path.join(base,'cache'), FB_DESKTOP_USER_DATA:path.join(base,'data') };
  const app = await electron.launch({ chromiumSandbox:true, args:[root,'--password-store=basic'], cwd:root, env, timeout:180000 });
  let resume, closed = false;
  try {
    const page = await app.firstWindow();
    page.setDefaultTimeout(120000);page.setDefaultNavigationTimeout(120000);
    await page.fill('#id_owner_name','Synthetic render owner');
    await page.fill('#id_company_name','Synthetic render books');
    await page.check('#id_protect');
    await page.fill('#id_lock_password',password);await page.fill('#id_lock_password2',password);
    await Promise.all([page.waitForURL(/\/settings\/recovery-key\/$/),page.getByRole('button',{name:'Create books',exact:true}).click()]);
    const company = await app.evaluate(({app}) => {
      const req = process.getBuiltinModule('module').createRequire(app.getAppPath()+'/package.json');
      return JSON.parse(req(process.env.FB_NATIVE_MODULE).query('{"type":"Companies"}'))[0].id;
    });
    const target = `fb://app/c/${company}/backups/`;
    const start = Date.now();await page.goto(target);
    console.log(`MEASURE ordinary empty Backups GET: ${Date.now()-start} ms (no injected delay)`);
    assert.match(await page.locator('.backup-status').innerText(),/0 files/);
    await app.evaluate(({app},beat) => {
      const req = process.getBuiltinModule('module').createRequire(app.getAppPath()+'/package.json');
      const fs = req('node:fs');let ticks=0;fs.writeFileSync(beat,'0');
      global.renderTimer=setInterval(()=>fs.writeFileSync(beat,String(++ticks)),20);
    },path.join(base,'heartbeat'));
    for (const mode of ['empty','populated','lock','quit']) {
      if (mode==='populated') {
        await Promise.all([page.waitForURL(/\?created=/),page.getByRole('button',{name:'Create backup now',exact:true}).click()]);
        assert.match(await page.locator('.backup-files').innerText(),/Verified/);
      }
      const ready = path.join(base,mode+'-ready');resume = path.join(base,mode+'-resume');
      await app.evaluate((_e,{ready,resume}) => {
        process.env.FB_KEY_STOP_AT='backups-before-status';
        process.env.FB_KEY_STOP_MARKER=ready;process.env.FB_KEY_RESUME_MARKER=resume;
      },{ready,resume});
      const navigation=page.goto(target+'?render-case='+mode).then(()=>({ok:true}),e=>({error:e.message}));
      const started=Date.now();
      while (!fs.existsSync(ready)) {
        assert.ok(Date.now()-started<30000,'render checkpoint not reached');await delay(20);
      }
      const ticks=()=>Number(fs.readFileSync(path.join(base,'heartbeat'),'utf8'));
      const before=ticks();await delay(300);const advanced=ticks()-before;
      // Release before asserting: the pre-fix synchronous implementation must fail without hanging.
      if (advanced<5) { fs.writeFileSync(resume,'resume');await navigation;assert.fail(`${mode}: main heartbeat advanced only ${advanced} times during held ordinary GET`); }
      if (mode==='lock') await app.evaluate(({powerMonitor})=>{powerMonitor.emit('lock-screen');});
      let quit;
      if (mode==='quit') {
        quit=app.waitForEvent('close');await app.evaluate(({app})=>{app.quit();});
        await delay(100);assert.ok(ticks()>before+5,'quit must wait for the active worker');
      }
      fs.writeFileSync(resume,'resume');
      if (mode==='quit') { await quit;closed=true; }
      const result=await navigation;
      if (mode==='empty'||mode==='populated') {
        assert.equal(result.ok,true,JSON.stringify(result));
        assert.match(await page.locator('main h1').innerText(),/Backups/);
        await page.screenshot({path:path.join(process.env.FB_SMOKE_SHOTS,mode+'.png')});
      }
      if (mode==='lock') {
        await page.waitForSelector('#id_password');
        assert.equal(await page.locator('.backup-files').count(),0,'locked render cannot return book data');
        await page.screenshot({path:path.join(process.env.FB_SMOKE_SHOTS,'locked.png')});
        await page.fill('#id_password',password);
        await Promise.all([page.waitForURL(/\/home\/$/),page.getByRole('button',{name:'Unlock',exact:true}).click()]);
      }
      console.log(`PASS ordinary Backups GET ${mode}: main heartbeat advanced ${advanced} times during 300 ms held render`);
    }
  } finally {
    if(resume)fs.writeFileSync(resume,'resume');
    if(!closed)await app.close();
    console.log(`Synthetic Backups render fixture retained at ${base}`);
  }
}
main().catch(error=>{console.error(error);process.exitCode=1;});

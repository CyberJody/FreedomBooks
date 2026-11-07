'use strict';
// Capture the final four pagination fixtures after the full catalogue has completed.
const {_electron:electron}=require('playwright');
const fs=require('node:fs');
const os=require('node:os');
const path=require('node:path');
const assert=require('node:assert/strict');
const root=path.resolve(__dirname,'..');
const shots=process.env.FB_SMOKE_SHOTS || '/mnt/Data/Projects/FreedomBooks/staging/d7b';
const fixtures=path.resolve(root,'../target/d7b-screendiff-final/rust');
const names=['page-two','page-unicode','page-huge','page-control'].map(n=>'d7b-register-'+n);
(async()=>{
  const data=fs.mkdtempSync(path.join(process.env.TMPDIR||os.tmpdir(),'fb-d7b-paging-'));
  const app=await electron.launch({args:[root],cwd:root,env:{...process.env,FB_DESKTOP_USER_DATA:data,XDG_CONFIG_HOME:path.join(data,'config')},timeout:60000});
  try{
    const page=await app.firstWindow();await page.setViewportSize({width:1440,height:1000});
    const errors=[];page.on('pageerror',e=>errors.push(e.message));
    await app.evaluate(({protocol},{root,fixtures,names})=>{
      const require=process.getBuiltinModule('module').createRequire(root+'/package.json');
      const fs=require('node:fs');const path=require('node:path');const guards=require(path.join(root,'main/guards.cjs'));
      protocol.unhandle('fb');
      protocol.handle('fb',request=>{
        const url=new URL(request.url);
        if(request.method!=='GET'||url.host!=='app')return new Response('Not found',{status:404});
        if(url.pathname.startsWith('/static/'))return guards.staticFile(path.join(root,'renderer/static'),url.pathname.slice(8));
        const name=/^\/__d7b_paging__\/([a-z0-9-]+)\/$/.exec(url.pathname)?.[1];
        if(!names.includes(name))return new Response('Not found',{status:404});
        return new Response(fs.readFileSync(path.join(fixtures,name+'.html'),'utf8'),{status:200,headers:{'content-type':'text/html; charset=utf-8','content-security-policy':guards.CSP,'cache-control':'no-store'}});
      });
    },{root,fixtures,names});
    for(const name of names){
      const response=await page.goto('fb://app/__d7b_paging__/'+name+'/');
      assert.match(response.headers()['content-security-policy'],/script-src 'self';/);
      assert.equal(await page.locator('.bank-register tbody tr[id^=line-]').count(),name.endsWith('control')?50:5);
      assert.equal(await page.locator('tr.forward').count(),name.endsWith('control')?0:1);
      await page.screenshot({path:path.join(shots,'catalogue-'+name+'.png'),fullPage:false});
    }
    assert.deepEqual(errors,[]);
    fs.writeFileSync(path.join(shots,'d7b-paging-catalogue.json'),JSON.stringify({screens:4,width:1440,height:1000,navigation:true,productionCSP:true,fixtures},null,2)+'\n');
    console.log('SMOKE D7B PAGING OK: 4 screens at 1440x1000; CSP/navigation and balance-forward rows verified');
  }finally{await app.close();fs.rmSync(data,{recursive:true,force:true});}
})().catch(e=>{console.error(e.stack||e);process.exitCode=1;});

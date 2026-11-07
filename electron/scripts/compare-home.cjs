'use strict';
// Render the frozen Django Home with its original CSS/data-URI arrow, then compare to the
// real Electron/CSP catalogue capture. No server and no changes to the frozen reference.
const fs = require('node:fs');
const path = require('node:path');
const {chromium} = require('playwright');
const root = path.resolve(__dirname,'../..');
const project = path.resolve(root,'../..');
const output = process.env.FB_SMOKE_SHOTS;
(async () => {
  const browser=await chromium.launch({executablePath:path.join(project,'.tooling/ms-playwright/chromium-1248/chrome-linux64/chrome'),ignoreDefaultArgs:['--hide-scrollbars']});
  try {
    const page=await browser.newPage({viewport:{width:1440,height:1000}});
    await page.route('**/*',async route => {
      const url=new URL(route.request().url());
      if (url.hostname!=='reference.invalid') return route.abort();
      if (url.pathname.startsWith('/static/')) {
        const rel=url.pathname.slice(8);
        const app=rel.startsWith('ui/')?'ledger':rel.split('/')[0];
        return route.fulfill({path:path.join(project,'wt/reference',app,'static',rel)});
      }
      return route.fulfill({contentType:'text/html',path:path.join(root,'target/screendiff/django/home.html')});
    });
    await page.goto('http://reference.invalid/home/');
    await page.waitForLoadState('networkidle');
    await page.screenshot({path:path.join(output,'django-home.png')});
    const arrows=await page.locator('.step.connected').evaluateAll(rows=>rows.map(row=>{
      const style=getComputedStyle(row,'::after');const r=row.getBoundingClientRect();
      return {x:r.right+3,y:r.top+30,width:24,height:16,background:style.backgroundImage};
    }));
    if (!arrows.length||arrows.some(a=>!a.background.startsWith('url("data:'))) throw Error('Django reference arrows missing');
    fs.writeFileSync(path.join(output,'home-arrows.json'),JSON.stringify(arrows,null,2)+'\n');
    console.log(`Django Home: ${arrows.length} original arrows, screenshot 1440x1000`);
  } finally {await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});

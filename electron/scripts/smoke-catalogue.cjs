'use strict';
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');

module.exports = async ({app, page, root, shots, shot, pass}) => {
  const screens = JSON.parse(fs.readFileSync(path.join(root, '../crates/fb-screendiff/screens.json'), 'utf8'));
  const fixtureDir = fs.realpathSync(process.env.FB_SMOKE_FIXTURE_DIR || path.join(process.env.FB_SCREENDIFF_OUT || path.join(root, '../target/screendiff'), 'rust'));
  // Test-process-only scheme handler: production code and the fixed IPC surface are unchanged.
  // Return the exact fb-app fixture bytes with the production CSP and production static resolver.
  await app.evaluate(({protocol}, {root, screens, fixtureDir}) => {
    const require = process.getBuiltinModule('module').createRequire(root + '/package.json');
    const fs = require('node:fs');
    const path = require('node:path');
    const guards = require(path.join(root, 'main/guards.cjs'));
    protocol.unhandle('fb');
    protocol.handle('fb', request => {
      const url = new URL(request.url);
      if (request.method !== 'GET' || url.host !== 'app') return new Response('Not found', {status:404});
      if (url.pathname.startsWith('/static/')) return guards.staticFile(path.join(root,'renderer/static'),url.pathname.slice(8));
      const match = /^\/__snapshot__\/([a-z0-9-]+)\/$/.exec(url.pathname);
      const name = match?.[1];
      if (!name || !Object.hasOwn(screens, name)) return new Response('Not found',{status:404});
      let body = fs.readFileSync(path.join(fixtureDir,name+'.html'),'utf8');
      if (screens[name].template) body = `<html><head><link rel="stylesheet" href="/static/ledger/app.css"><link rel="stylesheet" href="/static/salestax/salestax.css"></head><body>${body}</body></html>`;
      return new Response(body,{status:200,headers:{'content-type':'text/html; charset=utf-8','content-security-policy':guards.CSP,'cache-control':'no-store','x-content-type-options':'nosniff'}});
    });
  }, {root, screens, fixtureDir});
  const guards = require(path.join(root, 'main/guards.cjs'));
  let count = 0, hidden = 0;
  for (const [name, spec] of Object.entries(screens)) {
    if (spec.media || (spec.status >= 300 && spec.status < 400)) continue;
    // A release screen diff refuses plugin screens (N14) and writes no fixture for them.
    if (guards.pluginPath(spec.path.split('?')[0]) && !fs.existsSync(path.join(fixtureDir, name + '.html'))) { hidden++; continue; }
    const response = await page.goto(`fb://app/__snapshot__/${name}/`);
    assert.match(response.headers()['content-security-policy'], /script-src 'self';/);
    const icon = await page.evaluate(() => {
      const main=document.querySelector('main.has-page-icon[style]');
      return !main || main.style.getPropertyValue('--page-icon').startsWith('url(');
    });
    assert.ok(icon, `${name} heading icon must survive CSP`);
    await shot(page, 'catalogue-'+name);
    if (await page.evaluate(() => document.documentElement.scrollHeight > innerHeight)) {
      await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
      await page.screenshot({path:path.join(shots,'catalogue-'+name+'-bottom.png'),fullPage:false});
    }
    count++;
  }
  const evidence = {screens:count,hiddenPluginScreens:hidden,width:1440,height:1000,navigation:true,productionCSP:true,fixtureDir};
  fs.writeFileSync(path.join(shots,'catalogue.json'),JSON.stringify(evidence,null,2)+'\n');
  pass(`${count} catalogue screens captured by navigation under the real CSP/preload, heading icons present${hidden ? ` (${hidden} hidden plugin screens skipped)` : ''}`);
};

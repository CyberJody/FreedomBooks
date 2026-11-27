// Part of smoke-getin.cjs: a backup restored from the unlock screen while the main books still exist
// opens as a labelled copy; after a lock, choosing it asks for its own book password even with a
// saved key planted beside it; an interrupted "Use this as my books" is put back at the next start;
// the copy can then be made the working books, and survives quit and relaunch.
'use strict';
const assert=require('node:assert/strict');
const crypto=require('node:crypto');
const fs=require('node:fs');
const path=require('node:path');

async function run({s,launch,close,shot,text,signIn,unlock,pass,data,book,keyring,BOOK_PASSWORD}) {
  const companies=JSON.parse(await s.native('query',[JSON.stringify({type:'Companies'})]));
  const company=companies[0].id;
  await s.page.goto(`fb://app/c/${company}/backups/`);
  await s.page.getByRole('button',{name:'Create backup now',exact:true}).click();
  await s.page.waitForURL(/\/backups\/\?created=/);
  const backup=path.join(data,'backups',new URL(s.page.url()).searchParams.get('created'));
  assert.ok(fs.existsSync(backup));
  // Ask at every open so the unlock screen shows (the restore lives there).
  await s.page.goto('fb://app/settings/book-protection/');
  await s.page.check('input[name=ask_password]');await s.page.fill('#id_current_password',BOOK_PASSWORD);
  await Promise.all([s.page.waitForEvent('load'),s.page.getByRole('button',{name:'Save setting'}).click()]);
  await s.app.evaluate(({Menu})=>Menu.getApplicationMenu().getMenuItemById('lock-books').click());
  await s.page.waitForSelector('#id_password');

  await s.page.locator('summary').filter({hasText:'Restore a backup instead'}).click();
  await s.page.fill('#id_restore_name','Bakery restored');await s.page.fill('#id_restore_secret',BOOK_PASSWORD);
  await shot(s,'10-unlock-restore-open');
  await s.app.evaluate(({dialog},file)=>{dialog.showOpenDialog=async()=>({canceled:false,filePaths:[file]});},backup);
  await s.page.getByRole('button',{name:'Choose a backup to restore…'}).click();
  await signIn(s);
  const copy=path.join(data,'books','restored-bakery-restored','restored-bakery-restored.fbbook');
  assert.equal(JSON.parse(await s.native('appState')).path,copy);
  await s.page.goto(`fb://app/c/${company}/home/`);
  assert.match(await s.page.locator('[data-banner]').innerText(),/restored copy[\s\S]*not your main books/i);
  await shot(s,'11-restored-copy-banner');
  pass('a restore from the unlock screen opens as a clearly labelled copy');

  // A key for the copy, sealed for this keyring, planted as if one had been saved when it was opened.
  const copyKey=await s.native('keyForSealing');
  const sealedCopy=await s.app.evaluate(({safeStorage},k)=>safeStorage.encryptString(k).toString('base64'),copyKey);
  fs.writeFileSync(copy+'.os-key',Buffer.from(sealedCopy,'base64'),{mode:0o600});
  await s.app.evaluate(({Menu})=>Menu.getApplicationMenu().getMenuItemById('lock-books').click());
  await s.page.waitForSelector('#id_password');
  assert.match(await text(s),/You locked FreedomBooks/);
  await s.page.locator('summary').filter({hasText:'Open other books on this computer'}).click();
  await s.page.locator('li').filter({hasText:'Restored copy'}).getByRole('button',{name:'Open these'}).click();
  await s.page.waitForFunction(()=>(document.querySelector('.gate-file')?.innerText||'').includes('restored-bakery-restored'));
  assert.equal(JSON.parse(await s.native('appState')).open,false,'the lock screen did not open the copy through its saved key');
  // The chosen copy inherits the backup's ask-every-time setting and shows its own unlock reason.
  assert.match(await text(s),/ask for the book password each time it opens/);
  assert.ok(await s.page.locator('#id_password').isVisible());
  await shot(s,'11b-lock-screen-chooser-asks-for-the-copy-password');
  await unlock(s,BOOK_PASSWORD);await signIn(s);
  assert.equal(JSON.parse(await s.native('appState')).path,copy);
  pass('after a lock, choosing another book asks for its own book password; a saved key for it is not used');

  // "Use this as my books" interrupted mid-move (as by a power cut): the key record and receipts had
  // moved, the book had not. The next start puts them back before anything opens.
  await close(s);
  const books=path.join(data,'books'), target=path.join(books,'set-aside','20261111T000000Z-default');
  fs.mkdirSync(target,{recursive:true,mode:0o700});
  const moves=[[book+'.keys.json',path.join(target,'default.fbbook.keys.json')]];
  if (fs.existsSync(path.join(books,'receipts'))) moves.push([path.join(books,'receipts'),path.join(target,'receipts')]);
  moves.push([book,path.join(target,'default.fbbook')]);
  const journal={format:1,id:crypto.randomUUID(),phase:'prepared',main:copy,previous:book,set_aside:path.join(target,'default.fbbook'),folder:target,
    moves:moves.map(([from,to])=>({from,to})),actor:crypto.randomUUID(),detail:{}};
  fs.writeFileSync(path.join(books,'set-aside','changing-books.json'),JSON.stringify(journal),{mode:0o600});
  for (const [from,to] of moves.slice(0,-1)) fs.renameSync(from,to);
  s=await launch(keyring);
  await s.page.waitForSelector('#id_password');
  assert.ok(fs.existsSync(book+'.keys.json') && fs.existsSync(book),'the key record is back beside its book');
  assert.ok(!fs.existsSync(path.join(books,'set-aside','changing-books.json')) && !fs.existsSync(target),'the interrupted change is undone and its empty folder gone');
  assert.match(await s.page.locator('.gate-file').innerText(),/default\.fbbook/);
  await shot(s,'11c-interrupted-set-aside-undone');
  await unlock(s,BOOK_PASSWORD);await signIn(s);
  assert.equal(JSON.parse(await s.native('appState')).path,book);
  pass('an interrupted set-aside is undone at the next start: the main book opens with its password');
  await s.app.evaluate(({Menu})=>Menu.getApplicationMenu().getMenuItemById('lock-books').click());
  await s.page.waitForSelector('#id_password');
  await s.page.locator('summary').filter({hasText:'Open other books on this computer'}).click();
  await s.page.locator('li').filter({hasText:'Restored copy'}).getByRole('button',{name:'Open these'}).click();
  await s.page.waitForFunction(()=>(document.querySelector('.gate-file')?.innerText||'').includes('restored-bakery-restored'));
  await unlock(s,BOOK_PASSWORD);await signIn(s);

  await s.app.evaluate(({Menu})=>{const file=Menu.getApplicationMenu().items.find(i=>i.label==='File');file.submenu.items.find(i=>i.id==='use-restored').click();});
  await s.page.waitForURL(/\/settings\/restored-book\/$/);
  assert.match(await text(s),/Use this as my books/);
  assert.match(await text(s),/last changed/);
  await shot(s,'12-use-restored-page');
  await s.page.check('input[name=confirm]');
  await Promise.all([s.page.waitForURL(/\/settings\/restored-book\/\?done=1$/),s.page.getByRole('button',{name:'Use this as my books',exact:true}).click()]);
  assert.match(await text(s),/These are now your books/);
  await shot(s,'13-use-restored-done');
  assert.equal(JSON.parse(fs.readFileSync(path.join(data,'active-book.json'),'utf8')).path,copy);
  assert.ok(!fs.existsSync(book),'the previous main book was set aside');
  const aside=fs.readdirSync(path.join(data,'books','set-aside'));
  assert.equal(aside.length,1);
  assert.ok(fs.existsSync(path.join(data,'books','set-aside',aside[0],'default.fbbook')),'set aside, not deleted');
  assert.ok(fs.existsSync(path.join(data,'books','set-aside',aside[0],'default.fbbook.keys.json')));
  const audit=JSON.parse(await s.native('query',[JSON.stringify({type:'Companies'})]));
  void audit;
  await s.page.goto(`fb://app/c/${company}/history/`);
  assert.match(await text(s),/book\.restored_made_active|restored/i);
  pass('"Use this as my books" persists the copy, sets the old main book aside and audits it');
  await close(s);

  // Relaunch: the restored books are the ones that open. They inherited "ask every time" from the
  // books they were restored from, so they ask for their password.
  s=await launch(keyring);
  await s.page.waitForSelector('#id_password');
  assert.match(await s.page.locator('.gate-file').innerText(),/restored-bakery-restored\.fbbook/);
  await unlock(s,BOOK_PASSWORD);
  await signIn(s);
  assert.equal(JSON.parse(await s.native('appState')).path,copy);
  await s.page.goto(`fb://app/c/${company}/home/`);
  assert.equal(await s.page.locator('[data-banner]').count(),0,'the working books are no longer labelled a copy');
  pass('after quit and relaunch the restored books are the ones that open, unlabelled');
  // Lock with "ask every time" on: the unlock screen names this book and lists the set-aside one.
  await s.page.goto('fb://app/settings/book-protection/');
  await s.page.check('input[name=ask_password]');await s.page.fill('#id_current_password',BOOK_PASSWORD);
  await Promise.all([s.page.waitForEvent('load'),s.page.getByRole('button',{name:'Save setting'}).click()]);
  await s.app.evaluate(({Menu})=>Menu.getApplicationMenu().getMenuItemById('lock-books').click());
  await s.page.waitForSelector('#id_password');
  assert.match(await text(s),/restored-bakery-restored\.fbbook/);
  await s.page.locator('summary').filter({hasText:'Open other books on this computer'}).click();
  assert.match(await text(s),/Set aside: previous main books/);
  await shot(s,'14-unlock-book-chooser');
  // Open the set-aside book from the list: it opens as a copy, and the working books stay the same.
  await s.page.locator('li').filter({hasText:'Set aside'}).getByRole('button',{name:'Open these'}).click();
  await s.page.waitForSelector('#id_password');
  assert.match(await text(s),/set-aside/);
  await unlock(s,BOOK_PASSWORD);await signIn(s);
  await s.page.goto(`fb://app/c/${company}/home/`);
  assert.match(await s.page.locator('[data-banner]').innerText(),/restored copy|not your main books|open as a copy/i);
  await shot(s,'15-set-aside-book-opened-as-copy');
  assert.equal(JSON.parse(fs.readFileSync(path.join(data,'active-book.json'),'utf8')).path,copy);
  pass('the book chooser opens a set-aside book as a labelled copy without changing the working books');
  await close(s);
}
module.exports={run};

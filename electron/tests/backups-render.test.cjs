'use strict';
const test=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const vm=require('node:vm');
const target='fb://app/c/0195b4a4-55af-719e-96cb-a2f946b067bf/backups/';
const tick=()=>new Promise(resolve=>setImmediate(resolve));
function fixture(bankFeeds=false, canLock=true) {
  const state={open:true,signedIn:true,generation:1,pending:[],sync:0,agentRevoked:0};
  const native={setHost(){},pluginsEnabled:()=>false,bankfeedsEnabled:()=>bankFeeds,configureBackups(){},
    aiInvalidate:()=>{state.agentRevoked++;},
    appState:()=>JSON.stringify({open:state.open,can_lock:canLock,signed_in:state.signedIn,session_generation:state.generation}),
    lock:()=>{state.open=false;state.signedIn=false;state.generation++;},
    signOut:()=>{state.signedIn=false;state.generation++;state.agentRevoked++;},
    renderGate:()=>'<h1>Unlock</h1>',render:()=>{state.sync++;throw new Error('Backups must run on worker');},
    run:()=>{throw new Error('Bank requests must run on worker');},
    runAsync:command=>new Promise((resolve,reject)=>state.pending.push({command:JSON.parse(command),resolve,reject})),
    renderAsync:target=>new Promise((resolve,reject)=>state.pending.push({target,resolve,reject})),
  };
  const electron={app:{getVersion:()=>'',commandLine:{appendSwitch(){}},enableSandbox(){},whenReady:()=>new Promise(()=>{}),on(){}},
    protocol:{registerSchemesAsPrivileged(){}},ipcMain:{handle(){}}};
  const stubs={electron,'./development.cjs':{nativeModule:()=>'/native',userData:()=>'/synthetic',netHelper:()=>'/synthetic/fb-net'},'/native':native,
    './keys.cjs':{userDataDir:()=>'/synthetic',store:()=>({exists:()=>true,returnToMain(){}})},
    './identity.cjs':{configure(){},claimInstance:()=>true},'./about.cjs':{osLabel:()=>''},
    './ai-desktop.cjs':{create:()=>({menuItems:[]})},
    './save-file.cjs':{create:()=>({})},'./printing.cjs':{controller:()=>({})},
    './guards.cjs':require('../main/guards.cjs')};
  const sandbox={URL,Response,Buffer,__dirname:path.join(__dirname,'../main'),module:{exports:{}},
    require:n=>Object.hasOwn(stubs,n)?stubs[n]:require(n)};
  vm.runInNewContext(fs.readFileSync(path.join(__dirname,'../main/main.cjs'),'utf8')+'\nmodule.exports={handle,lockNow,calls,getPending:()=>pending};',sandbox);
  return {...sandbox.module.exports,state};
}
const result=()=>JSON.stringify({html:'<h1>Synthetic book Backups</h1>',status:200,route:'backups:page'});
test('ordinary Backups GETs serialize, retain query strings and refuse competing writes',async()=>{
  const f=fixture(),get=()=>f.handle({url:target+'?created=test',method:'GET'});
  const a=get(),b=get(),c=get();await tick();
  assert.equal(f.state.pending.length,1);assert.equal(f.state.sync,0);
  assert.equal(f.state.pending[0].target,new URL(target).pathname+'?created=test');
  await assert.rejects(f.calls['books.run']({}),/busy/);
  f.state.pending[0].resolve(result());assert.equal((await a).status,200);await tick();
  assert.equal(f.state.pending.length,2);
  f.state.pending[1].resolve(result());await b;await tick();
  assert.equal(f.state.pending.length,3);
  f.state.pending[2].resolve(result());assert.match(await (await c).text(),/Backups/);
});
test('deferred lock discards rendered book data and waiting GETs get the gate',async()=>{
  const f=fixture();const a=f.handle({url:target,method:'GET'}),b=f.handle({url:target,method:'GET'});
  await tick();f.lockNow();assert.equal(f.state.open,true);
  f.state.pending[0].resolve(result());
  for (const response of await Promise.all([a,b])) assert.equal(await response.text(),'<h1>Unlock</h1>');
  assert.equal(f.state.open,false);assert.equal(f.state.pending.length,1);
});
test('failed render releases admission for the next GET',async()=>{
  const f=fixture();const a=f.handle({url:target,method:'GET'}),b=f.handle({url:target,method:'GET'});
  await tick();f.state.pending[0].reject(new Error('synthetic disk failure'));
  assert.equal((await a).status,404);await tick();
  assert.equal(f.state.pending.length,2);f.state.pending[1].resolve(result());assert.equal((await b).status,200);
});

test('SimpleFIN connect and sync use worker admission and discard results after locking',async()=>{
  const id='0195b4a4-55af-719e-96cb-a2f946b067bf';
  for(const path of [`/c/${id}/bank-feeds/connect/`,`/c/${id}/bank-feeds/${id}/sync/`]) {
    const f=fixture(true);
    const pending=f.calls['books.run']({type:'SubmitForm',path,token:'synthetic',fields:[]});
    await tick();
    assert.equal(f.state.pending.length,1);
    assert.equal(f.state.pending[0].command.path,path);
    await assert.rejects(f.calls['books.run']({}),/busy/);
    f.lockNow();
    f.state.pending[0].resolve(JSON.stringify({outcome:'page',html:'private bank transactions',status:200}));
    const result=await pending;
    assert.equal(result.outcome,'redirect');assert.equal(result.location,'/');
    assert.equal(f.state.open,false);
    assert.ok(!JSON.stringify(result).includes('private bank'));
  }
});

test('OS/idle lock revokes AI authority even in built-in-key mode without book locking',()=>{
  const f=fixture(false,false);f.lockNow();assert.equal(f.state.open,true);assert.equal(f.state.agentRevoked,1);
});

test('identity-only deferred lock suppresses completed HTML and never stamps an old page as a new session',async()=>{
  const f=fixture(true,false);
  const rendered=f.handle({url:target,method:'GET'});
  await tick();f.lockNow();f.state.pending[0].resolve(result());
  const response=await rendered;
  assert.ok(!(await response.text()).includes('Synthetic book Backups'));
  assert.equal(f.state.open,true);assert.equal(f.state.signedIn,false);
  const g=fixture(true,false);
  const path='/c/0195b4a4-55af-719e-96cb-a2f946b067bf/bank-feeds/connect/';
  const outcome=g.calls['books.run']({type:'SubmitForm',path,token:'synthetic',fields:[]});
  await tick();g.lockNow();g.state.pending[0].resolve(JSON.stringify({outcome:'page',html:'private old bank form',status:200}));
  const settled=await Promise.allSettled([outcome]);
  assert.ok(!JSON.stringify(settled).includes('private old bank form'));
  assert.equal(g.getPending(),null,'old invalid HTML must never be relabelled under the new generation');
});

test('successful authentication may return its new generation, but deferred lock still ends it',async()=>{
  for(const deferredLock of [false,true]) {
    const f=fixture(false,false);
    const operation=f.calls['books.run']({type:'SubmitForm',path:'/desktop/sign-in/',token:'synthetic',fields:[]});
    await tick();
    if(deferredLock)f.lockNow();
    f.state.generation++;
    f.state.pending[0].resolve(JSON.stringify({outcome:'redirect',location:'/c/0195b4a4-55af-719e-96cb-a2f946b067bf/home/'}));
    const outcome=await operation;
    assert.equal(outcome.location,deferredLock?'/desktop/sign-in/':'/c/0195b4a4-55af-719e-96cb-a2f946b067bf/home/');
  }
});


test('OS lock cancels an unfinished MFA sign-in even without a live actor',()=>{
  const f=fixture(false,false);f.state.signedIn=false;
  const generation=f.state.generation;f.lockNow();
  assert.equal(f.state.generation,generation+1,'pending sign-in must be invalidated');
  assert.equal(f.state.agentRevoked,1);assert.equal(f.state.open,true);
});

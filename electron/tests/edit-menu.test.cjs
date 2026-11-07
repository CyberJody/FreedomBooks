'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const {items,attach} = require('../main/edit-menu.cjs');

test('editable text offers native paste, undo and selection commands', () => {
  const menu = items({isEditable:true,inputFieldType:'plainText',editFlags:{canPaste:true,canUndo:true,canSelectAll:true}});
  for (const role of ['paste','undo','selectAll']) assert.equal(menu.find(i=>i.role===role).enabled,true);
  for (const role of ['copy','cut','redo']) assert.equal(menu.find(i=>i.role===role).enabled,false);
});
test('passwords permit paste without exposing copy or cut', () => {
  const menu = items({isEditable:true,inputFieldType:'password',editFlags:{canPaste:true,canCopy:true,canCut:true}});
  assert.equal(menu.find(i=>i.role==='paste').enabled,true);
  assert.equal(menu.find(i=>i.role==='copy').enabled,false);
  assert.equal(menu.find(i=>i.role==='cut').enabled,false);
});
test('noneditable text offers copy only for a selection', () => {
  assert.deepEqual(items({isEditable:false}),[]);
  assert.deepEqual(items({isEditable:false,selectionText:'Selected',editFlags:{canCopy:true}}),[{role:'copy',enabled:true}]);
});
test('right click opens the native menu for the owning window', () => {
  let handler, opened;
  const win={webContents:{on:(event,fn)=>{assert.equal(event,'context-menu');handler=fn;}}};
  attach(win,{buildFromTemplate:template=>({popup:options=>{opened={template,options};}})});
  handler({}, {isEditable:true,editFlags:{canPaste:true}});
  assert.equal(opened.options.window,win);
  assert.equal(opened.template.find(i=>i.role==='paste').enabled,true);
});

'use strict';

// Electron supplies these flags for the clicked field; no clipboard contents cross IPC.
function items(params) {
  const flags = params.editFlags || {};
  const password = params.inputFieldType === 'password';
  if (!params.isEditable) return params.selectionText && !password ? [{role:'copy',enabled:!!flags.canCopy}] : [];
  return [
    {role:'undo',enabled:!!flags.canUndo}, {role:'redo',enabled:!!flags.canRedo},
    {type:'separator'},
    {role:'cut',enabled:!!flags.canCut && !password},
    {role:'copy',enabled:!!flags.canCopy && !password},
    {role:'paste',enabled:!!flags.canPaste},
    {type:'separator'}, {role:'selectAll',enabled:!!flags.canSelectAll},
  ];
}

function attach(win, Menu) {
  win.webContents.on('context-menu', (_event, params) => {
    const template = items(params);
    if (template.length) Menu.buildFromTemplate(template).popup({window:win});
  });
}

module.exports = {items, attach};

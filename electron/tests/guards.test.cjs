'use strict';
// Unit tests for the main-process guards that don't need a window (run with plain Node: npm test).
const test = require('node:test');
const assert = require('node:assert/strict');
const Module = require('node:module');
const path = require('node:path');
const fs = require('node:fs');
const os = require('node:os');

const realLoad = Module._load;
Module._load = function (request, ...rest) {
  if (request === 'electron') return {};
  return realLoad.call(this, request, ...rest);
};
global.Response = global.Response || class { constructor(body, init) { this.body = body; this.status = init.status; } };
const guards = require('../main/guards.cjs');

test('CSP forbids inline script, network and framing', () => {
  assert.match(guards.CSP, /default-src 'none'/);
  assert.match(guards.CSP, /script-src 'self'(;|$)/);
  assert.match(guards.CSP, /connect-src 'none'/);
  assert.doesNotMatch(guards.CSP, /unsafe-inline|unsafe-eval|https?:|\*/);
});

test('commands: only known types, plain objects, bounded size', () => {
  assert.equal(guards.command({ type: 'ExportHistory', company_id: 'c', filters: [] }).type, 'ExportHistory');
  assert.throws(() => guards.command({ type: 'Eval' }), /Unknown command/);
  assert.throws(() => guards.command([]), /Invalid command/);
  assert.throws(() => guards.command(Object.create({ type: 'SaveCustomer' })), /Invalid command/);
  assert.throws(() => guards.command({ type: 'SubmitForm', path: 'x', token: 't', fields: [] }), /Invalid form/);
  assert.throws(() => guards.command({ type: 'SubmitForm', path: '/a/', token: 't', fields: [['a', 1]] }), /Invalid form fields/);
  assert.throws(() => guards.command({ type: 'SubmitForm', display_name: 'x'.repeat(300 * 1024) }), /too large/);
  assert.throws(() => guards.setup({ owner_name: 'a', company_name: 'b', extra: 1 }), /Unknown setup field/);
  assert.throws(() => guards.target({ target: '//evil/' }), /Invalid screen address/);
});

test('static files: no traversal, only known types', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'fb-static-'));
  fs.mkdirSync(path.join(root, 'ledger'));
  fs.writeFileSync(path.join(root, 'ledger', 'app.css'), 'body{}');
  fs.writeFileSync(path.join(os.tmpdir(), 'secret.css'), 'x');
  assert.equal(guards.staticFile(root, 'ledger/app.css').status, 200);
  assert.equal(guards.staticFile(root, '../secret.css').status, 404);
  assert.equal(guards.staticFile(root, '..%2Fsecret.css').status, 404);
  assert.equal(guards.staticFile(root, 'ledger/app.css%00.js').status, 404);
  assert.equal(guards.staticFile(root, 'ledger').status, 404);
  assert.equal(guards.staticFile(root, '%E0%A4%A').status, 400);
});

test('setup accepts only the two starter-chart choices', () => {
  for (const starter_chart of ['standard', 'empty']) {
    const input = { owner_name: 'Owner', company_name: 'Synthetic', starter_chart, protect: 'on', lock_password: 'Synthetic password', lock_password2: 'Synthetic password' };
    assert.deepEqual(guards.setup(input), input);
  }
  for (const starter_chart of ['', 'bogus', 1]) {
    assert.throws(() => guards.setup({ starter_chart }), /Choose a chart|Invalid setup/);
  }
});

test('numbering commands and queries use the fixed bridge contract', () => {
  assert.ok(guards.appCommands.includes('SetNextNumber'));
  assert.throws(()=>guards.command({type:'SetNextNumber',company_id:'c',kind:'INV',bank_id:null,next:'100',expected_next:'1',expected_revision:0}),/Unknown command/);
  assert.equal(guards.query({type:'NumberingSettings',company_id:'c'}).type,'NumberingSettings');
});

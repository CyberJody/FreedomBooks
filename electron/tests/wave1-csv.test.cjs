'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const guards = require('../main/guards.cjs');
const remote = require('../main/remote-mode.cjs');

test('direct CSV route classifier covers all report, banking and history routes across local and remote handlers', () => {
  const reportRoutes = [
    ['profit_loss', '/c/11111111-1111-4111-8111-111111111111/reports/profit-and-loss/?output=csv'],
    ['balance_sheet', '/c/11111111-1111-4111-8111-111111111111/reports/balance-sheet/?output=csv'],
    ['general_ledger', '/c/11111111-1111-4111-8111-111111111111/reports/general-ledger/?output=csv'],
    ['ar_aging', '/c/11111111-1111-4111-8111-111111111111/reports/ar-aging/?output=csv'],
    ['ap_aging', '/c/11111111-1111-4111-8111-111111111111/reports/ap-aging/?output=csv'],
    ['vendor_1099', '/c/11111111-1111-4111-8111-111111111111/reports/vendor-1099/?output=csv'],
  ];
  for (const [route, path] of reportRoutes) {
    const url = new URL(path, 'fb://app');
    assert.equal(guards.csvExportFilename(route, url, 200, 'a,b\n'), 'report.csv', route);
    assert.equal(remote.csvPage(route, url, 200), true, route);
    assert.equal(guards.csvExportFilename(route, url, 400, 'err'), null, `${route} 400`);
    assert.equal(remote.csvPage(route, url, 400), false, `${route} remote 400`);

    const repeatedCsvLast = new URL(`${path.split('?')[0]}?output=print&output=csv`, 'fb://app');
    assert.equal(guards.csvExportFilename(route, repeatedCsvLast, 200, 'a,b\n'), 'report.csv', `${route} last=csv`);
    assert.equal(remote.csvPage(route, repeatedCsvLast, 200), true, `${route} remote last=csv`);

    const repeatedPrintLast = new URL(`${path.split('?')[0]}?output=csv&output=print`, 'fb://app');
    assert.equal(guards.csvExportFilename(route, repeatedPrintLast, 200, '<html>'), null, `${route} last=print`);
    assert.equal(remote.csvPage(route, repeatedPrintLast, 200), false, `${route} remote last=print`);
  }

  for (const route of ['register', 'reconcile:report']) {
    const url = new URL('/c/11111111-1111-4111-8111-111111111111/accounts/22222222-2222-4222-8222-222222222222/register/?output=csv', 'fb://app');
    assert.equal(guards.csvExportFilename(route, url, 200, 'a,b\n'), 'banking.csv', route);
    assert.equal(remote.csvPage(route, url, 200), true, route);
  }

  const historyUrl = new URL('/c/11111111-1111-4111-8111-111111111111/history/?format=csv', 'fb://app');
  assert.equal(guards.csvExportFilename('history', historyUrl, 200, '\uFEFFcol\n'), 'audit-history.csv');
  assert.equal(guards.csvExportFilename('history', historyUrl, 200, '<html>'), null);
  assert.equal(remote.csvPage('history', historyUrl, 200, '\uFEFFcol\n'), true);
  assert.equal(remote.csvPage('history', historyUrl, 200, '<html>'), false);

  const historyCsvLast = new URL('/c/11111111-1111-4111-8111-111111111111/history/?format=html&format=csv', 'fb://app');
  assert.equal(guards.csvExportFilename('history', historyCsvLast, 200, '\uFEFFcol\n'), 'audit-history.csv');
  assert.equal(remote.csvPage('history', historyCsvLast, 200, '\uFEFFcol\n'), true);

  const historyHtmlLast = new URL('/c/11111111-1111-4111-8111-111111111111/history/?format=csv&format=html', 'fb://app');
  assert.equal(guards.csvExportFilename('history', historyHtmlLast, 200, '\uFEFFcol\n'), null);
  assert.equal(remote.csvPage('history', historyHtmlLast, 200, '\uFEFFcol\n'), false);

  const nonCsv = new URL('/c/11111111-1111-4111-8111-111111111111/reports/ar-aging/', 'fb://app');
  assert.equal(guards.csvExportFilename('ar_aging', nonCsv, 200, '<html>'), null);
  assert.equal(remote.csvPage('ar_aging', nonCsv, 200), false);
});

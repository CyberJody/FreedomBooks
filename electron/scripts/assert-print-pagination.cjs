'use strict';
const assert = require('node:assert/strict');
const {spawnSync} = require('node:child_process');

// Text presence alone misses Chromium pagination regressions that paint totals
// over the final rows. Check the actual PDF's text coordinates as well.
module.exports = function assertInvoiceTotalsFollowRows(file, lastLineWords) {
  const result = spawnSync('pdftotext', ['-bbox-layout', file, '-'], {encoding: 'utf8'});
  assert.equal(result.status, 0, result.stderr);
  const pages = [...result.stdout.matchAll(/<page\b[^>]*>([\s\S]*?)<\/page>/g)];
  const finalRows = [], totals = [];
  pages.forEach((page, index) => {
    for (const block of page[1].matchAll(/<block\b([^>]*)>([\s\S]*?)<\/block>/g)) {
      const words = [...block[2].matchAll(/<word\b[^>]*>([^<]*)<\/word>/g)].map(word => word[1]);
      if (lastLineWords.every(word => words.includes(word))) {
        finalRows.push({page: index, bottom: Number(/yMax="([\d.]+)"/.exec(block[1])[1])});
      }
    }
    for (const word of page[1].matchAll(/<word\b([^>]*)>Subtotal<\/word>/g)) {
      totals.push({page: index, top: Number(/yMin="([\d.]+)"/.exec(word[1])[1])});
    }
  });
  assert.equal(finalRows.length, 1, 'Find the final invoice row in the PDF');
  assert.equal(totals.length, 1, 'Find the invoice subtotal in the PDF');
  const row = finalRows[0], total = totals[0];
  assert.ok(total.page > row.page || (total.page === row.page && total.top > row.bottom),
    `Invoice totals must follow the final row without overlap: ${JSON.stringify({row, total})}`);
};

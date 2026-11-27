'use strict';
const test=require('node:test'),assert=require('node:assert/strict');
const {fixed,money}=require('../renderer/static/sales/editor.js');
test('document totals accept ordinary backend decimal formats without floating point',()=>{
  for(const [quantity,rate,expected] of [['.5','10','5.00'],['1','10.000','10.00'],['1.00000','+.50','0.50'],['0.125','0.04','0.01'],['1.2345','17.23','21.27']]){
    assert.equal(money((fixed(quantity,4)*fixed(rate,2)+5000n)/10000n),expected);
  }
  assert.equal(fixed('1.23456',4),null);
  assert.equal(fixed('1e2',2),null); // Accepted by the server; preview defers calculation to Save.
  assert.equal(fixed('',2),null);
  assert.equal(fixed('.',2),null);
});

test('invoice preview keeps discount signs including sub-dollar amounts',()=>{
  const {lineAmount}=require('../renderer/static/sales/editor.js');
  assert.equal(money(lineAmount('1','84.03','DISCOUNT')),'-84.03');
  assert.equal(money(lineAmount('1','.01','DISCOUNT')),'-0.01');
  assert.equal(lineAmount('0','0','NOTE'),0n);
  assert.equal(lineAmount('1','0','NOTE'),null);
  assert.equal(lineAmount('0','1','CHARGE'),null);
  assert.equal(lineAmount('1','-1','DISCOUNT'),null);
});

test('invoice tax rounds each component exactly and uses net taxable discounts',()=>{
  const {invoiceTax}=require('../renderer/static/sales/editor.js');
  assert.equal(invoiceTax(1n,['4','4']),0n); // Never round the combined rate instead.
  assert.equal(invoiceTax(5994n,['4','3']),420n);
  assert.equal(invoiceTax(0n,['7']),0n);
  assert.equal(invoiceTax(100n,[]),0n);
  assert.equal(invoiceTax(-1n,['7']),null);
  assert.equal(invoiceTax(100n,undefined),null);
  assert.equal(invoiceTax(100n,['NaN']),null);
});

test('sales receipt and credit memo previews post what the lines show, by account',()=>{
  const {cents,plan}=require('../renderer/static/sales/document-forms.js');
  assert.equal(cents(money(123450n)),123450n); // Read the formatted amount shown by the line editor.
  assert.equal(money(123450n),'1,234.50');
  assert.equal(cents('12,34.50'),null);
  assert.equal(cents('30.00'),3000n);
  const accounts={kind:'SR',income:{a:'4000 · Sales',b:'4100 · Delivery'},collected_tax:'2200 · Sales tax payable',receivables:'1100 · A/R'};
  const lines=[{account:'a',amount:3000n},{account:'b',amount:500n},{account:'a',amount:1000n}];
  // A sale debits where the money went and credits income per account, then the tax.
  assert.deepEqual(plan('SR',lines,315n,accounts,'1200 · Undeposited funds'),[
    {account:'1200 · Undeposited funds',debit:4815n},
    {account:'4000 · Sales',credit:4000n},{account:'4100 · Delivery',credit:500n},
    {account:'2200 · Sales tax payable',credit:315n}]);
  // A credit memo is the reverse against receivables, and balances.
  const cm=plan('CM',lines,0n,{...accounts,kind:'CM'},'');
  assert.deepEqual(cm,[{account:'4000 · Sales',debit:4000n},{account:'4100 · Delivery',debit:500n},{account:'1100 · A/R',credit:4500n}]);
  assert.equal(cm.reduce((s,r)=>s+(r.debit||0n)-(r.credit||0n),0n),0n);
  assert.equal(plan('SR',[{account:'',amount:100n}],0n,accounts,'Bank')[1].account,'Income account not chosen');
});

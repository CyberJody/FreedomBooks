'use strict';
// The line editor shared by invoices, estimates, sales receipts and credit memos. The editor's
// data-document attribute names the document; an invoice has none and keeps its behaviour.
(() => {
  const catalogNode = document.getElementById('invoice-catalog');
  if (!catalogNode) return;
  const editor = document.querySelector('.document-editor'), documentKind = editor?.dataset.document || 'invoice';
  const catalog = JSON.parse(catalogNode.textContent), lines = document.getElementById('invoice-lines');
  const read = (row, key) => row.querySelector(`[name$="-${key}"]`);
  const hasContent = row => ['item', 'description', 'quantity', 'rate'].some(key => read(row, key)?.value.trim());
  const describe = row => {
    const kind = read(row, 'line_kind')?.value || 'CHARGE', removed = read(row, 'DELETE').checked;
    const kindLabel = row.querySelector('[data-line-kind]');
    if (kindLabel) kindLabel.textContent = kind === 'DISCOUNT' ? 'Discount / credit' : kind === 'NOTE' ? 'Note' : '';
    row.classList.toggle('is-removed', removed);
    const button = row.querySelector('[data-remove-line]');
    button.textContent = removed ? 'Undo' : '×';
    button.setAttribute('aria-label', removed ? 'Restore line' : 'Remove line');
    const note = kind === 'NOTE';
    row.classList.toggle('is-note',note);
    const description=read(row,'description');description.style.height='auto';description.style.height=Math.min(240,Math.max(38,description.scrollHeight+2))+'px';
    // Read-only, not disabled: note's explicit zero values still reach the server.
    for (const key of ['quantity', 'rate']) read(row, key).readOnly = note;
    read(row, 'item').disabled = note;
    read(row, 'taxable').disabled = note;
    row.querySelectorAll('.line-field input, .line-field select, .line-field textarea').forEach(field => {field.tabIndex=removed?-1:0;});
    const account = read(row, 'income_account'), item = read(row, 'item').value;
    if (documentKind === 'invoice') {
      const help = row.querySelector('[data-line-help]');
      help.hidden = removed || note || !read(row, 'description').value || !!account.value;
      help.textContent = item ? 'This item needs an income account in Items before the invoice can be posted. You can still save a draft.' : 'Select a product or service before posting. You can still save a draft.';
    } else if (account) {
      // The income account comes from the item. Ask for it only on a line without an item, or
      // when the chosen item has none, or when it was refused.
      const failed = !!row.querySelector('[data-field="income_account"] .errorlist');
      row.classList.toggle('needs-account', !removed && (failed || (item ? !catalog[item]?.income_account : hasContent(row))));
    }
  };
  lines.addEventListener('change', event => {
    const row = event.target.closest('fieldset'); if (!row) return;
    if (event.target.name.endsWith('-item')) {
      const item = catalog[event.target.value];
      // The invoice asks for the item list's current details whenever the item is chosen again.
      const refresh = read(row, 'refresh_item');
      if (documentKind === 'invoice' && refresh) refresh.value = item ? '1' : '';
      // Clearing an item must not leave its old accounting default attached.
      const account = read(row, 'income_account');
      if (account) account.value = item?.income_account || '';
      if (item) {
        // A line needs a description: an item saved without one gives its name (worker item 6).
        read(row, 'description').value = item.description || item.code || '';
        read(row, 'rate').value = item.rate === 'None' ? '' : item.rate;
        if (!read(row, 'quantity').value) read(row, 'quantity').value = '1';
      }
    }
    describe(row);
  });
  lines.addEventListener('input', event => {const row=event.target.closest('fieldset');if(row)describe(row);});
  lines.addEventListener('click', event => {
    const button = event.target.closest('[data-remove-line]'); if (!button) return;
    const row=button.closest('fieldset'),deleted=read(row,'DELETE');deleted.checked=!deleted.checked;
    describe(row);deleted.dispatchEvent(new Event('change',{bubbles:true}));
  });
  const add = (kind='CHARGE') => {
    const count = document.getElementById('id_lines-TOTAL_FORMS'), index = Number(count.value);
    if (index >= 50) return;
    const holder = document.createElement('div');
    holder.innerHTML = document.getElementById('empty-line').innerHTML.replaceAll('__prefix__',String(index));
    const row=holder.firstElementChild;
    if(read(row,'line_kind'))read(row,'line_kind').value=kind;
    if(kind==='NOTE'){read(row,'quantity').value='0';read(row,'rate').value='0';read(row,'tax_auto').value='';}
    if(kind==='DISCOUNT')read(row,'quantity').value='1';
    lines.append(row);count.value=String(index+1);describe(row);
    read(row,kind==='NOTE'?'description':'item').focus();
  };
  document.getElementById('add-line').addEventListener('click',()=>add());
  document.querySelectorAll('[data-add-kind]').forEach(button=>button.addEventListener('click',()=>add(button.dataset.addKind)));
  lines.querySelectorAll('fieldset').forEach(describe);
  const customersNode = document.getElementById('invoice-customers'), customer = document.querySelector('[name="customer"]');
  if (customersNode && customer) {
    const customers = JSON.parse(customersNode.textContent);
    const billTo=()=>{const data=customers[customer.value];document.querySelector('[data-bill-to]').textContent=data?.billing_address || (data?'No billing address on file':'Select a customer');document.querySelector('[data-customer-email]').textContent=data?.email || '';};
    customer.addEventListener('change',billTo);billTo();
  }
  const plusDays=(iso,days)=>{if(!/^\d{4}-\d{2}-\d{2}$/.test(iso))return null;const[y,m,d]=iso.split('-').map(Number),day=new Date(Date.UTC(y,m-1,d));day.setUTCDate(day.getUTCDate()+days);return day.toISOString().slice(0,10);};
  const date=document.querySelector('[name="date"]');
  // An estimate's expiry follows its date (30 days) until the person sets an expiry of their own.
  const expiry=document.querySelector('[name="expiry"]');
  if (date && expiry) {
    let follows = expiry.value !== '' && expiry.value === plusDays(date.value, 30);
    date.addEventListener('change',()=>{if(follows){const next=plusDays(date.value,30);if(next)expiry.value=next;}});
    expiry.addEventListener('change',()=>{follows=false;});
  }
})();

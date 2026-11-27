'use strict';
// Sales receipts and credit memos: the Bank account field appears only for "Deposit to: Bank
// account", and "Show accounting entries" previews what Save will post from the amounts the line
// editor shows. The server reviews and records the same entries in one step when Save is pressed.
(() => {
  const cents = text => {
    const m = /^(-?)(\d+|\d{1,3}(?:,\d{3})+)\.(\d{2})$/.exec((text || '').trim());
    return m ? (m[1] ? -1n : 1n) * (BigInt(m[2].replaceAll(',', '')) * 100n + BigInt(m[3])) : null;
  };
  const money = n => `${n < 0n ? '-' : ''}${(n < 0n ? -n : n) / 100n}.${String((n < 0n ? -n : n) % 100n).padStart(2, '0')}`;
  const grouped = n => money(n).replace(/\B(?=(\d{3})+(?!\d))(?=.*\.)/g, ',');
  // Debits and credits for a sales receipt (SR) or credit memo (CM) from shown line amounts.
  const plan = (kind, lines, tax, accounts, destination) => {
    const income = new Map();
    for (const line of lines) income.set(line.account, (income.get(line.account) || 0n) + line.amount);
    const total = [...income.values()].reduce((a, b) => a + b, 0n) + tax;
    const rows = [...income].map(([account, amount]) => ({ account: accounts.income?.[account] || 'Income account not chosen', amount }));
    if (tax > 0n) rows.push({ account: accounts.collected_tax || 'Sales tax payable (set it up in Accounting setup)', amount: tax });
    if (kind === 'SR') return [{ account: destination, debit: total }, ...rows.map(r => ({ account: r.account, credit: r.amount }))];
    return [...rows.map(r => ({ account: r.account, debit: r.amount })), { account: accounts.receivables || 'Accounts receivable (set it up in Accounting setup)', credit: total }];
  };
  if (typeof document === 'undefined') { module.exports = { cents, plan }; return; }
  const editor = document.querySelector('.document-editor'); if (!editor) return;
  const form = editor.querySelector('form'); if (!form) return;
  // Back to a form that was already saved: the browser shows its fields again but the page carries
  // a new request key, so saving would record a second document. The key the form was submitted
  // with is remembered for this page; coming Back puts it in again, and saving then shows the
  // document already recorded instead of recording another.
  const key = form.elements.request_key;
  if (key) {
    try {
      const slot = `fb-submitted-key:${location.pathname}`, entry = performance.getEntriesByType('navigation')[0];
      const saved = sessionStorage.getItem(slot);
      if (saved && entry && entry.type === 'back_forward') key.value = saved;
      form.addEventListener('submit', () => { try { sessionStorage.setItem(slot, key.value); } catch {} });
    } catch {}
  }
  const destination = form.elements.destination, bank = form.elements.bank, bankField = form.querySelector('[data-bank-field]');
  if (destination && bank && bankField) {
    const showBank = () => {
      const direct = destination.value === 'BANK';
      bankField.hidden = !direct;
      if (!direct) bank.value = '';
    };
    destination.addEventListener('change', showBank); showBank();
  }
  const accountsNode = document.getElementById('posting-accounts'), body = editor.querySelector('[data-entries]');
  if (!accountsNode || !body) return;
  const accounts = JSON.parse(accountsNode.textContent), catalog = JSON.parse(document.getElementById('invoice-catalog').textContent);
  const lines = document.getElementById('invoice-lines');
  const cell = (text, cls) => { const td = document.createElement('td'); if (cls) td.className = cls; td.textContent = text; return td; };
  const message = text => { const tr = document.createElement('tr'), td = cell(text); td.colSpan = 3; tr.append(td); body.replaceChildren(tr); };
  const refresh = () => {
    const shown = [];
    for (const row of lines.querySelectorAll('fieldset')) {
      const read = key => row.querySelector(`[name$="-${key}"]`), text = row.querySelector('[data-line-amount]')?.textContent || '';
      if (read('DELETE')?.checked || text === '—') continue;
      const amount = cents(text);
      if (amount === null) return message('The entries appear when every line has a quantity and rate.');
      const item = read('item')?.value;
      shown.push({ amount, account: read('income_account')?.value || catalog[item]?.income_account || '' });
    }
    if (!shown.length) return message('Enter a line to see the entries.');
    const taxText = editor.querySelector('[data-tax]')?.textContent || '0.00', tax = cents(taxText);
    if (tax === null) return message('The entries appear once the tax can be calculated.');
    const direct = destination?.value === 'BANK';
    const to = direct ? (bank?.selectedOptions[0]?.value ? bank.selectedOptions[0].textContent : 'Bank account not chosen') : (accounts.undeposited || 'Undeposited funds (set it up in Accounting setup)');
    body.replaceChildren(...plan(accounts.kind, shown, tax, accounts, to).map(row => {
      const tr = document.createElement('tr');
      tr.append(cell(row.account), cell(row.debit !== undefined ? grouped(row.debit) : '', 'num'), cell(row.credit !== undefined ? grouped(row.credit) : '', 'num'));
      return tr;
    }));
  };
  // Runs after the line editor has updated the amounts it shows (its listeners were added first).
  form.addEventListener('input', refresh); form.addEventListener('change', refresh);
  new MutationObserver(refresh).observe(lines, { childList: true }); refresh();
})();

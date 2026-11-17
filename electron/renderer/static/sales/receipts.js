'use strict';
// Receive payment: the page proposes how a payment applies to open invoices (oldest first); the
// server validates and records it. Money is handled as integer cents, never floating point.
(() => {
  const cents = text => {
    const m = /^\s*(\d{1,14})(?:\.(\d{0,2}))?\s*$/.exec(text || '');
    return m ? BigInt(m[1]) * 100n + BigInt((m[2] || '').padEnd(2, '0')) : null;
  };
  const money = n => `${n < 0n ? '-' : ''}${(n < 0n ? -n : n) / 100n}.${String((n < 0n ? -n : n) % 100n).padStart(2, '0')}`;
  const grouped = n => money(n).replace(/\B(?=(\d{3})+(?!\d))(?=.*\.)/g, ',');
  const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
  const shortDate = iso => { const m = /^(\d{4})-(\d{2})-(\d{2})/.exec(iso || ''); return m ? `${MONTHS[Number(m[2]) - 1]} ${Number(m[3])}, ${m[1]}` : (iso || ''); };
  // Oldest first: amounts fill each open balance in turn until the payment is used up.
  const autoApply = (received, balances) => {
    let left = received; return balances.map(b => { const n = left > b ? b : left; left -= n; return n > 0n ? n : 0n; });
  };
  if (typeof document === 'undefined') { module.exports = { cents, money, grouped, shortDate, autoApply }; return; }
  const form = document.getElementById('receive-payment'); if (!form) return;
  const data = id => { try { return JSON.parse(document.getElementById(id).textContent); } catch { return null; } };
  const open = data('open-invoices') || [], accounts = data('payment-accounts') || {};
  const customer = form.elements.customer, amount = form.elements.amount, destination = form.elements.destination, bank = form.elements.bank;
  const body = document.getElementById('invoice-rows'), total = document.getElementById('id_allocations-TOTAL_FORMS');
  const table = form.querySelector('[data-table]'), empty = form.querySelector('[data-empty]'), summary = form.querySelector('[data-summary]');
  const appliedCell = form.querySelector('[data-applied]'), entries = form.querySelector('[data-entries]'), bankField = form.querySelector('[data-bank-field]');
  const rows = () => [...body.querySelectorAll('tr[data-invoice]')];
  const pay = row => row.querySelector('input[type=checkbox]'), apply = row => row.querySelector('input[name$="-amount"]');
  const balance = row => cents(row.dataset.balance) ?? 0n;
  const appliedTotal = () => rows().reduce((sum, row) => sum + (cents(apply(row).value) ?? 0n), 0n);
  // The received amount follows the applied total until the person types an amount of their own.
  let typed = amount.value.trim() !== '' && cents(amount.value) !== appliedTotal();
  const cell = (tag, text, cls) => { const el = document.createElement(tag); if (cls) el.className = cls; el.textContent = text; return el; };
  const buildRow = (invoice, index) => {
    const tr = document.createElement('tr');
    tr.dataset.invoice = invoice.invoice_id; tr.dataset.balance = invoice.outstanding; tr.dataset.receivables = invoice.receivables || '';
    const payCell = cell('td', '', 'pay');
    const hidden = Object.assign(document.createElement('input'), { type: 'hidden', name: `allocations-${index}-invoice`, value: invoice.invoice_id });
    const box = Object.assign(document.createElement('input'), { type: 'checkbox', name: `allocations-${index}-pay`, id: `id_allocations-${index}-pay` });
    box.setAttribute('aria-label', `Pay ${invoice.number}`); payCell.append(hidden, box);
    const head = cell('th', invoice.number); head.scope = 'row';
    const due = cell('td', shortDate(invoice.due_date));
    if (invoice.overdue) { due.append(' ', cell('span', 'Overdue', 'status-pill attention')); }
    const applyCell = cell('td', '', 'num apply');
    const input = Object.assign(document.createElement('input'), { type: 'text', name: `allocations-${index}-amount`, id: `id_allocations-${index}-amount`, inputMode: 'decimal', maxLength: 32, autocomplete: 'off' });
    input.setAttribute('aria-label', `Amount to apply to ${invoice.number}`); applyCell.append(input);
    tr.append(payCell, head, cell('td', shortDate(invoice.date)), due, cell('td', grouped(cents(invoice.total) ?? 0n), 'num'), cell('td', grouped(cents(invoice.outstanding) ?? 0n), 'num'), applyCell);
    return tr;
  };
  const showCustomer = () => {
    const mine = open.filter(i => i.customer_id === customer.value);
    body.replaceChildren(...mine.map(buildRow)); total.value = String(mine.length);
    table.hidden = mine.length === 0; empty.hidden = mine.length > 0;
    empty.textContent = customer.value ? 'This customer has no open invoices. To record money received before you invoice, use Receive customer deposit.' : 'Choose a customer to see their open invoices.';
    if (typed) spread(); else amount.value = '';
  };
  const spread = () => {
    const received = cents(amount.value); if (received === null) return;
    const list = rows(), amounts = autoApply(received, list.map(balance));
    list.forEach((row, i) => { pay(row).checked = amounts[i] > 0n; apply(row).value = amounts[i] > 0n ? money(amounts[i]) : ''; });
  };
  const refresh = () => {
    const applied = appliedTotal(), received = cents(amount.value);
    appliedCell.textContent = grouped(applied);
    if (!rows().length) summary.textContent = '';
    else if (received === null) summary.textContent = amount.value.trim() ? 'Enter the amount received as a number, such as 125.00.' : `Applied ${grouped(applied)}`;
    else if (received === applied) summary.textContent = `Received ${grouped(received)} · applied in full`;
    else if (received > applied) summary.textContent = `Received ${grouped(received)} · ${grouped(received - applied)} still to apply`;
    else summary.textContent = `Received ${grouped(received)} · applied ${grouped(applied - received)} more than received`;
    summary.classList.toggle('attention', received !== null && received !== applied && rows().length > 0);
    // What posts: the deposit account is debited with the payment; each invoice's receivables account is credited.
    const credits = new Map();
    for (const row of rows()) { const n = cents(apply(row).value); if (n && n > 0n) { const key = row.dataset.receivables || 'Accounts receivable'; credits.set(key, (credits.get(key) || 0n) + n); } }
    const debit = received ?? applied;
    const target = destination.value === 'BANK' ? (bank.value ? bank.options[bank.selectedIndex].text : 'Bank account (choose one)') : (accounts.undeposited || 'Undeposited funds (set it up in Accounting setup)');
    const line = (account, dr, cr) => { const tr = document.createElement('tr'); tr.append(cell('td', account), cell('td', dr, 'num'), cell('td', cr, 'num')); return tr; };
    entries.replaceChildren(...(debit > 0n || credits.size ? [line(target, grouped(debit), ''), ...[...credits].map(([account, n]) => line(account, '', grouped(n)))] : [line('Enter an amount and choose invoices to see the entries.', '', '')]));
  };
  const toggleBank = () => { const on = destination.value === 'BANK'; bankField.hidden = !on; bank.disabled = !on; };
  customer.addEventListener('change', () => { showCustomer(); refresh(); });
  amount.addEventListener('input', () => { typed = amount.value.trim() !== ''; if (typed) spread(); else rows().forEach(row => { pay(row).checked = false; apply(row).value = ''; }); refresh(); });
  body.addEventListener('change', event => {
    const row = event.target.closest('tr'); if (!row || event.target.type !== 'checkbox') return;
    if (event.target.checked) {
      const left = (cents(amount.value) ?? 0n) - (appliedTotal() - (cents(apply(row).value) ?? 0n));
      apply(row).value = money(typed && left > 0n && left < balance(row) ? left : balance(row));
    } else apply(row).value = '';
    if (!typed) amount.value = appliedTotal() > 0n ? money(appliedTotal()) : '';
    refresh();
  });
  body.addEventListener('input', event => {
    const row = event.target.closest('tr'); if (!row || event.target.type === 'checkbox') return;
    pay(row).checked = event.target.value.trim() !== '';
    if (!typed) amount.value = appliedTotal() > 0n ? money(appliedTotal()) : '';
    refresh();
  });
  destination.addEventListener('change', () => { toggleBank(); refresh(); });
  bank.addEventListener('change', refresh);
  toggleBank(); refresh();
})();

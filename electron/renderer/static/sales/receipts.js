'use strict';
// Receive payment: the page proposes how a payment applies to open invoices (oldest first); the
// server validates and records it. Money is handled as integer cents, never floating point.
// Worker item 39: a typed amount fills the invoices the person chose first (ticked, typed into, or the one
// the page was opened from; with none chosen, the oldest open invoice counts as chosen), then the other
// open invoices oldest first. Money put on an invoice the person did not choose is shown at the amount box,
// and each row says whether it was chosen (`allocations-N-chosen`), so the server asks for a second Save
// before saving it. An invoice the person unticked is not filled again.
// Worker review M5: a typed amount stays as typed when invoices are unticked; money no invoice takes is said at
// the amount box, since a payment keeps no unapplied credit and the server refuses it until the two agree.
// Worker review 3b M7: the fee lines count only while the payment goes straight to a bank. Hidden, they are
// disabled, so they are not sent (what was typed comes back if a bank is chosen again). With fee lines the
// payment posts two entries, and the preview shows both: the payment into Undeposited Funds, then the
// deposit to the bank net of the fees.
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
  const extraNote = document.getElementById('amount-extra');
  let edited = false; // a change since the page was drawn: the server's own warning no longer describes it
  if (extraNote) amount.setAttribute('aria-describedby', [amount.getAttribute('aria-describedby'), 'amount-extra'].filter(Boolean).join(' '));
  const chosenBox = row => {
    let box = row.querySelector('input[name$="-chosen"]');
    if (!box) {
      const index = /^allocations-(\d+)-amount$/.exec(apply(row).name)?.[1];
      box = Object.assign(document.createElement('input'), { type: 'hidden', name: `allocations-${index}-chosen` });
      row.querySelector('td.pay').append(box);
    }
    return box;
  };
  const choose = (row, on) => { row.dataset.chosen = on ? '1' : '0'; };
  // A row whose amount changes loses the server's error about its old amount.
  const clearError = row => {
    const input = apply(row);
    row.classList.remove('has-error');
    input.removeAttribute('aria-invalid'); input.removeAttribute('aria-describedby');
    row.querySelector(`[id="${input.id.replace(/-amount$/, '-error')}"]`)?.remove();
  };
  const setApplied = (row, value) => { if (apply(row).value !== value) { apply(row).value = value; clearError(row); } };
  const balance = row => cents(row.dataset.balance) ?? 0n;
  const appliedTotal = () => rows().reduce((sum, row) => sum + (cents(apply(row).value) ?? 0n), 0n);
  // After a refused save, a row the page filled comes back marked 0 with its amount. The person did not untick
  // it, so a new amount may fill it again; and its money was spread from an amount the person typed.
  const filled = rows().filter(row => row.dataset.chosen === '0' && apply(row).value.trim() !== '');
  filled.forEach(row => { delete row.dataset.chosen; });
  // The received amount follows the applied total until the person types an amount of their own (or the page
  // comes back with money it spread from one, as after the server's warning).
  let typed = filled.length > 0 || !!extraNote?.hasAttribute('data-server') || (amount.value.trim() !== '' && cents(amount.value) !== appliedTotal());
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
    const list = rows();
    let mine = list.filter(row => row.dataset.chosen === '1');
    const others = list.filter(row => row.dataset.chosen === undefined);
    if (!mine.length && others.length) { const first = others.shift(); choose(first, true); mine = [first]; }
    const order = [...mine, ...others], amounts = autoApply(received, order.map(balance));
    list.forEach(row => {
      const i = order.indexOf(row), n = i < 0 ? 0n : amounts[i];
      pay(row).checked = n > 0n; setApplied(row, n > 0n ? money(n) : '');
    });
  };
  // What the page put on invoices the person did not choose, and what no invoice takes, said at the amount box.
  const showExtra = () => {
    for (const row of rows()) chosenBox(row).value = row.dataset.chosen === '1' ? '1' : '0';
    if (!extraNote || (!edited && extraNote.hasAttribute('data-server'))) return;
    const extra = rows().map(row => [row, cents(apply(row).value)]).filter(([row, n]) => row.dataset.chosen !== '1' && n !== null && n > 0n);
    const received = cents(amount.value);
    extraNote.removeAttribute('data-server'); extraNote.setAttribute('role', 'status');
    const words = [];
    if (extra.length && received !== null) {
      const total = extra.reduce((sum, [, n]) => sum + n, 0n);
      const number = row => row.querySelector('th').textContent;
      const names = extra.map(([row, n]) => extra.length === 1 ? number(row) : `${number(row)} (${grouped(n)})`);
      const list = names.length === 1 ? names[0] : `${names.slice(0, -1).join(', ')} and ${names.at(-1)}`;
      words.push(`${grouped(total)} of the ${grouped(received)} would go to ${list}, which you did not choose. Saving asks you to confirm this.`);
    }
    const left = received === null || !rows().length ? 0n : received - appliedTotal();
    if (left > 0n) words.push(`${grouped(left)} of the ${grouped(received)} received is not applied to an invoice. Apply it to an invoice, or correct the amount received: FreedomBooks does not keep part of a payment as unapplied credit, and the payment is not saved until the amounts applied add up to the amount received.`);
    extraNote.hidden = words.length === 0;
    extraNote.textContent = words.join(' ');
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
    showExtra();
    // What posts: the deposit account is debited with the payment; each invoice's receivables account is credited.
    const credits = new Map();
    for (const row of rows()) { const n = cents(apply(row).value); if (n && n > 0n) { const key = row.dataset.receivables || 'Accounts receivable'; credits.set(key, (credits.get(key) || 0n) + n); } }
    // Worker item 47: fee and adjustment lines (straight to a bank only) come off what the bank gets.
    const fees = destination.value === 'BANK' && feeBox && !feeBox.disabled ? feeLines() : [];
    const gross = received ?? applied;
    const net = gross + fees.reduce((sum, [, n]) => sum + n, 0n);
    const held = accounts.undeposited || 'Undeposited funds (set it up in Accounting setup)';
    const target = destination.value === 'BANK' ? (bank.value ? bank.options[bank.selectedIndex].text : 'Bank account (choose one)') : held;
    const line = (account, dr, cr) => { const tr = document.createElement('tr'); tr.append(cell('td', account), cell('td', dr, 'num'), cell('td', cr, 'num')); return tr; };
    const group = text => { const tr = document.createElement('tr'); tr.className = 'entries-group'; const th = cell('th', text); th.colSpan = 3; tr.append(th); return tr; };
    const feeRows = fees.map(([account, n]) => n < 0n ? line(account, grouped(-n), '') : line(account, '', grouped(n)));
    const arRows = [...credits].map(([account, n]) => line(account, '', grouped(n)));
    if (!(gross > 0n || credits.size)) entries.replaceChildren(line('Enter an amount and choose invoices to see the entries.', '', ''));
    else if (!fees.length) entries.replaceChildren(line(target, grouped(gross), ''), ...arRows);
    // Review 3b M7: the payment is held in Undeposited Funds, then deposited net of the fees, in one step.
    else entries.replaceChildren(
      group('1. The payment, held in Undeposited Funds'), line(held, grouped(gross), ''), ...arRows,
      group(`2. The deposit to ${target}, net of the fees and adjustments`), line(target, grouped(net), ''), ...feeRows, line(held, '', grouped(gross)));
  };
  const feeBox = form.querySelector('[data-fees]');
  const signed = text => { const t = String(text || '').trim(); const n = cents(t.replace(/^-\s*/, '')); return n === null ? null : t.startsWith('-') ? -n : n; };
  const feeLines = () => feeBox ? [...feeBox.querySelectorAll('[data-fee-line]')].map(row => {
    const select = row.querySelector('select'), n = signed(row.querySelector('input').value);
    return select.value && n ? [select.options[select.selectedIndex].text, n] : null;
  }).filter(Boolean) : [];
  // The fee lines follow Deposit to. Only as first drawn, a box the server showed with its refusal stays shown,
  // so the person sees why the payment was not saved.
  let destinationChanged = false;
  const toggleBank = () => {
    const on = destination.value === 'BANK'; bankField.hidden = !on; bank.disabled = !on;
    if (!feeBox) return;
    const shown = on || (!destinationChanged && !!feeBox.querySelector('#fee-errors'));
    feeBox.hidden = !shown; feeBox.disabled = !shown;
  };
  customer.addEventListener('change', () => { edited = true; showCustomer(); refresh(); });
  amount.addEventListener('input', () => { edited = true; typed = amount.value.trim() !== ''; if (typed) spread(); else rows().forEach(row => { pay(row).checked = false; setApplied(row, ''); }); refresh(); });
  body.addEventListener('change', event => {
    const row = event.target.closest('tr'); if (!row || event.target.type !== 'checkbox') return;
    edited = true;
    choose(row, event.target.checked);
    if (event.target.checked) {
      const left = (cents(amount.value) ?? 0n) - (appliedTotal() - (cents(apply(row).value) ?? 0n));
      setApplied(row, money(typed && left > 0n && left < balance(row) ? left : balance(row)));
    } else setApplied(row, '');
    if (!typed) amount.value = appliedTotal() > 0n ? money(appliedTotal()) : '';
    refresh();
  });
  body.addEventListener('input', event => {
    const row = event.target.closest('tr'); if (!row || event.target.type === 'checkbox') return;
    edited = true;
    clearError(row);
    pay(row).checked = event.target.value.trim() !== '';
    choose(row, pay(row).checked);
    if (!typed) amount.value = appliedTotal() > 0n ? money(appliedTotal()) : '';
    refresh();
  });
  destination.addEventListener('change', () => { destinationChanged = true; toggleBank(); refresh(); });
  feeBox?.addEventListener('input', refresh);
  feeBox?.addEventListener('change', refresh);
  feeBox?.addEventListener('click', () => window.setTimeout(refresh, 0));
  bank.addEventListener('change', refresh);
  toggleBank(); refresh();
})();

'use strict';
// Lines, totals and the accounting-entries panel for the bill and Write check pages
// (form#purchase-entry, data-kind "bill" or "check"). Amounts use exact cents (entries.js). On
// "Save and post"/"Save and record" the entries shown are sent as `shown_entries`; the server
// posts only when its own review produces the same entries.
(() => {
  const form = document.getElementById('purchase-entry');
  const E = window.FBEntries;
  if (!form || !E) return;
  const kind = form.dataset.kind;
  const accountField = form.dataset.accountField;
  const table = document.getElementById('purchase-lines');
  const body = table.querySelector('tbody');
  const template = document.getElementById('empty-line');
  const count = document.getElementById('id_lines-TOTAL_FORMS');
  const toggle = document.getElementById('toggle-items');
  const read = (id) => { const n = document.getElementById(id); return n ? JSON.parse(n.textContent) : null; };
  const posting = read('purchase-posting') || {};
  const field = (row, name) => row.querySelector(`[name$="-${name}"]`);
  const rows = () => [...body.querySelectorAll('tr.purchase-line')].filter((r) => !r.hidden);
  const optionLabel = (select) => {
    if (!select) return '';
    const o = select.options[select.selectedIndex];
    return o && o.value ? o.textContent.trim() : '';
  };

  // Check lines: an item makes an Items line (quantity × cost); otherwise quantity is one.
  const itemTab = (row) => {
    if (kind !== 'check') return;
    const item = field(row, 'item'), tab = field(row, 'tab'), qty = field(row, 'quantity');
    if (!tab) return;
    tab.value = item && item.value ? 'ITEM' : 'EXPENSE';
    if (qty) {
      qty.readOnly = tab.value !== 'ITEM';
      if (tab.value !== 'ITEM') qty.value = '1';
    }
  };
  // The server also accepts a leading "+" and extra trailing zeros ("10.000"); drop them so the
  // exact-cents arithmetic sees the same number.
  const plain = (text, places) => {
    let t = String(text).trim().replace(/,/g, '').replace(/^\+/, '');
    if (t.includes('.')) while (t.length - t.indexOf('.') - 1 > places && t.endsWith('0')) t = t.slice(0, -1);
    return t.endsWith('.') ? t.slice(0, -1) || '0' : t;
  };
  const quantityOf = (row) => {
    const q = field(row, 'quantity');
    const text = q ? q.value.trim() : '';
    return text === '' ? '1' : plain(text, 4);
  };
  const amountOf = (row) => {
    const rate = field(row, 'rate');
    if (!rate || rate.value.trim() === '') return 0n;
    return E.lineAmount(quantityOf(row), plain(rate.value, 2));
  };
  const used = (row) => ['description', 'rate', 'item'].some((n) => { const f = field(row, n); return f && f.value.trim() !== ''; })
    || (field(row, accountField) && field(row, accountField).value !== '');

  const totals = () => {
    let total = 0n, valid = true;
    const debits = new Map();
    let missingAccount = false;
    for (const row of rows()) {
      const amount = amountOf(row);
      const out = row.querySelector('[data-line-amount]');
      if (amount === null) { valid = false; if (out) out.textContent = '—'; continue; }
      if (out) out.textContent = used(row) ? E.display(amount) : '—';
      if (!used(row)) continue;
      total += amount;
      const account = field(row, accountField);
      if (!account || !account.value) { missingAccount = true; continue; }
      const entry = debits.get(account.value) || { label: optionLabel(account), cents: 0n };
      entry.cents += amount;
      debits.set(account.value, entry);
    }
    for (const node of document.querySelectorAll('[data-total]')) node.textContent = valid ? E.display(total) : '—';
    // Entries in the shape the server reviews them (payables/checks posting).
    let credit = null, note = '';
    if (kind === 'bill') {
      if (posting.payables) credit = { account: posting.payables, label: posting.payables_label };
      else note = 'Set a payables account in Accounting setup before posting bills.';
    } else {
      const bank = form.querySelector('[name="bank"]');
      if (bank && bank.value) credit = { account: bank.value, label: optionLabel(bank) };
      else note = 'Choose the bank account this is paid from.';
    }
    if (!valid) note = 'Fix the amounts marked to see the entries.';
    else if (missingAccount) note = 'Choose an account on every line to see the entries.';
    else if (total === 0n) note = 'Enter an amount to see the entries.';
    const entries = [];
    if (!note && credit) {
      entries.push({ account: credit.account, label: credit.label, debit: 0n, credit: total });
      for (const [account, d] of debits) {
        if (kind === 'bill' && d.cents <= 0n) continue;
        entries.push({ account, label: d.label, debit: d.cents, credit: 0n });
      }
    }
    const tbody = document.querySelector('[data-entries]');
    if (tbody) {
      tbody.replaceChildren(...entries.map((e) => {
        const tr = document.createElement('tr');
        for (const [text, cls] of [[e.label, ''], [e.debit ? E.display(e.debit) : '', 'num'], [e.credit ? E.display(e.credit) : '', 'num']]) {
          const td = document.createElement('td');
          td.textContent = text;
          if (cls) td.className = cls;
          tr.append(td);
        }
        return tr;
      }));
    }
    const noteNode = document.querySelector('[data-entries-note]');
    if (noteNode) noteNode.textContent = note;
    // Kept current on every change: the desktop shell sends the form from a capture-phase
    // submit listener, before any listener here could fill it in.
    form.querySelector('[name="shown_entries"]').value = E.signature(entries);
    return entries;
  };

  const showItems = (on) => {
    table.classList.toggle('show-items', on);
    toggle.setAttribute('aria-pressed', String(on));
    toggle.textContent = on ? 'Hide item, quantity and rate' : 'Show item, quantity and rate';
    for (const label of table.querySelectorAll('[data-rate-label]')) label.textContent = on ? 'Rate' : 'Amount';
  };
  // The compact view hides Item and Qty, so it is only offered while no line uses them.
  const needsItems = () => rows().some((row) => {
    const item = field(row, 'item');
    return (item && item.value) || !/^(1(\.0*)?|)$/.test(quantityOf(row).trim());
  });
  toggle.addEventListener('click', () => {
    const on = !table.classList.contains('show-items');
    if (!on && needsItems()) {
      toggle.textContent = 'Lines use items or quantities';
      setTimeout(() => showItems(true), 1600);
      return;
    }
    showItems(on);
  });

  const prepare = (row, index) => {
    const position = index + 1;
    const remove = row.querySelector('[data-remove-line]');
    if (remove) remove.setAttribute('aria-label', `Remove line ${position}`);
    if (window.FBAccountPicker) window.FBAccountPicker.enhanceAll(row);
    itemTab(row);
    if (!table.classList.contains('show-items')) for (const l of row.querySelectorAll('[data-rate-label]')) l.textContent = 'Amount';
    else for (const l of row.querySelectorAll('[data-rate-label]')) l.textContent = 'Rate';
  };
  const addLine = (focus) => {
    const index = Number(count.value);
    if (index >= 50) return null;
    const holder = document.createElement('tbody');
    holder.innerHTML = template.innerHTML.replaceAll('__prefix__', String(index));
    const row = holder.querySelector('tr');
    body.append(row);
    count.value = String(index + 1);
    if (kind === 'check') { const tab = field(row, 'tab'); if (tab) tab.value = 'EXPENSE'; }
    prepare(row, rows().length - 1);
    if (focus) {
      const target = row.querySelector('.picker-input') || row.querySelector('select,input:not([type=hidden]),textarea');
      if (target) target.focus();
    }
    return row;
  };
  // A new blank line appears once the last line is used.
  const keepBlankLast = () => {
    const visible = rows();
    if (!visible.length || used(visible[visible.length - 1])) addLine(false);
  };

  body.addEventListener('click', (event) => {
    const button = event.target.closest('[data-remove-line]');
    if (!button) return;
    const row = button.closest('tr');
    const del = field(row, 'DELETE');
    if (del) del.checked = true;
    row.hidden = true;
    const next = rows()[0];
    keepBlankLast();
    totals();
    const focus = (rows().find((r) => r.compareDocumentPosition(row) & Node.DOCUMENT_POSITION_PRECEDING) || next || rows()[0]);
    if (focus) { const t = focus.querySelector('.picker-input,select,input:not([type=hidden])'); if (t) t.focus(); }
  });
  form.addEventListener('input', () => { totals(); keepBlankLast(); });
  form.addEventListener('change', (event) => {
    const row = event.target.closest && event.target.closest('tr.purchase-line');
    if (row && (event.target.name || '').endsWith('-item')) itemTab(row);
    totals();
    keepBlankLast();
  });
  document.getElementById('add-line').addEventListener('click', () => addLine(true));
  // Alt+Enter adds a line, as on the invoice editor.
  form.addEventListener('keydown', (event) => {
    if (event.altKey && event.key === 'Enter') { event.preventDefault(); addLine(true); }
  });
  // Submit-once is the desktop shell's (one bridged submit per form at a time); the server's
  // request key makes a repeated "Save and post" open the posted bill instead of posting again.

  rows().forEach(prepare);
  showItems(form.dataset.itemsOpen === '1' || needsItems());
  keepBlankLast();
  totals();
})();

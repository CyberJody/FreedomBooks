'use strict';
// Pay bills: tick bills, see one payment per vendor with its check number and entries, and send
// the exact proposal shown (shown-<vendor>) so the server posts only what this page displayed.
(() => {
  const form = document.getElementById('pay-bills');
  const E = window.FBEntries;
  if (!form || !E || !document.getElementById('pay-proposal')) return; // nothing to pay
  const data = JSON.parse((document.getElementById('pay-data') || {}).textContent || '{}');
  const next = data.next_numbers || {};
  const rows = [...form.querySelectorAll('tbody tr[data-bill]')];
  const bank = form.querySelector('#id_bank');
  const method = form.querySelector('#id_method');
  const reference = form.querySelector('#id_reference');
  const referenceLabel = document.getElementById('reference-label');
  const proposal = document.getElementById('pay-proposal');
  const entriesBox = document.getElementById('pay-entries');
  const shownBox = document.getElementById('pay-shown');
  const total = document.getElementById('pay-total');
  const submit = document.getElementById('pay-submit');
  const all = document.getElementById('pay-all');
  const note = document.getElementById('filter-note');
  const filterForm = document.getElementById('pay-filter');
  const filterVendor = document.getElementById('filter-vendor');
  const filterDue = document.getElementById('filter-due');
  const box = (row) => row.querySelector('input[name="pay"]');
  const amount = (row) => row.querySelector('input[name^="amount-"]');
  // The server orders vendors by UTF-8 bytes, which is code point order (not UTF-16 units).
  const byCodePoint = (a, b) => {
    const x = Array.from(a, (ch) => ch.codePointAt(0)), y = Array.from(b, (ch) => ch.codePointAt(0));
    for (let i = 0; i < Math.min(x.length, y.length); i++) if (x[i] !== y[i]) return x[i] < y[i] ? -1 : 1;
    return x.length - y.length;
  };
  const el = (tag, text, cls) => { const n = document.createElement(tag); if (text != null) n.textContent = text; if (cls) n.className = cls; return n; };

  // Check numbers: the first vendor takes the number entered, each next vendor the next one.
  const references = (count) => {
    const r = reference.value.trim();
    if (method.value !== 'CHECK' || count <= 1 || r === '') return { refs: Array(count).fill(r) };
    if (!/^\d{1,18}$/.test(r)) return { error: 'Enter the first check number as digits only; each vendor’s check takes the next number.' };
    const start = BigInt(r);
    return { refs: Array.from({ length: count }, (_, i) => (start + BigInt(i)).toString().padStart(r.length, '0')) };
  };

  const recompute = () => {
    const ticked = rows.filter((row) => box(row).checked);
    const groups = new Map();
    let bad = 0;
    for (const row of ticked) {
      const raw = amount(row).value;
      const c = raw.trim() === '' ? E.cents(row.dataset.open) : E.cents(raw.replace(/[$\s]/g, ''));
      const open = E.cents(row.dataset.open);
      const invalid = c === null || c <= 0n || c > open;
      amount(row).classList.toggle('invalid', invalid);
      if (invalid) { bad += 1; continue; }
      const v = row.dataset.vendor;
      if (!groups.has(v)) groups.set(v, { vendor: v, label: row.dataset.vendorLabel, bills: [] });
      groups.get(v).bills.push({ row, cents: c });
    }
    const list = [...groups.values()].sort((a, b) => byCodePoint(a.label, b.label) || byCodePoint(a.vendor, b.vendor));
    const refs = references(list.length);
    shownBox.replaceChildren();
    proposal.replaceChildren();
    entriesBox.replaceChildren();
    let grand = 0n;
    const bankLabel = bank && bank.selectedOptions[0] ? bank.selectedOptions[0].textContent : '';
    if (list.length) {
      const table = el('table', null, 'pay-proposal');
      const head = el('thead'); const hr = el('tr');
      for (const [t, cls] of [['Vendor'], ['Bills'], [method.value === 'CHECK' ? 'Check no.' : 'Reference'], ['Amount', 'num']]) { const th = el('th', t, cls); th.scope = 'col'; hr.append(th); }
      head.append(hr); table.append(head);
      const body = el('tbody');
      list.forEach((g, i) => {
        const sum = g.bills.reduce((s, b) => s + b.cents, 0n);
        grand += sum;
        const tr = el('tr');
        tr.append(el('td', g.label), el('td', g.bills.map((b) => b.row.dataset.number).join(', ')), el('td', refs.refs ? refs.refs[i] || '—' : '—'), el('td', E.display(sum), 'num'));
        body.append(tr);
        // The entries this payment posts: the bank is credited, each bill's payables account debited.
        const debits = new Map();
        for (const b of g.bills) debits.set(b.row.dataset.ap, (debits.get(b.row.dataset.ap) || { label: b.row.dataset.apLabel, cents: 0n }));
        for (const b of g.bills) debits.get(b.row.dataset.ap).cents += b.cents;
        const entries = [{ account: bank ? bank.value : '', debit: 0n, credit: sum }];
        for (const [account, d] of debits) entries.push({ account, debit: d.cents, credit: 0n });
        if (refs.refs && bank && bank.value) {
          const input = el('input'); input.type = 'hidden'; input.name = `shown-${g.vendor}`;
          input.value = `${refs.refs[i]}||${E.signature(entries)}`;
          shownBox.append(input);
        }
        const et = el('table', null, 'pay-entry-table');
        const cap = el('caption', `${g.label}${refs.refs && refs.refs[i] ? ` · ${method.value === 'CHECK' ? 'check' : 'reference'} ${refs.refs[i]}` : ''}`);
        const eh = el('tr'); for (const [t, cls] of [['Account'], ['Debit', 'num'], ['Credit', 'num']]) { const th = el('th', t, cls); th.scope = 'col'; eh.append(th); }
        const eb = el('tbody');
        for (const [, d] of debits) { const r = el('tr'); r.append(el('td', d.label), el('td', E.display(d.cents), 'num'), el('td', '', 'num')); eb.append(r); }
        const br = el('tr'); br.append(el('td', bankLabel || 'Bank account'), el('td', '', 'num'), el('td', E.display(sum), 'num')); eb.append(br);
        const ehead = el('thead'); ehead.append(eh);
        et.append(cap, ehead, eb);
        entriesBox.append(et);
      });
      table.append(body);
      const foot = el('tfoot'); const fr = el('tr');
      const ft = el('th', 'Total'); ft.scope = 'row'; ft.colSpan = 3;
      fr.append(ft, el('td', E.display(grand), 'num')); foot.append(fr); table.append(foot);
      proposal.append(table);
    } else {
      proposal.append(el('p', 'Tick bills to see the payments.'));
      entriesBox.append(el('p', 'Tick bills to see the entries.'));
    }
    if (refs.error) proposal.append(el('p', refs.error, 'field-error'));
    if (bad) proposal.append(el('p', `Check the amount on ${bad} ticked bill${bad === 1 ? '' : 's'}: it must be more than zero and no more than the open balance.`, 'field-error'));
    const count = ticked.length;
    total.textContent = count ? `${count} bill${count === 1 ? '' : 's'} · ${list.length} payment${list.length === 1 ? '' : 's'} · Total ${E.display(grand)}` : 'No bills ticked';
    // Ticked bills the filter hides still count; say so.
    const hidden = ticked.filter((row) => row.hidden).length;
    note.hidden = hidden === 0;
    note.textContent = hidden ? `${hidden} ticked bill${hidden === 1 ? ' is' : 's are'} hidden by the filter and will be paid too.` : '';
    const visible = rows.filter((row) => !row.hidden);
    if (all) { all.checked = visible.length > 0 && visible.every((row) => box(row).checked); all.indeterminate = !all.checked && visible.some((row) => box(row).checked); }
  };

  const setTicked = (row, on) => {
    box(row).checked = on;
    row.classList.toggle('ticked', on);
    if (on && amount(row).value.trim() === '') amount(row).value = row.dataset.open;
    if (!on) { amount(row).value = ''; amount(row).classList.remove('invalid'); }
  };
  for (const row of rows) {
    box(row).addEventListener('change', () => { setTicked(row, box(row).checked); recompute(); });
    amount(row).addEventListener('input', () => {
      if (amount(row).value.trim() !== '' && !box(row).checked) { box(row).checked = true; row.classList.add('ticked'); }
      recompute();
    });
  }
  if (all) all.addEventListener('change', () => { for (const row of rows) if (!row.hidden) setTicked(row, all.checked); recompute(); });

  // Filters work in place, so ticks are kept.
  const applyFilter = () => {
    const v = filterVendor ? filterVendor.value : '';
    const due = filterDue ? filterDue.value : '';
    for (const row of rows) row.hidden = !((v === '' || row.dataset.vendor === v) && (due === '' || row.dataset.due <= due));
    recompute();
  };
  if (filterForm) {
    filterForm.addEventListener('submit', (e) => { e.preventDefault(); applyFilter(); });
    filterVendor.addEventListener('change', applyFilter);
    filterDue.addEventListener('change', applyFilter);
    document.getElementById('filter-apply').hidden = true;
  }

  // By check, the number starts as the bank's next check number (editable).
  if (reference.value !== '' && bank && reference.value === next[bank.value]) reference.dataset.auto = '1';
  const refill = () => {
    referenceLabel.textContent = method.value === 'CHECK' ? 'First check no.' : 'Reference';
    if (reference.value !== '' && reference.dataset.auto !== '1') return;
    reference.value = method.value === 'CHECK' && bank && next[bank.value] !== undefined ? next[bank.value] : '';
    reference.dataset.auto = '1';
  };
  if (bank) bank.addEventListener('change', () => { refill(); recompute(); });
  method.addEventListener('change', () => { refill(); recompute(); });
  reference.addEventListener('input', () => { reference.dataset.auto = '0'; recompute(); });

  // Send once: a second press while the first is on its way does nothing.
  form.addEventListener('submit', (e) => {
    if (form.dataset.sending === '1') { e.preventDefault(); return; }
    recompute();
    form.dataset.sending = '1';
    submit.setAttribute('aria-disabled', 'true');
    submit.textContent = 'Paying…';
  });
  window.addEventListener('pageshow', () => { form.dataset.sending = '0'; submit.removeAttribute('aria-disabled'); submit.textContent = 'Pay selected bills'; });
  applyFilter();
})();

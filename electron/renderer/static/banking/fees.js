'use strict';
// Worker item 47: "Fees and adjustments" on Record deposit and Receive payment. Lines can be added and
// removed, and the total is shown live: what the payments come to, the fees and adjustments (a fee is
// negative), and what reaches the bank. Exact cents (BigInt); the server checks and posts.
// Worker review 3b: a comma is refused here as the save refuses it (M7), so the live total never shows an
// amount the save will not take. A positive line on an expense account (`data-expense`), usually a fee typed
// without its minus sign, is said beside the total (M1); the server's own warning at the lines, and the
// confirmation it carries (`fee_ok`), go once a line is changed, so a changed line is checked again.
(() => {
  const AMOUNT = /^-?\d+(\.\d{1,2})?$/;
  const toCents = text => {
    const t = String(text || '').trim().replace(/^-\s+/, '-');
    if (!AMOUNT.test(t)) return null;
    const [whole, frac = ''] = t.replace('-', '').split('.');
    const n = BigInt(whole) * 100n + BigInt(frac.padEnd(2, '0'));
    return t.startsWith('-') ? -n : n;
  };
  const money = c => { const a = c < 0n ? -c : c; return `${c < 0n ? '-' : ''}${(a / 100n).toLocaleString('en-US')}.${String(a % 100n).padStart(2, '0')}`; };
  if (typeof document === 'undefined') { module.exports = { toCents, money }; return; }
  for (const box of document.querySelectorAll('[data-fees]')) {
    const body = box.querySelector('[data-fee-rows]');
    const out = box.querySelector('[data-fee-total]');
    const lines = () => [...body.querySelectorAll('[data-fee-line]')];
    const base = () => {
      if (box.dataset.baseFrom === 'receipts') return [...document.querySelectorAll('input[name=receipts]:checked')].reduce((n, b) => n + (toCents(b.dataset.amount) ?? 0n), 0n);
      if (box.dataset.baseFrom === 'amount') return toCents(document.getElementById('id_amount')?.value);
      return null;
    };
    const update = () => {
      let fees = 0n, bad = false, any = false;
      const positive = [];
      for (const line of lines()) {
        const text = line.querySelector('[name=fee_amount]').value.trim();
        if (!text) continue;
        any = true;
        const n = toCents(text);
        if (n === null || n === 0n) { bad = true; continue; }
        fees += n;
        const option = line.querySelector('select').selectedOptions[0];
        if (n > 0n && option?.hasAttribute('data-expense')) positive.push(`${money(n)} on ${option.textContent}`);
      }
      const received = base();
      out.classList.toggle('attention', bad);
      if (bad) { out.textContent = 'Enter each fee or adjustment as an amount such as -24.95.'; return; }
      const words = [];
      if (received !== null && any) words.push(`${box.dataset.baseLabel} ${money(received)} · fees and adjustments ${money(fees)} · ${box.dataset.totalLabel} ${money(received + fees)}`);
      else if (box.dataset.baseFrom === 'receipts') words.push(`${box.dataset.totalLabel} ${money(received)}`);
      if (positive.length === 1) words.push(`${positive[0]} is positive, so it adds to the deposit and credits that expense; a fee is negative, such as -24.95.`);
      else if (positive.length) words.push(`${positive.slice(0, -1).join(', ')} and ${positive.at(-1)} are positive, so they add to the deposit and credit those expenses; a fee is negative, such as -24.95.`);
      out.textContent = words.join('. ');
      out.classList.toggle('attention', (received !== null && any && received + fees <= 0n) || positive.length > 0);
    };
    const changed = () => { box.querySelector('[data-fee-warning]')?.remove(); box.querySelector('[data-fee-ok]')?.remove(); };
    box.addEventListener('input', changed);
    box.addEventListener('change', changed);
    const renumber = () => lines().forEach((line, i) => {
      line.querySelector('select').setAttribute('aria-label', `Account, fee or adjustment line ${i + 1}`);
      line.querySelector('input').setAttribute('aria-label', `Amount, fee or adjustment line ${i + 1}`);
      line.querySelector('[data-fee-remove]').setAttribute('aria-label', `Remove fee or adjustment line ${i + 1}`);
    });
    box.addEventListener('click', ev => {
      if (ev.target.closest('[data-fee-add]')) {
        const line = lines().at(-1).cloneNode(true);
        line.querySelector('select').value = box.dataset.default || '';
        line.querySelector('input').value = '';
        body.append(line);
        renumber();
        line.querySelector('select').focus();
      } else if (ev.target.closest('[data-fee-remove]')) {
        const line = ev.target.closest('[data-fee-line]');
        if (lines().length > 1) line.remove(); else line.querySelector('input').value = '';
        changed();
        renumber();
        update();
        box.querySelector('[data-fee-add]').focus();
      }
    });
    document.addEventListener('input', update);
    document.addEventListener('change', update);
    update();
  }
})();

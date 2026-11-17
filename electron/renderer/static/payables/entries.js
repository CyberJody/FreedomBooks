'use strict';
// Shared by the bill, check and Pay bills pages: exact cents arithmetic and the canonical
// entries string the server compares before a one-step "Save and post" (d6.rs entries_signature).
(() => {
  // Parse "1,234.5" into BigInt cents; null when it is not a plain non-negative amount.
  const cents = (text) => {
    const t = String(text == null ? '' : text).trim().replace(/,/g, '');
    if (!/^\d*(\.\d{0,2})?$/.test(t) || t === '' || t === '.') return null;
    const [w, f = ''] = t.split('.');
    return BigInt(w || '0') * 100n + BigInt((f + '00').slice(0, 2));
  };
  // Quantity with up to 4 places as BigInt ten-thousandths; null when invalid.
  const tenThousandths = (text) => {
    const t = String(text == null ? '' : text).trim().replace(/,/g, '');
    if (!/^\d*(\.\d{0,4})?$/.test(t) || t === '' || t === '.') return null;
    const [w, f = ''] = t.split('.');
    return BigInt(w || '0') * 10000n + BigInt((f + '0000').slice(0, 4));
  };
  // calculations::line_amount: (quantity(4dp) * rate(2dp) + 5000) / 10000, half up on cents.
  const lineAmount = (quantity, rate) => {
    const q = tenThousandths(quantity), r = cents(rate);
    if (q === null || r === null) return null;
    return (q * r + 5000n) / 10000n;
  };
  const text = (c) => {
    const neg = c < 0n, a = neg ? -c : c;
    return (neg ? '-' : '') + (a / 100n).toString() + '.' + (a % 100n).toString().padStart(2, '0');
  };
  const display = (c) => {
    const neg = c < 0n, a = neg ? -c : c;
    const whole = (a / 100n).toString().replace(/\B(?=(\d{3})+(?!\d))/g, ',');
    return (neg ? '-' : '') + whole + '.' + (a % 100n).toString().padStart(2, '0');
  };
  // entries: [{account, debit: BigInt, credit: BigInt}]
  const signature = (entries) => entries
    .map((e) => `${e.account}:${text(e.debit)}:${text(e.credit)}`)
    .sort((a, b) => (a < b ? -1 : a > b ? 1 : 0))
    .join('|');
  window.FBEntries = { cents, tenThousandths, lineAmount, text, display, signature };
})();

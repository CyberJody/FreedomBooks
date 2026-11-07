/* N18 Reconcile screen: live totals while ticking (progressive enhancement; the server recomputes every
   figure on Save / Reconcile now and refuses Reconcile now unless the difference is exactly 0.00).
   Money is handled as BigInt cents, never floating point. */
(function () {
  'use strict';
  var form = document.getElementById('reconcile-form');
  if (!form || typeof BigInt === 'undefined') { return; }
  function cents(text) {
    text = String(text || '0').trim();
    var negative = text.charAt(0) === '-';
    var parts = text.replace('-', '').split('.');
    var whole = BigInt(parts[0] || '0') * 100n + BigInt(((parts[1] || '') + '00').slice(0, 2));
    return negative ? -whole : whole;
  }
  function money(value) {
    var negative = value < 0n;
    if (negative) { value = -value; }
    var rest = String(value % 100n);
    return (negative ? '-' : '') + String(value / 100n) + '.' + (rest.length < 2 ? '0' + rest : rest);
  }
  function shown(value, negate) {
    if (negate && value !== 0n) { value = -value; }
    return money(value);
  }
  var sign = BigInt(form.getAttribute('data-sign') === '-1' ? -1 : 1);
  var beginning = cents(form.getAttribute('data-beginning'));
  var ending = cents(form.getAttribute('data-ending'));
  var boxes = Array.prototype.slice.call(form.querySelectorAll('input[name="tick"]'));
  var button = document.getElementById('reconcile-now');
  var hint = document.getElementById('reconcile-hint');
  var blocked = button.disabled && document.querySelector('.notice.warning[role="alert"]');
  function set(id, text) { var node = document.getElementById(id); if (node) { node.textContent = text; } }
  function update() {
    var debit = 0n, credit = 0n, nDebit = 0, nCredit = 0;
    boxes.forEach(function (box) {
      var row = box.closest('tr');
      if (row) { row.classList.toggle('is-cleared', box.checked); }
      if (!box.checked) { return; }
      var amount = BigInt(box.getAttribute('data-cents'));
      if (box.getAttribute('data-side') === 'debit') { debit += amount; nDebit += 1; } else { credit += amount; nCredit += 1; }
    });
    var cleared = beginning + sign * (debit - credit);
    var difference = ending - cleared;
    set('cleared-debit', shown(debit, sign !== 1n)); set('cleared-credit', shown(credit, sign === 1n));
    set('count-debit', String(nDebit)); set('count-credit', String(nCredit));
    set('cleared-balance', money(cleared)); set('difference', money(difference));
    var box = document.getElementById('difference-box');
    if (box) { box.classList.toggle('zero', difference === 0n); box.classList.toggle('off', difference !== 0n); }
    if (!blocked) {
      button.disabled = difference !== 0n;
      if (hint) {
        hint.textContent = difference === 0n ? 'The difference is 0.00: reconcile now to lock these lines.'
          : 'Reconcile now is available when the difference is 0.00. Leave keeps your ticks.';
      }
    }
  }
  boxes.forEach(function (box) { box.addEventListener('change', update); });
  var markAll = form.querySelector('.rec-mark-all');
  if (markAll && boxes.length) {
    markAll.hidden = false;
    markAll.addEventListener('click', function (event) {
      var target = event.target.closest('button[data-mark]');
      if (!target) { return; }
      var on = target.getAttribute('data-mark') === 'all';
      boxes.forEach(function (box) { box.checked = on; });
      update();
    });
  }
  update();
})();

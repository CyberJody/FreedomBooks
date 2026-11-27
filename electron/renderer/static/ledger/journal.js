// General journal entry totals and line editor.
(function () {
  function cents(value) {
    var text = String(value || '').trim();
    if (!/^\d{0,14}(\.\d*)?$/.test(text) || text === '' || text === '.') return text === '' ? 0n : null;
    var parts = text.split('.');
    var frac = (parts[1] || '') + '000';
    var hundredths = BigInt(parts[0] || '0') * 100n + BigInt(frac.slice(0, 2));
    if (parseInt(frac.charAt(2), 10) >= 5) hundredths += 1n;      // half-up, as the server rounds
    return hundredths;
  }
  function format(hundredths) {
    var sign = hundredths < 0 ? '-' : '';
    var abs = hundredths < 0n ? -hundredths : hundredths;
    var whole = (abs / 100n).toLocaleString('en-US'), part = String(abs % 100n);
    return sign + whole + '.' + (part.length < 2 ? '0' + part : part);
  }
  function refresh(form) {
    var debit = 0n, credit = 0n, invalid = false;
    form.querySelectorAll('tbody tr.journal-line').forEach(function (row) {
      var remove = row.querySelector('input[name$="-DELETE"]');
      if (remove && remove.checked) return;
      var d = cents(row.querySelector('input[name$="-debit"]').value);
      var c = cents(row.querySelector('input[name$="-credit"]').value);
      if (d === null || c === null) { invalid = true; return; }
      debit += d; credit += c;
    });
    var out = document.getElementById('total-difference');
    var diff = document.getElementById('total-diff-value');
    document.getElementById('total-debit').textContent = format(debit);
    document.getElementById('total-credit').textContent = format(credit);
    if (diff) diff.textContent = invalid ? '—' : format(debit - credit);
    if (invalid) { out.textContent = 'Some amounts are not valid numbers.'; out.className = 'unbalanced'; }
    else if (debit !== credit) { out.textContent = 'Out of balance by ' + format(debit - credit) + ' (posting needs equal debits and credits).'; out.className = 'unbalanced'; }
    else if (debit === 0n) { out.textContent = 'Enter amounts.'; out.className = ''; }
    else { out.textContent = 'Balanced.'; out.className = ''; }
  }
  function maxLines(form) {
    var n = parseInt(form.getAttribute('data-line-max'), 10);
    return n > 0 ? n : 50;
  }
  function showLimit(form, on) {
    var note = document.getElementById('line-limit');
    var button = document.getElementById('add-line');
    if (note) note.hidden = !on;
    if (button) {
      button.disabled = on;
      button.setAttribute('aria-disabled', on ? 'true' : 'false');
    }
  }
  function addLine(form) {
    var count = document.getElementById('id_lines-TOTAL_FORMS');
    var index = Number(count.value);
    if (index >= maxLines(form)) {
      showLimit(form, true);
      return;
    }
    var html = document.getElementById('empty-line').innerHTML.replaceAll('__prefix__', String(index));
    var holder = document.createElement('tbody');
    holder.innerHTML = html.trim();
    var row = holder.querySelector('tr');
    form.querySelector('table.journal-lines tbody').appendChild(row);
    count.value = String(index + 1);
    row.querySelectorAll('label.sr-only').forEach(function (label) {
      label.textContent = label.textContent.replace('Additional line', 'Line ' + (index + 1));
    });
    window.FBAccountPicker.enhanceAll(row);
    var account = row.querySelector('.picker-input');
    if (account) {
      account.focus({ preventScroll: true });
      row.scrollIntoView({ block: 'nearest' });
    }
    if (index + 1 >= maxLines(form)) showLimit(form, true);
    refresh(form);
  }
  function clearUnused(form) {
    form.querySelectorAll('tbody tr.journal-line').forEach(function (row) {
      var account = row.querySelector('[name$="-account"]');
      var debit = row.querySelector('[name$="-debit"]');
      var credit = row.querySelector('[name$="-credit"]');
      var used = (account && account.value) || (debit && debit.value.trim()) || (credit && credit.value.trim());
      if (used) return;
      var memo = row.querySelector('[name$="-description"]');
      var name = row.querySelector('[name$="-name"]');
      if (memo) memo.value = '';
      if (name) name.value = '';
    });
  }
  document.addEventListener('DOMContentLoaded', function () {
    var form = document.querySelector('form.journal-entry');
    if (!form) return;
    form.addEventListener('input', function () { refresh(form); });
    form.addEventListener('change', function () { refresh(form); });
    var button = document.getElementById('add-line');
    if (button) button.addEventListener('click', function () { addLine(form); });
    form.addEventListener('submit', function () { clearUnused(form); });
    var count = document.getElementById('id_lines-TOTAL_FORMS');
    if (count && Number(count.value) >= maxLines(form)) showLimit(form, true);
    refresh(form);
  });
})();

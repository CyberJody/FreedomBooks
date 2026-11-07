'use strict';
document.getElementById('add-allocation').addEventListener('click', () => {
  const count = document.getElementById('id_allocations-TOTAL_FORMS');
  const index = Number(count.value);
  if (index >= 50) return;
  document.getElementById('allocation-rows').insertAdjacentHTML('beforeend', document.getElementById('allocation-empty').innerHTML.replaceAll('__prefix__', String(index)));
  count.value = String(index + 1);
  document.getElementById(`id_allocations-${index}-bill`).focus();
});

// FX4: by check, the reference starts as the bank's next check number (a hint; editable).
(() => {
  const node = document.getElementById('check-next-numbers');
  const form = document.getElementById('payment-entry');
  if (!node || !form) return;
  const next = JSON.parse(node.textContent);
  const bank = form.querySelector('[name="bank"]'), method = form.querySelector('[name="method"]'), reference = form.querySelector('[name="reference"]');
  if (!bank || !method || !reference) return;
  if (reference.value !== '' && reference.value === next[bank.value]) reference.dataset.auto = '1';
  const refill = () => {
    const auto = reference.value === '' || reference.dataset.auto === '1';
    if (!auto) return;
    if (method.value === 'CHECK' && next[bank.value] !== undefined) { reference.value = next[bank.value]; reference.dataset.auto = '1'; }
    else { reference.value = ''; reference.dataset.auto = '1'; }
  };
  bank.addEventListener('change', refill);
  method.addEventListener('change', refill);
  reference.addEventListener('input', () => { reference.dataset.auto = '0'; });
})();

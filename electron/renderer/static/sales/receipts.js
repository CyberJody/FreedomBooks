'use strict';
document.getElementById('add-allocation').addEventListener('click', () => {
  const count = document.getElementById('id_allocations-TOTAL_FORMS');
  const index = Number(count.value);
  if (index >= 50) return;
  document.getElementById('allocation-rows').insertAdjacentHTML('beforeend', document.getElementById('allocation-empty').innerHTML.replaceAll('__prefix__', String(index)));
  count.value = String(index + 1);
  document.getElementById(`id_allocations-${index}-invoice`).focus();
});

// A suggested check number follows the bank. Keep an edited number when switching methods.
(() => {
  const form = document.getElementById('refund-entry');
  const { bank, method, check_number: number } = form.elements;
  const next = JSON.parse(document.getElementById('refund-numbers').textContent);
  let manual = number.value !== (next[bank.value] || '');
  let saved = number.value;
  function refresh() {
    const check = method.value === 'CHECK';
    number.disabled = !check;
    number.value = check ? (manual ? saved : next[bank.value] || '') : '';
  }
  number.addEventListener('input', () => { manual = true; saved = number.value; });
  bank.addEventListener('change', refresh);
  method.addEventListener('change', refresh);
  if (method.value !== 'CHECK') {
    manual = false;
    refresh();
  }
})();

'use strict';
(() => {
  const source = document.getElementById('id_source_date')?.value;
  const frequency = document.getElementById('id_frequency');
  const start = document.getElementById('id_start_date');
  if (source && frequency && start) {
    const following = repeat => {
      const [year, month, day] = source.split('-').map(Number);
      const target = new Date(Date.UTC(year + (repeat === 'YEARLY' ? 1 : 0), month - 1 + (repeat === 'MONTHLY' ? 1 : 0), 1));
      const last = new Date(Date.UTC(target.getUTCFullYear(), target.getUTCMonth() + 1, 0)).getUTCDate();
      target.setUTCDate(Math.min(day, last));
      return target.toISOString().slice(0, 10);
    };
    let defaultDate = following(frequency.value);
    frequency.addEventListener('change', () => {
      const updated = following(frequency.value);
      if (start.value === defaultDate) start.value = updated;
      defaultDate = updated;
    });
  }
  const add = document.getElementById('recurring-add-line');
  if (!add) return;
  add.addEventListener('click', () => {
    const count = document.getElementById('recurring-line-count');
    const n = Number(count.value);
    if (!Number.isInteger(n) || n >= 50) return;
    const fragment = document.getElementById('recurring-empty-line').content.cloneNode(true);
    for (const el of fragment.querySelectorAll('[name], [id], [for]')) {
      for (const attr of ['name', 'id', 'for']) {
        if (el.hasAttribute(attr)) el.setAttribute(attr, el.getAttribute(attr).replaceAll('__prefix__', String(n)));
      }
    }
    document.getElementById('recurring-lines').append(fragment);
    count.value = String(n + 1);
    add.disabled = n + 1 >= 50;
  });
})();

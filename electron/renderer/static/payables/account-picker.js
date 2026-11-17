'use strict';
// Type-to-search account picker for the bill and Write check pages. It enhances a
// <select data-picker="account"> in place: the select stays in the form as the submitted field
// (moved out of sight), and a text box with a listbox lets the user find an account by any part
// of its code or name. ARIA 1.2 combobox pattern: arrows move, Enter or Tab chooses, Escape
// closes and restores, typing filters. No inline script or style (the app's CSP).
// The sales lane builds a similar picker for customers and items; the two can be unified.
(() => {
  let serial = 0;
  const words = (text) => text.toLowerCase().split(/[\s·,()/-]+/).filter(Boolean);
  const optionsOf = (select) => {
    const out = [];
    for (const node of select.children) {
      if (node.tagName === 'OPTGROUP') {
        for (const o of node.children) out.push({ value: o.value, label: o.textContent.trim(), group: node.label });
      } else if (node.tagName === 'OPTION') {
        out.push({ value: node.value, label: node.textContent.trim(), group: '' });
      }
    }
    return out;
  };
  const labelOf = (select) => {
    const o = select.options[select.selectedIndex];
    return o && o.value ? o.textContent.trim() : '';
  };

  function enhance(select) {
    if (select.dataset.pickerReady === '1') return;
    select.dataset.pickerReady = '1';
    const id = `picker-${++serial}`;
    const wrap = document.createElement('div');
    wrap.className = 'picker';
    const input = document.createElement('input');
    input.type = 'text';
    input.className = 'picker-input';
    input.id = `${select.id || id}-search`;
    input.autocomplete = 'off';
    input.spellcheck = false;
    input.setAttribute('role', 'combobox');
    input.setAttribute('aria-autocomplete', 'list');
    input.setAttribute('aria-expanded', 'false');
    input.setAttribute('aria-controls', `${id}-list`);
    input.placeholder = 'Type an account name or code';
    for (const a of ['aria-invalid', 'aria-describedby']) if (select.hasAttribute(a)) input.setAttribute(a, select.getAttribute(a));
    const list = document.createElement('ul');
    list.id = `${id}-list`;
    list.className = 'picker-list';
    list.setAttribute('role', 'listbox');
    list.hidden = true;
    // The label that named the select now names the text box.
    if (select.id) {
      for (const label of document.querySelectorAll(`label[for="${CSS.escape(select.id)}"]`)) {
        label.htmlFor = input.id;
        list.setAttribute('aria-label', label.textContent.trim().replace(/:$/, ''));
      }
    }
    select.parentNode.insertBefore(wrap, select);
    wrap.append(select, input, list);
    select.classList.add('picker-select');
    select.tabIndex = -1;
    select.setAttribute('aria-hidden', 'true');
    input.value = labelOf(select);

    let shown = [];
    let active = -1;
    const close = () => {
      list.hidden = true;
      input.setAttribute('aria-expanded', 'false');
      input.removeAttribute('aria-activedescendant');
      active = -1;
    };
    const mark = (index) => {
      active = index;
      list.querySelectorAll('[role=option]').forEach((li, i) => li.setAttribute('aria-selected', String(i === index)));
      const li = list.querySelector(`#${id}-o${index}`);
      if (li) { input.setAttribute('aria-activedescendant', li.id); li.scrollIntoView({ block: 'nearest' }); }
      else input.removeAttribute('aria-activedescendant');
    };
    const open = (query) => {
      const terms = words(query || '');
      shown = optionsOf(select).filter((o) => o.value !== '' && terms.every((t) => (o.label + ' ' + o.group).toLowerCase().includes(t)));
      list.replaceChildren();
      let group = null;
      shown.forEach((o, i) => {
        if (o.group !== group) {
          group = o.group;
          if (group) {
            const g = document.createElement('li');
            g.className = 'picker-group';
            g.setAttribute('role', 'presentation');
            g.textContent = group;
            list.append(g);
          }
        }
        const li = document.createElement('li');
        li.id = `${id}-o${i}`;
        li.setAttribute('role', 'option');
        li.setAttribute('aria-selected', 'false');
        li.dataset.index = String(i);
        li.textContent = o.label;
        list.append(li);
      });
      if (!shown.length) {
        const li = document.createElement('li');
        li.className = 'picker-empty';
        li.setAttribute('role', 'presentation');
        li.textContent = 'No account matches';
        list.append(li);
      }
      list.hidden = false;
      input.setAttribute('aria-expanded', 'true');
      const current = shown.findIndex((o) => o.value === select.value);
      mark(query ? (shown.length ? 0 : -1) : current);
    };
    const choose = (index) => {
      const o = shown[index];
      if (!o) return;
      const changed = select.value !== o.value;
      select.value = o.value;
      input.value = o.label;
      close();
      if (changed) select.dispatchEvent(new Event('change', { bubbles: true }));
    };
    input.addEventListener('focus', () => { input.select(); });
    input.addEventListener('input', () => {
      if (input.value.trim() === '') {
        if (select.value !== '') { select.value = ''; select.dispatchEvent(new Event('change', { bubbles: true })); }
        open('');
        return;
      }
      open(input.value);
    });
    input.addEventListener('keydown', (event) => {
      const isOpen = !list.hidden;
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault();
        if (!isOpen) { open(''); return; }
        if (!shown.length) return;
        const step = event.key === 'ArrowDown' ? 1 : -1;
        mark(active < 0 ? (step > 0 ? 0 : shown.length - 1) : (active + step + shown.length) % shown.length);
      } else if (event.key === 'Enter') {
        if (isOpen && active >= 0) { event.preventDefault(); choose(active); }
      } else if (event.key === 'Escape') {
        if (isOpen) { event.preventDefault(); input.value = labelOf(select); close(); }
      } else if (event.key === 'Tab') {
        if (isOpen && active >= 0 && input.value !== labelOf(select)) choose(active);
        else close();
      }
    });
    list.addEventListener('mousedown', (event) => {
      const li = event.target.closest('[role=option]');
      event.preventDefault();
      if (li) choose(Number(li.dataset.index));
    });
    input.addEventListener('blur', () => {
      close();
      input.value = labelOf(select);
    });
    // A script that sets the select (an item's default account, a vendor's default) dispatches
    // change; keep the text in step.
    select.addEventListener('change', () => { if (document.activeElement !== input) input.value = labelOf(select); });
  }

  const enhanceAll = (root) => {
    for (const select of (root || document).querySelectorAll('select[data-picker="account"]')) enhance(select);
  };
  window.FBAccountPicker = { enhance, enhanceAll, sync: (select) => select.dispatchEvent(new Event('change', { bubbles: true })) };
  enhanceAll(document);
})();

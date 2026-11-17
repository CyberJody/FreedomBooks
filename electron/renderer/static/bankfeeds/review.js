/* Review transactions: category search, row keys, the selection bar and in-place row actions.
   The server stays the authority. Every action is an ordinary post of this form through the preload's
   single-use, path-bound token (no page API); the reply's fragments replace the rows it acted on. Nothing
   here records anything or decides what is recorded. */
(() => {
  'use strict';
  const form = document.getElementById('bankfeed-review');
  if (!form) return;
  const table = form.querySelector('table.review');
  const bar = document.getElementById('review-bulk');
  const live = document.getElementById('review-live');
  const byId = id => document.getElementById(id);
  const rows = () => Array.from(form.querySelectorAll('tr[data-review-row]'));
  const field = (row, key) => row.querySelector(`[name="${key}_${row.dataset.reviewRow}"]`);
  const picks = () => Array.from(form.querySelectorAll('input[name="sel"]'));
  const say = text => { if (live) { live.textContent = ''; window.setTimeout(() => { live.textContent = text; }, 30); } };

  // Ask the server for an in-place reply (fragments plus the next token) instead of a whole page.
  const inline = document.createElement('input');
  inline.type = 'hidden'; inline.name = 'inline'; inline.value = '1';
  form.append(inline);

  let dirty = rows().some(row => (row.dataset.draftRetained || '').toLowerCase() === 'true');

  // --- category search (WAI-ARIA combobox with a list popup) ------------------------------------------
  const recent = (table?.dataset.recent || '').split(' ').filter(Boolean);
  const chosen = []; // categories picked on this page, newest first
  let comboCount = 0;
  const optionsOf = select => Array.from(select.options).filter(o => o.value !== '' || select.id === 'bulk-category');
  const labelOf = select => (select.value === '' && select.id !== 'bulk-category') ? '' : (select.selectedOptions[0]?.textContent || '');
  function rank(select, query) {
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    const all = optionsOf(select);
    const order = [...chosen, ...recent];
    const recentRank = o => { const i = order.indexOf(o.value); return i < 0 ? order.length : i; };
    if (!words.length) {
      return all.slice().sort((a, b) => recentRank(a) - recentRank(b));
    }
    const hits = all.filter(o => { const t = o.textContent.toLowerCase(); return words.every(w => t.includes(w)); });
    const starts = o => o.textContent.toLowerCase().split(/[\s·]+/).some(part => part.startsWith(words[0])) ? 0 : 1;
    return hits.map((o, i) => [o, i]).sort((a, b) => recentRank(a[0]) - recentRank(b[0]) || starts(a[0]) - starts(b[0]) || a[1] - b[1]).map(x => x[0]);
  }
  function enhance(select) {
    if (select.dataset.enhanced) return;
    select.dataset.enhanced = '1';
    const id = ++comboCount;
    const wrap = document.createElement('span');
    wrap.className = 'combo';
    const input = document.createElement('input');
    input.type = 'text';
    input.className = 'combo-input';
    input.autocomplete = 'off';
    input.spellcheck = false;
    input.placeholder = 'Type to search';
    input.setAttribute('role', 'combobox');
    input.setAttribute('aria-autocomplete', 'list');
    input.setAttribute('aria-expanded', 'false');
    input.setAttribute('aria-label', select.getAttribute('aria-label') || 'Category');
    const list = document.createElement('ul');
    list.className = 'combo-list';
    list.id = `combo-list-${id}`;
    list.setAttribute('role', 'listbox');
    list.setAttribute('aria-label', 'Categories');
    list.hidden = true;
    input.setAttribute('aria-controls', list.id);
    select.before(wrap);
    wrap.append(input, list, select);
    // The native select stays in the form (and is what is submitted); the text box drives it.
    select.hidden = true;
    select.tabIndex = -1;
    input.value = labelOf(select);
    let shown = [];
    let active = -1;
    const close = () => { list.hidden = true; input.setAttribute('aria-expanded', 'false'); input.removeAttribute('aria-activedescendant'); active = -1; };
    const mark = i => {
      active = i;
      shown.forEach((li, n) => li.setAttribute('aria-selected', n === i ? 'true' : 'false'));
      if (i >= 0 && shown[i]) { input.setAttribute('aria-activedescendant', shown[i].id); shown[i].scrollIntoView({ block: 'nearest' }); }
      else input.removeAttribute('aria-activedescendant');
    };
    const open = query => {
      const found = rank(select, query).slice(0, 60);
      list.replaceChildren();
      shown = found.map((o, n) => {
        const li = document.createElement('li');
        li.id = `${list.id}-${n}`;
        li.setAttribute('role', 'option');
        li.dataset.value = o.value;
        li.textContent = o.textContent;
        if ([...chosen, ...recent].includes(o.value) && !query) li.classList.add('recent');
        return li;
      });
      if (!shown.length) {
        const li = document.createElement('li');
        li.className = 'combo-empty';
        li.textContent = 'No category matches';
        list.append(li);
      } else list.append(...shown);
      list.hidden = false;
      input.setAttribute('aria-expanded', 'true');
      mark(shown.length ? 0 : -1);
      say(`${shown.length} ${shown.length === 1 ? 'category matches' : 'categories match'}`);
    };
    const choose = value => {
      select.value = value;
      select.dispatchEvent(new Event('change', { bubbles: true }));
      input.value = labelOf(select);
      if (value && select.id !== 'bulk-category') { const i = chosen.indexOf(value); if (i >= 0) chosen.splice(i, 1); chosen.unshift(value); }
      close();
    };
    select.addEventListener('change', () => { if (document.activeElement !== input) input.value = labelOf(select); });
    input.addEventListener('input', () => open(input.value));
    input.addEventListener('focus', () => input.select());
    input.addEventListener('keydown', ev => {
      if (ev.isComposing) return;
      const isOpen = !list.hidden;
      if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp') {
        ev.preventDefault();
        ev.stopPropagation();
        if (!isOpen) { open(ev.altKey ? '' : input.value === labelOf(select) ? '' : input.value); return; }
        if (shown.length) mark((active + (ev.key === 'ArrowDown' ? 1 : -1) + shown.length) % shown.length);
      } else if (ev.key === 'Enter') {
        // Never let Enter submit the whole form from here: it would accept every selected row.
        ev.preventDefault();
        ev.stopPropagation();
        if (isOpen && active >= 0) choose(shown[active].dataset.value);
        else if (isOpen) close();
        // Back to the row, where Enter accepts.
        input.closest('tr[data-review-row]')?.focus();
      } else if (ev.key === 'Escape') {
        ev.stopPropagation();
        if (isOpen) { ev.preventDefault(); input.value = labelOf(select); close(); }
        else input.closest('tr[data-review-row]')?.focus();
      } else if (ev.key === 'Tab' && isOpen && active >= 0 && input.value !== labelOf(select)) {
        choose(shown[active].dataset.value);
      }
    });
    list.addEventListener('mousedown', ev => ev.preventDefault());
    list.addEventListener('click', ev => { const li = ev.target.closest('[role=option]'); if (li) choose(li.dataset.value); });
    input.addEventListener('blur', () => {
      if (!list.hidden) close();
      const text = input.value.trim();
      if (text === labelOf(select)) return;
      if (!text) { choose(''); return; }
      const found = rank(select, text);
      if (found.length === 1) choose(found[0].value);
      else input.value = labelOf(select);
    });
  }

  // --- what each "Record as" needs -------------------------------------------------------------------
  function syncKind(row) {
    const kind = field(row, 'kind')?.value;
    if (!kind) return;
    for (const el of row.querySelectorAll('.for-kind')) el.hidden = !(el.dataset.kinds || '').split(' ').includes(kind);
  }
  function prepare(root) {
    root.querySelectorAll('select[data-combobox]').forEach(enhance);
    (root.matches?.('tr[data-review-row]') ? [root] : Array.from(root.querySelectorAll('tr[data-review-row]'))).forEach(syncKind);
  }

  // --- selection bar ---------------------------------------------------------------------------------
  const category = byId('bulk-category');
  const kind = byId('bulk-kind');
  const rule = byId('bulk-rule');
  const payee = byId('bulk-payee');
  const setPayee = byId('bulk-set-payee');
  const has = (select, value) => Array.from(select.options).some(o => o.value === value);
  const label = select => select.selectedOptions[0]?.textContent || '(none)';
  function plan(pick) {
    const row = pick.closest('tr[data-review-row]');
    const record = field(row, 'kind');
    const account = field(row, 'account');
    const name = field(row, 'payee');
    const reason = row.dataset.bulkBlock;
    if (reason) return { row, reason };
    // A category/rule edit must not turn a match, receipt deposit or prepared transfer into another
    // posting. Reclassify those rows one at a time.
    if (['MATCH', 'DEPOSIT', 'TRANSFER'].includes(record.value)) {
      return { row, reason: 'Keeps its match, receipt or transfer; change this row on its own to reclassify' };
    }
    if (rule.value && !row.dataset.matchingRules.split(' ').includes(rule.value)) return { row, reason: 'Does not match this rule' };
    if (rule.value && !has(category, rule.selectedOptions[0].dataset.account)) return { row, reason: 'Rule category is no longer available' };
    const nextKind = kind.value === 'KEEP' ? record.value : kind.value === 'AUTO' ? (has(record, 'CHECK') ? 'CHECK' : 'JOURNAL') : kind.value;
    if (!has(record, nextKind) || !['CHECK', 'JOURNAL'].includes(nextKind)) return { row, reason: 'Classification is incompatible with category/payee edits' };
    if (category.value && (!account || !has(account, category.value))) return { row, reason: 'Category is unavailable or is the mapped account itself' };
    if (setPayee.checked && (Array.from(payee.value).length > 120 || payee.value.includes('\0'))) {
      return { row, reason: 'Enter a payee of at most 120 characters without null characters' };
    }
    const edits = [];
    const add = (control, value, text) => { if (control && control.value !== value) edits.push({ control, value, text }); };
    const kindText = Array.from(record.options).find(o => o.value === nextKind).textContent;
    add(record, nextKind, `Record as: ${label(record)} → ${kindText}`);
    if (category.value) add(account, category.value, `Category: ${label(account)} → ${label(category)}`);
    if (setPayee.checked) add(name, payee.value, `Payee: ${name?.value || '(blank)'} → ${payee.value || '(blank)'}`);
    return { row, edits };
  }
  const selectedPlans = () => picks().filter(p => p.checked).map(plan);
  function preview() {
    if (!bar) return;
    const all = picks();
    const plans = selectedPlans();
    const changed = plans.filter(p => p.edits?.length).length;
    bar.hidden = plans.length === 0;
    byId('review-count').textContent = `${plans.length} selected`;
    const head = byId('review-all');
    if (head) { head.checked = all.length > 0 && plans.length === all.length; head.indeterminate = plans.length > 0 && plans.length < all.length; }
    byId('bulk-summary').textContent = `Preview: ${changed} will change, ${plans.filter(p => p.reason).length} skipped`;
    byId('bulk-preview').hidden = !(category.value || setPayee.checked || rule.value || kind.value !== 'AUTO');
    const list = byId('bulk-changes');
    list.replaceChildren();
    for (const p of plans) {
      const li = document.createElement('li');
      li.textContent = `${p.row.querySelector('.desc').firstChild?.textContent || ''}: ${p.reason ? `Skipped — ${p.reason}` : p.edits.map(e => e.text).join('; ') || 'No changes'}`;
      list.append(li);
    }
    byId('bulk-apply').disabled = changed === 0;
  }
  let anchor = null;
  function selectRange(from, to, checked) {
    const all = picks();
    const a = all.indexOf(from), b = all.indexOf(to);
    if (a < 0 || b < 0) return;
    all.slice(Math.min(a, b), Math.max(a, b) + 1).forEach(p => { p.checked = checked; });
  }
  form.addEventListener('click', ev => {
    const pick = ev.target.closest('input[name="sel"]');
    if (!pick) return;
    if (ev.shiftKey && anchor && anchor.isConnected) selectRange(anchor, pick, pick.checked);
    anchor = pick;
    preview();
  });
  byId('review-all')?.addEventListener('click', ev => { picks().forEach(p => { p.checked = ev.target.checked; }); anchor = null; preview(); });
  byId('review-clear')?.addEventListener('click', () => { picks().forEach(p => { p.checked = false; }); anchor = null; preview(); });
  bar?.addEventListener('input', () => preview());
  bar?.addEventListener('change', () => preview());
  rule?.addEventListener('change', () => {
    const option = rule.selectedOptions[0];
    if (rule.value) {
      category.value = option.dataset.account;
      category.dispatchEvent(new Event('change', { bubbles: true }));
      payee.value = option.dataset.payee;
      setPayee.checked = true;
    }
    preview();
  });
  byId('bulk-apply')?.addEventListener('click', () => {
    const plans = selectedPlans();
    let changed = 0;
    for (const p of plans) {
      if (!p.edits?.length) continue;
      for (const edit of p.edits) { edit.control.value = edit.value; edit.control.dispatchEvent(new Event('change', { bubbles: true })); }
      p.row.classList.add('draft-edited');
      syncKind(p.row);
      changed++;
    }
    if (changed) dirty = true;
    byId('bulk-result').textContent = `${changed} drafts updated; ${plans.filter(p => p.reason).length} skipped. Nothing recorded yet.`;
    preview();
  });

  // --- row keys ----------------------------------------------------------------------------------------
  const typing = el => el.matches('input:not([type=checkbox]):not([type=radio]):not([type=button]):not([type=submit]), textarea, select, [contenteditable]:not([contenteditable=false])');
  function move(row, step, extend) {
    const all = rows();
    const next = all[Math.max(0, Math.min(all.length - 1, all.indexOf(row) + step))];
    if (!next || next === row) return;
    if (extend) {
      const from = row.querySelector('input[name="sel"]');
      const to = next.querySelector('input[name="sel"]');
      if (from && to) { from.checked = true; to.checked = true; preview(); }
    }
    next.focus();
  }
  const press = button => { if (button && !button.disabled) form.requestSubmit(button); };
  function focusCategory(row) {
    const input = row.querySelector('.combo-input');
    if (input && !input.closest('[hidden]')) { input.focus(); return; }
    const kindSelect = field(row, 'kind');
    if (kindSelect) kindSelect.focus();
  }
  form.addEventListener('keydown', ev => {
    if (ev.isComposing) return;
    const row = ev.target.closest('tr[data-review-row]');
    // Enter in a text field finishes typing: it never submits the whole form (that would accept every
    // selected row). In a row, focus goes back to the row, where Enter accepts.
    if (ev.key === 'Enter' && ev.target.matches('input[type=text]')) { ev.preventDefault(); row?.focus(); return; }
    if (!row || ev.altKey) return;
    if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === 'a' && !typing(ev.target)) {
      ev.preventDefault(); picks().forEach(p => { p.checked = true; }); preview(); return;
    }
    if (ev.ctrlKey || ev.metaKey) return;
    if (ev.key === 'Escape' && typing(ev.target)) { row.focus(); return; }
    if (typing(ev.target)) return; // no shortcuts while typing or choosing in a list
    if ((ev.key === 'Enter' || ev.key === ' ') && ev.target.matches('button, a')) return; // their own action
    const key = ev.key.length === 1 ? ev.key.toLowerCase() : ev.key;
    if (key === 'ArrowDown' || key === 'j') move(row, 1, ev.shiftKey);
    else if (key === 'ArrowUp' || key === 'k') move(row, -1, ev.shiftKey);
    else if (key === 'Home') rows()[0]?.focus();
    else if (key === 'End') rows().at(-1)?.focus();
    else if (key === 'Enter') press(row.querySelector('[data-primary]'));
    else if (key === 'c') focusCategory(row);
    else if (key === 'i') press(row.querySelector('[data-ignore]'));
    else if (key === 'r') row.querySelector('[data-create-rule]')?.click();
    else if (key === ' ' && !ev.target.matches('input[name="sel"]')) {
      const pick = row.querySelector('input[name="sel"]');
      if (pick) { pick.checked = !pick.checked; anchor = pick; preview(); }
    } else return;
    ev.preventDefault();
  });
  // Clicking the row's plain text puts the keys on that row.
  form.addEventListener('mousedown', ev => {
    const row = ev.target.closest('tr[data-review-row]');
    if (row && !ev.target.closest('input, select, textarea, button, a, label, .combo')) window.setTimeout(() => row.focus(), 0);
  });
  form.addEventListener('change', ev => {
    const row = ev.target.closest('tr[data-review-row]');
    if (!row || ev.target.name === 'sel') return;
    dirty = true;
    if (ev.target.name === `kind_${row.dataset.reviewRow}`) syncKind(row);
    preview();
  });
  form.addEventListener('input', ev => { if (ev.target.closest('tr[data-review-row]') && ev.target.name !== 'sel') dirty = true; });

  // --- in-place actions ------------------------------------------------------------------------------
  // Runs before the preload (window capture precedes its document listener): a declined confirmation or a
  // second action while one is in flight never reaches it, so no single-use token is spent for nothing.
  let busy = false;
  let queued = null;
  let last = null;
  window.addEventListener('submit', ev => {
    if (ev.target !== form) return;
    const button = ev.submitter;
    if (button?.dataset.confirm && !window.confirm(button.dataset.confirm)) {
      ev.preventDefault(); ev.stopImmediatePropagation(); return;
    }
    if (busy) {
      ev.preventDefault(); ev.stopImmediatePropagation();
      queued = button || null;
      return;
    }
    busy = true;
    last = { name: button?.name || '', value: button?.value || '' };
    form.setAttribute('aria-busy', 'true');
    form.querySelector(`#review-row-${last.value}`)?.classList.add('row-busy');
  }, true);
  const done = () => { busy = false; form.removeAttribute('aria-busy'); form.querySelectorAll('.row-busy').forEach(r => r.classList.remove('row-busy')); };
  function nextOpenRow(after) {
    const all = rows();
    const i = all.indexOf(after);
    return all.slice(i + 1).find(r => r.dataset.status === 'NEW') || all.slice(0, Math.max(i, 0)).reverse().find(r => r.dataset.status === 'NEW');
  }
  form.addEventListener('fb:fragments', () => {
    done();
    prepare(form);
    preview();
    const results = form.querySelector('#review-status .results');
    if (results) say(results.textContent.trim());
    const row = last?.value ? form.querySelector(`#review-row-${last.value}`) : null;
    if (row) {
      if (row.classList.contains('row-error')) row.focus();
      else if (last.name === 'undo_one') row.querySelector('[data-primary]')?.focus();
      else if (row.dataset.status === 'NEW') row.focus();
      else (nextOpenRow(row) || row).focus();
    } else if (last?.name === 'action') {
      rows().find(r => r.dataset.status === 'NEW')?.focus();
    }
    if (queued && queued.isConnected) { const b = queued; queued = null; press(b); } else queued = null;
  });
  form.addEventListener('fb:submit-failed', () => { done(); queued = null; });

  // --- leaving with unsaved drafts ------------------------------------------------------------------------
  document.querySelector('.review-filters')?.addEventListener('submit', ev => {
    if (dirty && !window.confirm('Changing filters leaves these unsaved review drafts. Continue?')) ev.preventDefault();
  });
  document.addEventListener('click', ev => {
    const link = ev.target.closest('a');
    if (!link) return;
    if (link.hasAttribute('data-create-rule')) {
      const row = link.closest('[data-review-row]');
      const url = new URL(link.href);
      const account = row && field(row, 'account');
      const name = row && field(row, 'payee');
      if (account) url.searchParams.set('category', account.value);
      if (name) url.searchParams.set('payee', name.value);
      link.href = url.href;
    }
    if (dirty && !window.confirm('Leaving this page discards unsaved review drafts. Continue?')) ev.preventDefault();
  });

  prepare(form);
  preview();
  // After a full re-render with an error, start on the failed row.
  form.querySelector('tr.row-error')?.focus();
})();

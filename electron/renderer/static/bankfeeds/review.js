/* Review-only draft editing. Posting stays in the existing authenticated SubmitForm/core path. */
(() => {
  'use strict';
  const form = document.getElementById('bankfeed-review');
  const tools = document.getElementById('review-tools');
  if (!form || !tools) return;
  tools.hidden = false;
  const byId = id => document.getElementById(id);
  const picks = Array.from(form.querySelectorAll('input[name="sel"]'));
  const rowOf = pick => pick.closest('[data-review-row]');
  const field = (row, key) => row.querySelector(`[name="${key}_${row.dataset.reviewRow}"]`);
  const label = select => select.selectedOptions[0]?.textContent || '(none)';
  const has = (select, value) => Array.from(select.options).some(o => o.value === value);
  let anchor = null;
  let dirty = Array.from(form.querySelectorAll('[data-draft-retained]'))
    .some(row => row.dataset.draftRetained.toLowerCase() === 'true');
  const category = byId('bulk-category');
  const kind = byId('bulk-kind');
  const rule = byId('bulk-rule');
  const payee = byId('bulk-payee');
  const setPayee = byId('bulk-set-payee');

  function plan(pick) {
    const row = rowOf(pick);
    const record = field(row, 'kind');
    const account = field(row, 'account');
    const name = field(row, 'payee');
    const reason = row.dataset.bulkBlock;
    if (reason) return {row, reason};
    // A category/rule edit must not turn a match, receipt settlement or prepared transfer
    // into another posting. Reclassification requires changing that individual row first.
    if (['MATCH', 'DEPOSIT', 'TRANSFER'].includes(record.value)) {
      return {row, reason: 'Keep the existing match, receipt or transfer; change this row individually to reclassify'};
    }
    if (rule.value && !row.dataset.matchingRules.split(' ').includes(rule.value)) {
      return {row, reason: 'Does not match this rule'};
    }
    if (rule.value && !has(category, rule.selectedOptions[0].dataset.account)) {
      return {row, reason: 'Rule category is no longer available'};
    }
    const nextKind = kind.value === 'KEEP' ? record.value : kind.value === 'AUTO'
      ? (has(record, 'CHECK') ? 'CHECK' : 'JOURNAL') : kind.value;
    if (!has(record, nextKind) || !['CHECK', 'JOURNAL'].includes(nextKind)) {
      return {row, reason: 'Classification is incompatible with category/payee edits'};
    }
    if (category.value && !has(account, category.value)) {
      return {row, reason: 'Category is unavailable or is the mapped account itself'};
    }
    if (setPayee.checked && (Array.from(payee.value).length > 120 || payee.value.includes('\0'))) {
      return {row, reason: 'Enter a payee of at most 120 characters without null characters'};
    }
    const edits = [];
    const add = (control, value, text) => {
      if (control.value !== value) edits.push({control, value, text});
    };
    const kindText = Array.from(record.options).find(o => o.value === nextKind).textContent;
    add(record, nextKind, `Record as: ${label(record)} → ${kindText}`);
    if (category.value) add(account, category.value, `Category: ${label(account)} → ${label(category)}`);
    if (setPayee.checked) add(name, payee.value, `Payee: ${name.value || '(blank)'} → ${payee.value || '(blank)'}`);
    return {row, edits};
  }
  const selectedPlans = () => picks.filter(p => p.checked).map(plan);
  function preview() {
    const plans = selectedPlans();
    const changed = plans.filter(p => p.edits?.length).length;
    byId('review-count').textContent = `${plans.length} selected on this page`;
    byId('review-all').checked = picks.length > 0 && plans.length === picks.length;
    byId('review-all').indeterminate = plans.length > 0 && plans.length < picks.length;
    byId('bulk-summary').textContent = `${changed} drafts will change; ${plans.filter(p => p.reason).length} skipped; ${plans.length} selected`;
    const list = byId('bulk-changes');
    list.replaceChildren();
    for (const p of plans) {
      const li = document.createElement('li');
      li.textContent = `${p.row.querySelector('.desc').textContent}: ${p.reason ? `Skipped — ${p.reason}` : p.edits.map(e => e.text).join('; ') || 'No changes'}`;
      list.append(li);
    }
    byId('bulk-apply').disabled = changed === 0;
  }
  function range(end, checked) {
    const start = anchor ?? end;
    picks.slice(Math.min(start, end), Math.max(start, end) + 1).forEach(p => { p.checked = checked; });
  }
  for (const [index, pick] of picks.entries()) {
    pick.addEventListener('focus', () => { if (anchor === null) anchor = index; });
    pick.addEventListener('click', event => {
      if (event.shiftKey) range(index, pick.checked);
      else anchor = index;
      preview();
    });
    // Shortcuts are deliberately scoped to row checkboxes, never text/select/contenteditable.
    pick.addEventListener('keydown', event => {
      if (event.isComposing || event.altKey) return;
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'a') {
        event.preventDefault(); picks.forEach(p => { p.checked = true; }); preview(); return;
      }
      if (event.ctrlKey || event.metaKey || !['ArrowUp', 'ArrowDown', 'Home', 'End'].includes(event.key)) return;
      event.preventDefault();
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? picks.length - 1
        : Math.max(0, Math.min(picks.length - 1, index + (event.key === 'ArrowDown' ? 1 : -1)));
      if (event.shiftKey) range(next, true); else anchor = next;
      picks[next].focus(); preview();
    });
    pick.addEventListener('change', preview);
  }
  byId('review-all').addEventListener('click', event => {
    picks.forEach(p => { p.checked = event.target.checked; }); anchor = null; preview();
  });
  byId('review-clear').addEventListener('click', () => {
    picks.forEach(p => { p.checked = false; }); anchor = null; preview();
  });
  tools.addEventListener('input', event => { if (event.target.id !== 'review-all') preview(); });
  tools.addEventListener('change', preview);
  rule.addEventListener('change', () => {
    const option = rule.selectedOptions[0];
    if (rule.value) {
      category.value = option.dataset.account;
      payee.value = option.dataset.payee;
      setPayee.checked = true;
    }
    preview();
  });
  byId('bulk-apply').addEventListener('click', () => {
    const plans = selectedPlans();
    let changed = 0;
    for (const p of plans) {
      if (!p.edits?.length) continue;
      for (const edit of p.edits) edit.control.value = edit.value;
      p.row.classList.add('draft-edited');
      changed++;
    }
    if (changed) dirty = true;
    byId('bulk-result').textContent = `${changed} review drafts updated; ${plans.filter(p => p.reason).length} skipped. Nothing posted.`;
    preview();
  });
  form.addEventListener('input', event => {
    if (event.target.closest('[data-review-row]') && event.target.name !== 'sel') dirty = true;
    preview();
  });
  form.addEventListener('change', event => {
    if (event.target.closest('[data-review-row]') && event.target.name !== 'sel') dirty = true;
    preview();
  });
  document.querySelector('.review-filters')?.addEventListener('submit', event => {
    if (dirty && !window.confirm('Changing filters leaves these unsaved review drafts. Continue?')) event.preventDefault();
  });
  document.addEventListener('click', event => {
    const link = event.target.closest('a');
    if (!link) return;
    if (link.hasAttribute('data-create-rule')) {
      const row = link.closest('[data-review-row]');
      const url = new URL(link.href);
      url.searchParams.set('category', field(row, 'account').value);
      url.searchParams.set('payee', field(row, 'payee').value);
      link.href = url.href;
    }
    if (dirty && !window.confirm('Leaving this page discards unsaved review drafts. Continue?')) event.preventDefault();
  });
  // Return focus to a failed row without moving focus during ordinary draft editing.
  form.querySelector('.row-error input[name="sel"]')?.focus();
  preview();
})();

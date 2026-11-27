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
  const hidden = (name, value) => { const input = document.createElement('input'); input.type = 'hidden'; input.name = name; input.value = value; form.append(input); return input; };
  hidden('inline', '1');
  // Worker items 40 and 42: the reply draws every row the page shows again (`shown`), keeping a typed
  // draft only in the rows listed in `edited`. A row leaves `edited` once the reply no longer holds a
  // draft for it (accepted, or drawn fresh), so "unsaved review drafts" means one is really left.
  const shownField = hidden('shown', ''), editedField = hidden('edited', '');
  const retained = row => (row.dataset.draftRetained || '').toLowerCase() === 'true';
  const edits = new Set(rows().filter(retained).map(row => row.dataset.reviewRow));
  const known = new WeakSet(rows());
  // An action is in flight (between the post and its reply); see "an action in flight" below.
  let busy = false;
  const late = new Set();
  const edit = row => {
    const pk = row?.dataset.reviewRow;
    if (!pk) return;
    edits.add(pk);
    if (busy) late.add(pk);
  };
  const dirty = () => edits.size > 0;
  function settleEdits() {
    for (const pk of [...edits]) {
      const row = byId(`review-row-${pk}`);
      if (!row || row.dataset.status !== 'NEW' || (!known.has(row) && !retained(row))) edits.delete(pk);
    }
    rows().forEach(row => known.add(row));
  }
  // Worker item 43: the left menu's Review count follows the reply's count.
  function menuCount() {
    const count = Number(byId('review-status')?.dataset.toReview);
    const link = byId('nav-review');
    if (!link || !Number.isInteger(count) || count < 0) return;
    let badge = link.querySelector('.nav-badge');
    if (count > 0 && !badge) {
      badge = document.createElement('span'); badge.className = 'nav-badge';
      const words = document.createElement('span'); words.className = 'sr-only'; words.textContent = ' to review';
      link.append(badge, words);
    }
    if (count > 0) badge.textContent = String(count);
    else if (badge) { if (badge.nextElementSibling?.classList.contains('sr-only')) badge.nextElementSibling.remove(); badge.remove(); }
    link.classList.toggle('has-badge', count > 0);
  }

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

  // --- split a row over several accounts (worker item 46) ----------------------------------------------
  // Amounts are exact cents (BigInt): the lines must add up to the row's amount before Accept is sent.
  const AMOUNT = /^-?\d+(\.\d{1,2})?$/;
  const toCents = text => {
    const t = String(text).trim().replace(/^-\s+/, '-');
    if (!AMOUNT.test(t)) return null;
    const [whole, frac = ''] = t.replace('-', '').split('.');
    const n = BigInt(whole) * 100n + BigInt(frac.padEnd(2, '0'));
    return t.startsWith('-') ? -n : n;
  };
  const money = c => { const a = c < 0n ? -c : c; return `${c < 0n ? '-' : ''}${(a / 100n).toLocaleString('en-US')}.${String(a % 100n).padStart(2, '0')}`; };
  const splitBox = row => row.querySelector('[data-split]');
  const splitOn = row => !!row.querySelector('[data-split-toggle]')?.checked && ['CHECK', 'JOURNAL'].includes(field(row, 'kind')?.value);
  function splitState(row) {
    const box = splitBox(row);
    const want = toCents(box?.dataset.amount || '') ?? 0n;
    let total = 0n, count = 0, bad = null;
    for (const line of box ? box.querySelectorAll('[data-split-line]') : []) {
      const amount = line.querySelector('[data-split-amount]').value.trim();
      const account = line.querySelector('select').value;
      const memo = line.querySelector('[name^="split_memo_"]').value.trim();
      if (!amount && !account && !memo) continue;
      count++;
      const n = toCents(amount);
      if (n === null || n === 0n) { if (bad === null) bad = amount; continue; }
      total += n;
    }
    return { want, total, count, bad, left: want - total };
  }
  function showSplit(row) {
    const out = row.querySelector('[data-split-left]');
    if (!out) return;
    const s = splitState(row);
    out.classList.remove('ok', 'short', 'over');
    if (s.bad !== null) { out.textContent = s.bad ? `${s.bad} is not an amount` : 'Enter an amount on each line'; out.classList.add('over'); }
    else if (s.left === 0n) { out.textContent = 'Remaining 0.00'; out.classList.add('ok'); }
    else if (s.left > 0n) { out.textContent = `Remaining ${money(s.left)}`; out.classList.add('short'); }
    else { out.textContent = `Over by ${money(-s.left)}`; out.classList.add('over'); }
  }
  // What the server would refuse, said at the lines before anything is sent.
  function splitProblem(row) {
    const s = splitState(row);
    if (s.count === 0) return 'Enter the split lines, or untick Split.';
    if (s.bad !== null) return 'Enter an amount such as 19.99 on each split line.';
    if (s.left !== 0n) return `The split lines add up to ${money(s.total)}, not the bank's ${money(s.want)}: ${s.left > 0n ? `${money(s.left)} is left to assign` : `they are ${money(-s.left)} over`}. Nothing was recorded.`;
    return '';
  }
  function newLine(row, account = '') {
    const pk = row.dataset.reviewRow;
    const box = splitBox(row);
    const n = box.querySelectorAll('[data-split-line]').length + 1;
    const make = (tag, props, attrs = {}) => { const el = Object.assign(document.createElement(tag), props); for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v); return el; };
    const line = make('span', { className: 'split-line' }, { 'data-split-line': '' });
    const select = make('select', { name: `split_account_${pk}` }, { 'data-combobox': '', 'aria-label': `Account, split line ${n}` });
    for (const o of field(row, 'account')?.options || []) {
      if (o.value && o.textContent.startsWith('Unavailable')) continue;
      select.append(new Option(o.textContent, o.value, false, !!account && o.value === account));
    }
    const nums = make('span', { className: 'split-nums' });
    nums.append(
      make('input', { type: 'text', name: `split_amount_${pk}`, maxLength: 20, autocomplete: 'off', placeholder: 'Amount' }, { inputmode: 'decimal', 'aria-label': `Amount, split line ${n}`, 'data-split-amount': '' }),
      make('input', { type: 'text', name: `split_memo_${pk}`, maxLength: 160, placeholder: 'Memo' }, { 'aria-label': `Memo, split line ${n}` }),
      make('button', { type: 'button', className: 'split-remove', textContent: '×' }, { 'data-split-remove': '', 'aria-label': `Remove split line ${n}` }));
    line.append(select, nums);
    box.querySelector('[data-split-add]').before(line);
    enhance(select);
    return line;
  }
  function toggleSplit(row) {
    const box = splitBox(row);
    if (!box) return;
    const on = !!row.querySelector('[data-split-toggle]')?.checked;
    box.querySelector('[data-split-lines]').hidden = !on;
    const single = row.querySelector('.single-cat');
    if (single) single.hidden = on;
    if (on && !box.querySelector('[data-split-line]')) {
      // The row's category (typed, or a rule's) starts the first line; the amounts are the person's.
      const first = newLine(row, field(row, 'account')?.value || '');
      newLine(row);
      first.querySelector('[data-split-amount]').focus({ preventScroll: true });
    }
    showSplit(row);
  }
  form.addEventListener('click', ev => {
    const row = ev.target.closest('tr[data-review-row]');
    if (!row) return;
    if (ev.target.closest('[data-split-add]')) {
      newLine(row).querySelector('.combo-input, select')?.focus({ preventScroll: true });
      edit(row);
      showSplit(row);
    } else if (ev.target.closest('[data-split-remove]')) {
      const lines = row.querySelectorAll('[data-split-line]');
      if (lines.length > 1) ev.target.closest('[data-split-line]').remove();
      else lines[0].querySelectorAll('input').forEach(i => { i.value = ''; });
      edit(row);
      showSplit(row);
      row.querySelector('[data-split-add]')?.focus({ preventScroll: true });
    }
  });

  // --- deposit received payments (worker items 47 and 48) ----------------------------------------------
  // The ticked payments plus the fee and adjustment lines (a fee is negative) must equal the bank row.
  const depositBox = row => row.querySelector('[data-deposit]');
  const depositOn = row => field(row, 'kind')?.value === 'DEPOSIT' && !!depositBox(row);
  function depositState(row) {
    const box = depositBox(row);
    const want = toCents(box?.dataset.amount || '') ?? 0n;
    let paid = 0n, ticked = 0, fees = 0n, any = false, bad = false;
    for (const pick of box ? box.querySelectorAll('input[name^="receipt_"]:checked') : []) { ticked++; paid += toCents(pick.dataset.amount) ?? 0n; }
    for (const line of box ? box.querySelectorAll('[data-fee-line]') : []) {
      const text = line.querySelector('input').value.trim();
      if (!text) continue;
      any = true;
      const n = toCents(text);
      if (n === null || n === 0n || !line.querySelector('select').value) bad = true; else fees += n;
    }
    return { want, paid, ticked, fees, any, bad, total: paid + fees };
  }
  function showDeposit(row) {
    const out = row.querySelector('[data-deposit-left]');
    if (!out) return;
    const d = depositState(row);
    out.classList.remove('ok', 'short', 'over');
    if (d.bad) { out.textContent = 'Enter each fee with its account and an amount such as -24.95'; out.classList.add('over'); return; }
    const parts = `Payments ${money(d.paid)}${d.any ? ` · fees ${money(d.fees)}` : ''}`;
    if (d.total === d.want && d.ticked) { out.textContent = `${parts} = bank ${money(d.want)}`; out.classList.add('ok'); }
    else if (d.total < d.want) { out.textContent = `${parts}: ${money(d.want - d.total)} short of the bank's ${money(d.want)}`; out.classList.add('short'); }
    else { out.textContent = `${parts}: ${money(d.total - d.want)} more than the bank's ${money(d.want)}`; out.classList.add('over'); }
  }
  function depositProblem(row) {
    const d = depositState(row);
    if (!d.ticked) return 'Tick the received payments in this bank deposit.';
    if (d.bad) return 'Enter each fee or adjustment with its account and an amount such as -24.95.';
    if (d.total !== d.want) return `${d.any ? 'The selected receipts and fees total' : 'The selected receipts total'} ${money(d.total)}, not the bank's ${money(d.want)}.`;
    return '';
  }
  function addFeeLine(row) {
    const box = depositBox(row);
    const line = box.querySelector('[data-fee-line]:last-of-type').cloneNode(true);
    line.querySelector('select').value = box.dataset.default || '';
    line.querySelector('input').value = '';
    box.querySelector('[data-fee-rows]').append(line);
    box.querySelectorAll('[data-fee-line]').forEach((l, i) => {
      l.querySelector('select').setAttribute('aria-label', `Account, fee or adjustment line ${i + 1}`);
      l.querySelector('input').setAttribute('aria-label', `Amount, fee or adjustment line ${i + 1}`);
    });
    return line;
  }
  form.addEventListener('click', ev => {
    const row = ev.target.closest('tr[data-review-row]');
    const add = ev.target.closest('[data-fee-add]');
    if (!row || !add) return;
    addFeeLine(row).querySelector('select').focus();
    edit(row);
    showDeposit(row);
  });

  // --- match multiple posted entries (FY2025 banking fix) ------------------------------------------------
  const multiBox = row => row.querySelector('[data-multi-match]');
  const multiActive = row => {
    const box = multiBox(row);
    if (!box || field(row, 'kind')?.value !== 'MATCH') return false;
    if (box.dataset.hasSingle !== '1') return true;
    return box.querySelectorAll('input[name^="match_line_"]:checked').length > 0;
  };
  function multiState(row) {
    const box = multiBox(row);
    const want = toCents(box?.dataset.amount || '') ?? 0n;
    let total = 0n, ticked = 0;
    for (const pick of box ? box.querySelectorAll('input[name^="match_line_"]:checked') : []) {
      ticked++;
      total += toCents(pick.dataset.amount) ?? 0n;
    }
    return { want, total, ticked };
  }
  function showMultiMatch(row) {
    const out = row.querySelector('[data-multi-match-left]');
    if (!out) return;
    const m = multiState(row);
    out.classList.remove('ok', 'short', 'over');
    if (m.ticked > 20) { out.textContent = `Selected ${m.ticked} entries: at most 20 entries can match one transaction`; out.classList.add('over'); }
    else if (m.ticked >= 2 && m.total === m.want) { out.textContent = `Selected ${money(m.total)} = bank ${money(m.want)}`; out.classList.add('ok'); }
    else if (m.total < m.want) { out.textContent = `Selected ${money(m.total)}: ${money(m.want - m.total)} short of the bank's ${money(m.want)}`; out.classList.add('short'); }
    else { out.textContent = `Selected ${money(m.total)}: ${money(m.total - m.want)} more than the bank's ${money(m.want)}`; out.classList.add('over'); }
  }
  function multiMatchProblem(row) {
    const m = multiState(row);
    if (m.ticked < 2) return 'Tick at least two posted entries whose amounts total this transaction.';
    if (m.ticked > 20) return 'A bank transaction matches at most 20 posted entries.';
    if (m.total !== m.want) return `The selected entries total ${money(m.total)}, not the bank's ${money(m.want)}.`;
    return '';
  }

  // --- what each "Record as" needs -------------------------------------------------------------------
  function syncKind(row) {
    const kind = field(row, 'kind')?.value;
    if (!kind) return;
    for (const el of row.querySelectorAll('.for-kind')) el.hidden = !(el.dataset.kinds || '').split(' ').includes(kind);
  }
  function prepare(root) {
    root.querySelectorAll('select[data-combobox]').forEach(enhance);
    (root.matches?.('tr[data-review-row]') ? [root] : Array.from(root.querySelectorAll('tr[data-review-row]'))).forEach(row => { syncKind(row); showSplit(row); showDeposit(row); showMultiMatch(row); });
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
    if (row.querySelector('[data-split-toggle]')?.checked) return { row, reason: 'Keeps its split lines; change this row on its own' };
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
      edit(p.row);
      changed++;
    }
    byId('bulk-result').textContent = `${changed} drafts updated; ${plans.filter(p => p.reason).length} skipped. Nothing recorded yet.`;
    preview();
    if (changed) {
      byId('bulk-preview').hidden = false;
      byId('bulk-summary').textContent = 'Applied changes';
      byId('bulk-changes').replaceChildren(...plans.map(p => {
        const li = document.createElement('li');
        const change = p.reason || p.edits.map(e => e.text).join('; ') || 'Already had these settings';
        li.textContent = `${p.row.querySelector('.desc').firstChild.textContent}: ${change}`;
        return li;
      }));
    }
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
    if ((ev.key === 'Enter' || ev.key === ' ') && ev.target.closest('button, a, summary, input[type=checkbox]:not([name="sel"])')) return; // their own action
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
    if (row && !ev.target.closest('input, select, textarea, button, a, label, summary, .combo')) window.setTimeout(() => row.focus(), 0);
  });
  form.addEventListener('change', ev => {
    const row = ev.target.closest('tr[data-review-row]');
    if (!row || ev.target.name === 'sel') return;
    edit(row);
    if (ev.target.name === `kind_${row.dataset.reviewRow}`) syncKind(row);
    if (ev.target.matches('[data-split-toggle]')) toggleSplit(row);
    if (ev.target.closest('[data-split]')) showSplit(row);
    if (ev.target.closest('[data-deposit]')) showDeposit(row);
    if (ev.target.closest('[data-multi-match]')) showMultiMatch(row);
    preview();
  });
  form.addEventListener('input', ev => {
    if (ev.target.name === 'sel') return;
    const row = ev.target.closest('tr[data-review-row]');
    edit(row);
    if (row && ev.target.closest('[data-split]')) showSplit(row);
    if (row && ev.target.closest('[data-deposit]')) showDeposit(row);
    if (row && ev.target.closest('[data-multi-match]')) showMultiMatch(row);
  });

  // --- an action in flight (worker item 40, review S2) -----------------------------------------------------
  // The reply draws every shown row again from what was posted. What the person types, chooses or ticks on a
  // row while the reply is on its way is newer than that: it is carried over to the row the reply draws (by
  // name and position, with as many split and fee lines), so nothing typed is lost and the row stays a draft.
  // The reply's own choices for the row (what it may match, its notes) are kept. Focus and caret stay where
  // the person is typing.
  let sent = null; // the rows as they were when the action was sent, and their ticks
  let lastFocus = null, focusMoved = false;
  form.addEventListener('focusin', ev => { lastFocus = ev.target; if (busy) focusMoved = true; });
  const named = row => {
    const found = new Map();
    for (const el of row.querySelectorAll('input[name], select[name], textarea[name]')) {
      if (el.type === 'hidden' || el.name === 'sel') continue;
      if (!found.has(el.name)) found.set(el.name, []);
      found.get(el.name).push(el);
    }
    return found;
  };
  function carry(from, to) {
    const count = (row, selector) => row.querySelectorAll(selector).length;
    if (splitBox(to)) {
      while (count(to, '[data-split-line]') < count(from, '[data-split-line]')) newLine(to);
      while (count(to, '[data-split-line]') > Math.max(count(from, '[data-split-line]'), 1)) Array.from(to.querySelectorAll('[data-split-line]')).at(-1).remove();
    }
    if (depositBox(to)) {
      while (count(to, '[data-fee-line]') < count(from, '[data-fee-line]')) addFeeLine(to);
      while (count(to, '[data-fee-line]') > Math.max(count(from, '[data-fee-line]'), 1)) Array.from(to.querySelectorAll('[data-fee-line]')).at(-1).remove();
    }
    const now = named(to);
    for (const [name, olds] of named(from)) {
      const news = now.get(name) || [];
      olds.forEach((old, i) => {
        if (old.type === 'checkbox' || old.type === 'radio') {
          const box = news.find(n => n.value === old.value);
          if (box) box.checked = old.checked;
          return;
        }
        const el = news[i];
        if (!el || el.value === old.value) return;
        // A choice the reply no longer offers stays visible, as the server keeps one ("choose again").
        if (el.tagName === 'SELECT' && !Array.from(el.options).some(o => o.value === old.value)) el.append(new Option('Unavailable choice — choose again', old.value));
        el.value = old.value;
      });
    }
    for (const select of to.querySelectorAll('select[data-enhanced]')) {
      const input = select.closest('.combo')?.querySelector('.combo-input');
      if (input) input.value = labelOf(select);
    }
    if (splitBox(to)) toggleSplit(to);
    to.dataset.draftRetained = 'true';
  }
  // Where the focus was, in the row drawn again: the same named control (or its search box), by position.
  function focusIn(from, to, el) {
    const combo = el.matches('.combo-input') ? el.closest('.combo')?.querySelector('select') : null;
    const control = combo || el;
    if (!control.name) return el.matches('tr[data-review-row]') ? to : null;
    const index = (named(from).get(control.name) || []).indexOf(control);
    const target = (named(to).get(control.name) || [])[index] || (control.name === 'sel' ? to.querySelector('input[name="sel"]') : null);
    if (!target) return null;
    return combo ? target.closest('.combo')?.querySelector('.combo-input') || null : target;
  }
  // Called first on each reply. Returns where the focus was, if the person moved it into a row while the
  // action was in flight; it is put back once the rows are prepared (their search boxes made).
  function carryLate() {
    if (!sent) return null;
    const { rows: was, ticks } = sent;
    let focus = null;
    for (const [pk, from] of was) {
      const to = byId(`review-row-${pk}`);
      if (!to || to === from || to.dataset.status !== 'NEW') continue;
      const tick = from.querySelector('input[name="sel"]'), now = to.querySelector('input[name="sel"]');
      if (tick && now && tick.checked !== ticks.get(pk)) now.checked = tick.checked;
      // A row that cannot be carried over stays as the reply drew it; the reply itself must still be taken.
      if (late.has(pk)) { try { carry(from, to); } catch {} }
      if (focusMoved && lastFocus && !lastFocus.isConnected && from.contains(lastFocus)) {
        focus = { from, to, el: lastFocus, text: lastFocus.matches('.combo-input') ? lastFocus.value : null, start: lastFocus.selectionStart, end: lastFocus.selectionEnd };
      }
    }
    sent = null;
    late.clear();
    return focus;
  }
  function restoreFocus(focus) {
    const target = focus && focus.to.isConnected ? focusIn(focus.from, focus.to, focus.el) : null;
    if (!target) return false;
    target.focus({ preventScroll: true });
    if (focus.text !== null && target.value !== focus.text) target.value = focus.text;
    // The preload selects a prefilled number box on focus, and a search box selects its text; the person was
    // typing there, so the caret goes back where it was.
    if (typeof focus.start === 'number' && typeof target.setSelectionRange === 'function') {
      try { target.setSelectionRange(focus.start, focus.end ?? focus.start); } catch {}
    }
    return true;
  }

  // --- in-place actions ------------------------------------------------------------------------------
  // Runs before the preload (window capture precedes its document listener): a declined confirmation or a
  // second action while one is in flight never reaches it, so no single-use token is spent for nothing.
  let queued = null;
  let last = null;
  window.addEventListener('submit', ev => {
    if (ev.target !== form) return;
    const button = ev.submitter;
    // Worker item 46: a split that does not add up to the row is refused at its lines; nothing is sent.
    // Worker item 48: so is a deposit whose ticked payments and fees do not equal the bank amount.
    if (['accept_one', 'accept_again_one', 'match_one'].includes(button?.name)) {
      const row = byId(`review-row-${button.value}`);
      const isMatchBtn = button.name === 'match_one';
      const problem = isMatchBtn
        ? (row && multiActive(row) ? multiMatchProblem(row) : '')
        : (row && splitOn(row) ? splitProblem(row) : row && depositOn(row) ? depositProblem(row) : row && multiActive(row) ? multiMatchProblem(row) : '');
      if (problem) {
        ev.preventDefault(); ev.stopImmediatePropagation();
        const selector = isMatchBtn ? '[data-multi-match-error]' : (splitOn(row) ? '[data-split-error]' : depositOn(row) ? '[data-deposit-error]' : '[data-multi-match-error]');
        const box = row.querySelector(selector);
        const details = box?.closest('details');
        if (details) details.open = true;
        if (box) { box.textContent = problem; box.hidden = false; }
        say(problem);
        const focusTarget = isMatchBtn ? '[data-multi-match] input' : (splitOn(row) ? '[data-split-amount]' : depositOn(row) ? '[data-deposit] input' : '[data-multi-match] input');
        (row.querySelector(focusTarget) || row).focus({ preventScroll: true });
        return;
      }
    }
    if (button?.dataset.confirm && !window.confirm(button.dataset.confirm)) {
      ev.preventDefault(); ev.stopImmediatePropagation(); return;
    }
    if (busy) {
      ev.preventDefault(); ev.stopImmediatePropagation();
      queued = button || null;
      return;
    }
    busy = true;
    sent = { rows: new Map(rows().map(row => [row.dataset.reviewRow, row])), ticks: new Map(rows().map(row => [row.dataset.reviewRow, !!row.querySelector('input[name="sel"]')?.checked])) };
    late.clear(); focusMoved = false;
    shownField.value = rows().map(row => row.dataset.reviewRow).join(',');
    editedField.value = [...edits].join(',');
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
    const focus = carryLate();
    done();
    settleEdits();
    menuCount();
    prepare(form);
    preview();
    const results = form.querySelector('#review-status .results');
    if (results) say(results.textContent.trim());
    const row = last?.value ? form.querySelector(`#review-row-${last.value}`) : null;
    // The person moved on while the action was in flight: the focus stays where they are.
    const stay = restoreFocus(focus) || (focusMoved && !!document.activeElement && document.activeElement !== document.body);
    focusMoved = false;
    if (stay) {
      // nothing to move
    } else if (row) {
      if (row.classList.contains('row-error')) row.focus({ preventScroll: true });
      else if (last.name === 'undo_one') row.querySelector('[data-primary]')?.focus({ preventScroll: true });
      else if (row.dataset.status === 'NEW') row.focus({ preventScroll: true });
      else (nextOpenRow(row) || row).focus({ preventScroll: true });
    } else if (last?.name === 'action') {
      rows().find(r => r.dataset.status === 'NEW')?.focus({ preventScroll: true });
    }
    if (queued && queued.isConnected) { const b = queued; queued = null; press(b); } else queued = null;
  });
  form.addEventListener('fb:submit-failed', () => { done(); queued = null; sent = null; late.clear(); focusMoved = false; });

  // --- leaving with unsaved drafts ------------------------------------------------------------------------
  document.querySelector('.review-filters')?.addEventListener('submit', ev => {
    if (dirty() && !window.confirm('Changing filters leaves these unsaved review drafts. Continue?')) ev.preventDefault();
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
    if (dirty() && !window.confirm('Leaving this page discards unsaved review drafts. Continue?')) ev.preventDefault();
  });

  prepare(form);
  preview();
  // After a full re-render with an error, start on the failed row.
  form.querySelector('tr.row-error')?.focus();
})();

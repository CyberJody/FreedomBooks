'use strict';
// Sandboxed, isolated preload (ARCHITECTURE 8). No page API is exposed. Its own
// event handlers turn POST forms into path-bound commands and handle CSV exports.
const { ipcRenderer } = require('electron');

async function call(name, payload) {
  const r = await ipcRenderer.invoke(`fb:${name}`, payload);
  if (!r || !r.ok) throw new Error((r && r.error) || 'Call failed');
  return r.value;
}

// No page needs a bridge API. Only these isolated preload handlers invoke IPC.

function follow(result) {
  if (result.outcome === 'redirect') {
    const target = new URL(result.location, window.location.href);
    if (target.origin === window.location.origin && target.pathname === window.location.pathname && target.search === window.location.search) {
      // A bridged POST did not navigate away. Django's redirect still GETs the register when only
      // its line anchor changed; a fragment-only assign would leave stale marks and a spent token.
      window.history.replaceState(null, '', target.href);
      window.location.reload();
    } else window.location.assign(result.location);
  }
  else if (result.outcome === 'invalid') window.location.replace(result.reload);
}

function fieldsOf(form, submitter) {
  const data = new FormData(form, submitter || undefined);
  const out = [];
  for (const [k, v] of data.entries()) if (typeof v === 'string') out.push([k, v]);
  return out;
}

// Approved issue and SimpleFIN setup links open in the system browser. The main process
// re-checks the exact allowlist; outside links never navigate the app window.
const ISSUE_URL = 'https://github.com/CyberJody/FreedomBooks/issues/new/choose';
const SIMPLEFIN_URL = 'https://bridge.simplefin.org/simplefin/create';
document.addEventListener('click', async (event) => {
  const anchor = event.target.closest && event.target.closest('a[href]');
  if (!anchor || event.defaultPrevented) return;
  const href = anchor.getAttribute('href');
  if (!/^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith('fb://app/')) return;
  event.preventDefault();
  if (![ISSUE_URL, SIMPLEFIN_URL].includes(href) || event.button !== 0) return;
  try {
    await call('app.openExternal', { url: href });
  } catch (e) { window.alert(String(e.message || e)); }
}, true);

// CSV links retain Django's URLs; a desktop Save dialog replaces the browser download.
document.addEventListener('click', async (event) => {
  const anchor = event.target.closest && event.target.closest('a[href]');
  if (!anchor || event.defaultPrevented || event.button !== 0) return;
  const url = new URL(anchor.href, window.location.href);
  const match = /^\/c\/([0-9a-f-]{36})\/history\/$/.exec(url.pathname);
  if (url.origin !== window.location.origin || !match || url.searchParams.get('format') !== 'csv') return;
  event.preventDefault();
  try {
    await call('books.run', { type: 'ExportHistory', company_id: match[1], filters: Array.from(url.searchParams.entries()) });
  } catch (e) { window.alert(String(e.message || e)); }
}, true);

// D8 report CSVs keep the GET links and use the existing typed export/Save-dialog path.
document.addEventListener('click', async event => {
  const anchor = event.target.closest && event.target.closest('a[href]');
  if (!anchor || event.defaultPrevented || event.button !== 0) return;
  const target = new URL(anchor.href, window.location.href);
  const match = /^\/c\/([0-9a-f-]{36})\/reports\/(profit-and-loss|balance-sheet|general-ledger)\/$/.exec(target.pathname);
  if (target.origin !== window.location.origin || !match || target.searchParams.get('output') !== 'csv') return;
  event.preventDefault();
  try {
    const names = {'profit-and-loss':'profit_loss','balance-sheet':'balance_sheet','general-ledger':'general_ledger'};
    await call('books.run', {type:'ExportReport', company_id:match[1], report:names[match[2]], filters:Array.from(target.searchParams.entries())});
  } catch (e) { window.alert(String(e.message || e)); }
}, true);

// BACKUPS-POLISH: while a backup, restore or copy runs (in the main process's worker, so the window keeps
// answering), the Backups page says so and its buttons are disabled; they come back if it is refused.
// Other pages wait for it to finish, so the line says that, with the time so far. The previous
// action's messages are hidden meanwhile, so an old refusal never sits beside the new attempt.
const BACKUPS_PAGE = /^\/c\/[0-9a-f-]{36}\/backups\/(create\/)?$/;
const BUSY_WAIT = ' Other pages open when it finishes.';
const BUSY_LABELS = { setup:'Creating your books…', protection:'Changing book protection… A safety backup is checked first.', create: `Backing up… This can take a while for a large book.${BUSY_WAIT}`, restore: `Restoring… Choose the backup file, then wait while it is checked and restored.${BUSY_WAIT}`, copy: 'Saving a copy of the backup…' };
function backupsBusy(kind) {
  const main = document.querySelector('main');
  if (!main || !BUSY_LABELS[kind]) return () => {};
  const line = document.createElement('p');
  line.className = 'notice backup-busy';
  line.setAttribute('role', 'status');
  line.textContent = BUSY_LABELS[kind];
  const elapsed = document.createElement('span');
  elapsed.className = 'backup-elapsed';
  elapsed.setAttribute('aria-hidden', 'true');
  line.append(' ', elapsed);
  const started = Date.now();
  const tick = () => {
    const s = Math.floor((Date.now() - started) / 1000);
    elapsed.textContent = `Elapsed ${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}.`;
  };
  tick();
  const timer = setInterval(tick, 1000);
  const stale = Array.from(main.querySelectorAll(':scope > p.errorlist, :scope > p.notice, form.backup-restored')).filter(m => !m.hidden);
  stale.forEach(m => { m.hidden = true; });
  const at = main.querySelector('.backup-status');
  if (at) at.before(line); else main.prepend(line);
  main.setAttribute('aria-busy', 'true');
  const controls = Array.from(main.querySelectorAll('button, input')).filter(c => !c.disabled);
  controls.forEach(c => { c.disabled = true; });
  const links = Array.from(main.querySelectorAll('table.backup-files a'));
  links.forEach(a => a.setAttribute('aria-disabled', 'true'));
  return () => {
    clearInterval(timer);
    line.remove();
    stale.forEach(m => { m.hidden = false; });
    main.removeAttribute('aria-busy');
    controls.forEach(c => { c.disabled = false; });
    links.forEach(a => a.removeAttribute('aria-disabled'));
  };
}
// SIM-FIX item 1 (deliberate divergence, DESKTOP-WAVES.md): the frozen forms put their submit buttons
// after the growing line list, so "Add line" pushed "Save estimate" / "Estimate list" down by a whole
// line and a click aimed at them landed on the new line instead. Keep the clicked add button (and so
// the action row beside and below it) where the pointer is. Measured before the page script runs
// (capture on document), corrected after it (bubble on window), so the page's own focus() is undone.
const ADD_BUTTONS = '#add-line, #add-allocation, .add-line';
let addAnchor = null;
document.addEventListener('click', (event) => {
  const button = event.target.closest && event.target.closest(ADD_BUTTONS);
  addAnchor = button && button.form ? { button, top: button.getBoundingClientRect().top } : null;
}, true);
window.addEventListener('click', () => {
  const anchor = addAnchor;
  addAnchor = null;
  if (!anchor || !anchor.button.isConnected) return;
  const settle = () => {
    const moved = anchor.button.getBoundingClientRect().top - anchor.top;
    if (!moved) return;
    // Near the end of the page there is no room left to scroll; add just enough room below the footer.
    const room = document.documentElement.scrollHeight - window.innerHeight - window.scrollY;
    if (moved > room) {
      const pad = parseFloat(document.body.style.paddingBottom) || 0;
      document.body.style.paddingBottom = `${Math.ceil(pad + moved - room)}px`;
    }
    window.scrollBy(0, moved);
  };
  settle();
  // Chromium's scroll anchoring may shift the page again at the next layout; settle once more there.
  window.requestAnimationFrame(settle);
});

// SIM-FIX item 1: a review re-render (the same form page plus a review section above the buttons) used to
// open at the top with the new Record button below the fold; bring the action row into view instead.
// Focus is not moved, so Enter does not record.
const REVIEWED = 'fb:reviewed';
document.addEventListener('DOMContentLoaded', () => {
  let flagged = false;
  try { flagged = window.sessionStorage.getItem(REVIEWED) === window.location.pathname; window.sessionStorage.removeItem(REVIEWED); } catch {}
  const record = flagged && document.querySelector('form[method=post] button[name=action][value=record]');
  if (record) record.scrollIntoView({ block: 'center' });
});

// SIM-FIX item 5: report presets and dates agree. Choosing a preset fills From/To with the dates it stands
// for (data-start/data-end, resolved by the host with the report's own clock) before Run; typing other
// dates switches the preset to Custom, so the preset never silently overrides them. A query whose
// start/end disagree with its preset (typed or bookmarked) shows the dates the report used in the address.
document.addEventListener('DOMContentLoaded', () => {
  const preset = document.querySelector('form.report-controls select#report-preset');
  const form = preset && preset.form;
  const start = form && form.querySelector('input[name=start]');
  const end = form && form.querySelector('input[name=end]');
  if (!start || !end) return;
  const range = () => {
    const option = preset.selectedOptions[0];
    return option && option.dataset.end ? [option.dataset.start || '', option.dataset.end] : null;
  };
  preset.addEventListener('change', () => {
    const r = range();
    if (r) [start.value, end.value] = r;
  });
  const typed = () => {
    const r = range();
    if (r && (start.value !== r[0] || end.value !== r[1])) preset.value = 'custom';
  };
  start.addEventListener('change', typed);
  end.addEventListener('change', typed);
  const r = range();
  const url = new URL(window.location.href);
  const last = (key) => { const all = url.searchParams.getAll(key); return all.length ? all[all.length - 1] : null; };
  const dated = url.searchParams.has('start') || url.searchParams.has('end');
  if (!r || !dated || (last('start') === start.value && last('end') === end.value) || start.value !== r[0] || end.value !== r[1]) return;
  const pairs = [];
  let placed = false;
  for (const [key, value] of url.searchParams) {
    if (key !== 'start' && key !== 'end') pairs.push([key, value]);
    else if (!placed) { placed = true; pairs.push(['start', r[0]], ['end', r[1]]); }
  }
  window.history.replaceState(null, '', `${url.pathname}?${new URLSearchParams(pairs)}${url.hash}`);
});

// One bridged submit per form at a time: a second click while the first command is in flight would only
// spend the page's single-use token and report "This form has expired".
const submitting = new WeakSet();

// J-ATTACH: an Attach form never uses Chromium's file chooser or sends file bytes. "Attach a file" and
// Attach both post the form's text fields; the main process then shows the native chooser and Rust reads
// the picked file. (The input's `required` would otherwise block Attach before any submit event.)
const ATTACH_FORM = 'form.attachments-add';
document.addEventListener('click', (event) => {
  const control = event.target.closest && event.target.closest(`${ATTACH_FORM} input[type=file], ${ATTACH_FORM} button[type=submit]`);
  if (!control || event.defaultPrevented || event.button !== 0) return;
  event.preventDefault();
  sendForm(control.form, control.tagName === 'BUTTON' ? control : null);
}, true);

document.addEventListener('submit', (event) => {
  const form = event.target;
  // A payment has a control named "method", which shadows HTMLFormElement.method.
  if (!(form instanceof HTMLFormElement) || (form.getAttribute('method') || 'get').toLowerCase() !== 'post') return;
  event.preventDefault();
  sendForm(form, event.submitter);
}, true);

async function sendForm(form, submitter) {
  if (submitting.has(form)) return;
  // Any other form with a file field: its file part cannot cross the bridge, so nothing is sent.
  if (!form.matches(ATTACH_FORM) && Array.from(new FormData(form).values()).some(v => typeof v !== 'string' && v.size > 0)) {
    window.alert('This form sends a file, which the desktop app cannot do here yet. Nothing was sent.');
    return;
  }
  submitting.add(form);
  const fields = fieldsOf(form, submitter);
  const get = (name) => (fields.find(([k]) => k === name) || [])[1] || '';
  const action = new URL(form.getAttribute('action') || window.location.pathname, window.location.href).pathname;
  const busyKind = form.dataset.gate === 'setup' ? 'setup' : form.dataset.gate === 'restore' ? 'restore' : action === '/settings/book-protection/' && ['add','remove'].includes(get('action')) ? 'protection' : BACKUPS_PAGE.test(action) && !form.dataset.gate ? (action.endsWith('/create/') ? 'create' : get('action')) : '';
  const done = busyKind && !document.querySelector('main[aria-busy=true]') ? backupsBusy(busyKind) : () => {};
  try {
    let result;
    const logo = form.querySelector('#company-logo-file');
    // An invalid form retains a pending upload. Removing the logo also discards that pending choice.
    if (logo && !logo.files.length && get('remove_logo') === 'on') {
      for (let i=fields.length-1;i>=0;i--) if (fields[i][0]==='logo_upload') fields.splice(i,1);
    }
    if (logo && logo.files.length) {
      const file = logo.files[0];
      if (file.size === 0) { logo.value = ''; throw new Error('Choose a PNG or JPEG logo. This file is empty.'); }
      if (file.size > 128 * 1024) { logo.value = ''; throw new Error('Choose a logo up to 128 KB.'); }
      const encoded = await new Promise((resolve,reject) => {
        const reader = new FileReader();
        reader.onload = () => resolve(String(reader.result).split(',')[1]);
        reader.onerror = () => reject(new Error('The logo could not be read. Choose it again.'));
        reader.readAsDataURL(file);
      });
      for (let i=fields.length-1;i>=0;i--) if (fields[i][0]==='logo_upload') fields.splice(i,1);
      fields.push(['logo_upload',encoded]);
    }
    if (form.dataset.gate === 'setup') {
      result = await call('app.createBook', { owner_name: get('owner_name'), company_name: get('company_name'), lock_password: get('lock_password'), lock_password2: get('lock_password2'), protect:get('protect'), starter_chart:get('starter_chart') });
    } else if (form.dataset.gate === 'restore') {
      result = await call('app.restoreBook', {name:get('name'),secret:get('secret')});
    } else if (form.dataset.gate === 'unlock') {
      result = await call('app.unlock', { password: get('password') });
    } else {
      const token = get('fb_command_token');
      result = await call('books.run', { type: 'SubmitForm', path: new URL(form.getAttribute('action') || window.location.pathname, window.location.href).pathname, token, fields: fields.filter(([k]) => k !== 'fb_command_token') });
    }
    if (result.outcome !== 'redirect' && result.outcome !== 'invalid') done();
    if (result.data?.desktop_keys === 'copy-recovery') {
      const token = form.querySelector('input[name=fb_command_token]');
      if (token && result.data.next_token) token.value = result.data.next_token;
      const status = document.getElementById('recovery-copy-status');
      if (status) status.textContent = result.data.copied ? 'Recovery key copied. Paste it somewhere safe.' : result.data.copy_error;
    }
    if (result.outcome === 'invalid' && get('action') === 'review') {
      try { window.sessionStorage.setItem(REVIEWED, new URL(result.reload, window.location.href).pathname); } catch {}
    }
    follow(result);
    // A redirect or re-render is navigating away; the spent token must not be sent again meanwhile.
    if (result.outcome !== 'redirect' && result.outcome !== 'invalid') submitting.delete(form);
  } catch (e) {
    done();
    submitting.delete(form);
    window.alert(String(e.message || e));
  }
}

// Ported pages set the page icon with an inline `style` attribute, which the CSP blocks; apply the same
// custom property through the CSSOM (allowed) so the markup stays identical to Django's.
document.addEventListener('DOMContentLoaded', () => {
  const main = document.querySelector('main.has-page-icon[style]');
  const m = main && /--page-icon:url\('(\/static\/[^')]+)'\)/.exec(main.getAttribute('style') || '');
  if (m) main.style.setProperty('--page-icon', `url('${m[1]}')`);
});

// D7b CSV links use the same typed export/Save dialog as the report wave.
document.addEventListener('click', async event => {
  const anchor = event.target.closest && event.target.closest('a[href]');
  if (!anchor || event.defaultPrevented || event.button !== 0) return;
  const target = new URL(anchor.href, window.location.href);
  if (target.origin !== window.location.origin || target.searchParams.getAll('output').at(-1) !== 'csv') return;
  const bank = /^\/c\/([0-9a-f-]{36})\/accounts\/([0-9a-f-]{36})\/register\/$/.exec(target.pathname);
  const rec = /^\/c\/([0-9a-f-]{36})\/reconciliations\/([0-9a-f-]{36})\/$/.exec(target.pathname);
  if (!bank && !rec) return;
  event.preventDefault();
  try {
    const token = anchor.querySelector('input[name=fb_command_token]').value;
    await call('books.run', {type:'SubmitForm', path:target.pathname, token, fields:Array.from(target.searchParams.entries())});
    window.location.reload();
  } catch (error) { window.alert(String(error.message || error)); }
}, true);

// DESKTOP-BACKUPS: the Backups page keeps Django's Download links; a native Save dialog replaces the
// browser download, and Rust copies the file (typed read-only export, like the CSV links above).
document.addEventListener('click', async event => {
  const anchor = event.target.closest && event.target.closest('a[href]');
  if (!anchor || event.defaultPrevented || event.button !== 0) return;
  const target = new URL(anchor.href, window.location.href);
  const match = /^\/c\/([0-9a-f-]{36})\/backups\/download\/([^/]+)$/.exec(target.pathname);
  if (target.origin !== window.location.origin || !match) return;
  event.preventDefault();
  const done = document.querySelector('main[aria-busy=true]') ? () => {} : backupsBusy('copy');
  try {
    const result = await call('books.run', {type:'ExportBackup', company_id:match[1], name:decodeURIComponent(match[2])});
    if (result.outcome !== 'redirect') done();
    follow(result);
  } catch (error) { done(); window.alert(String(error.message || error)); }
}, true);
// PRINT keeps Django's button and packaged CSS. No renderer API or new IPC command is exposed.
document.addEventListener('click', async event => {
  const button = event.target.closest && event.target.closest('#print-now');
  if (!button || event.defaultPrevented || event.button !== 0) return;
  event.preventDefault();
  const input = button.querySelector('input[name=fb_command_token]');
  if (!input || button.disabled) return;
  button.disabled = true;
  try {
    const result = await call('books.run',{type:'SubmitForm',path:window.location.pathname+window.location.search,token:input.value,fields:[['action','desktop_print']]});
    if (result.outcome === 'done' && result.data.print_token) input.value = result.data.print_token;
    if (result.data && result.data.print_error) window.alert(result.data.print_error);
  } catch (error) { window.alert(String(error.message || error)); }
  finally { button.disabled = false; }
}, true);

document.addEventListener('DOMContentLoaded', () => {
  if (!document.getElementById('print-now')) return;
  // The frozen document's @page margin boxes are inline and CSP blocks their style element.
  // Apply only that trusted template-generated rule through the CSSOM (as with the page icon).
  for (const style of document.head.querySelectorAll('style')) {
    const source = style.textContent.trim();
    if (!source.startsWith('@page {') || !source.endsWith('}')) continue;
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(source);
    document.adoptedStyleSheets = [...document.adoptedStyleSheets,sheet];
  }
});

// Django opens customer prints in a new tab. The hardened app uses the same local window.
document.addEventListener('click', event => {
  const anchor=event.target.closest && event.target.closest('a[href]');
  if (!anchor || event.defaultPrevented || event.button!==0 || anchor.target!=='_blank') return;
  const target=new URL(anchor.href,window.location.href);
  if (target.origin!==window.location.origin || !target.pathname.endsWith('/print/')) return;
  event.preventDefault();window.location.assign(target.href);
},true);


function protectionFields() {
  const checkbox = document.querySelector('#id_protect');
  const fields = document.querySelector('#own-password-fields');
  if (!checkbox || !fields) return;
  fields.hidden = !checkbox.checked;
  for (const input of fields.querySelectorAll('input')) input.disabled = !checkbox.checked;
}
document.addEventListener('DOMContentLoaded', protectionFields);
document.addEventListener('change', event => { if (event.target.id === 'id_protect') protectionFields(); });

// Inline creation retains the other mapping rows in this same form submission.
function newFeedAccountFields() {
  for (const select of document.querySelectorAll('#bankfeed-mapping select[name^="map_"]')) {
    const fields = select.closest('td')?.querySelector('.new-feed-account');
    if (fields) { fields.hidden = select.value !== '__new__'; fields.disabled = fields.hidden; }
  }
}
document.addEventListener('DOMContentLoaded', newFeedAccountFields);
document.addEventListener('change', event => {
  if (event.target.matches('#bankfeed-mapping select[name^="map_"]')) newFeedAccountFields();
});

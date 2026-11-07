'use strict';
// Security guards for the main process (ARCHITECTURE 8, Electron security checklist).
const path = require('node:path');
const fs = require('node:fs');

// No inline scripts or styles, nothing from anywhere but our own scheme, no network at all.
const CSP = [
  "default-src 'none'",
  "script-src 'self'",
  "style-src 'self'",
  "img-src 'self'",
  "font-src 'self'",
  "connect-src 'none'",
  "media-src 'none'",
  "object-src 'none'",
  "frame-src 'none'",
  "worker-src 'none'",
  "frame-ancestors 'none'",
  "base-uri 'none'",
  "form-action 'self'",
].join('; ');

const MIME = { '.css': 'text/css; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.svg': 'image/svg+xml', '.png': 'image/png', '.ico': 'image/x-icon', '.woff2': 'font/woff2' };

/** Serve one bundled static file; no traversal, no directories, only known types. */
function staticFile(root, rel) {
  let decoded;
  try {
    decoded = decodeURIComponent(rel);
  } catch {
    return new Response('Bad request', { status: 400 });
  }
  const full = path.resolve(root, decoded);
  const type = MIME[path.extname(full).toLowerCase()];
  if (!type || !full.startsWith(root + path.sep) || decoded.includes('\0')) return new Response('Not found', { status: 404 });
  let data;
  try {
    const real = fs.realpathSync(full);
    if (!real.startsWith(fs.realpathSync(root) + path.sep) || !fs.statSync(real).isFile()) return new Response('Not found', { status: 404 });
    data = fs.readFileSync(real);
  } catch {
    return new Response('Not found', { status: 404 });
  }
  return new Response(data, { status: 200, headers: { 'content-type': type, 'x-content-type-options': 'nosniff', 'content-security-policy': CSP } });
}

const MAX_BYTES = 256 * 1024;

function plain(payload, what) {
  if (payload === null || typeof payload !== 'object' || Array.isArray(payload) || Object.getPrototypeOf(payload) !== Object.prototype) {
    throw new Error(`Invalid ${what}.`);
  }
  if (JSON.stringify(payload).length > MAX_BYTES) throw new Error(`${what} too large.`);
  return payload;
}

// Command and query type names the shell accepts; fb-app re-validates every field (deny_unknown_fields).
const COMMANDS = new Set(["SetNextNumber", "SaveDirectoryRecord", "SetDirectoryActive", "SaveTaxRate", "SaveTaxGroup", "SaveTaxSettings", "SetTaxItemActive", "FinishTaxBulkStep", "SaveReminderSettings", "ExportHistory", "SaveCompany", "SaveCompanyDetails", "SaveJournal", "PostJournal", "ReverseJournal", "FillAccountingSetup", "DismissSetup", "SaveAccount", "SaveCustomer", "SaveItem", "SaveSetup", "SaveInvoice", "PostInvoice", "SaveInvoiceDraft", "PostReviewedInvoice", "VoidInvoice", "SaveEstimate", "TransitionEstimate", "ConvertEstimate", "ReviewCustomerReceipt", "RecordCustomerReceipt", "ReverseCustomerReceipt", "SubmitForm", "SaveBill", "ReviewBill", "PostBill", "VoidBill", "ReviewBillPayment", "RecordBillPayment", "ReverseBillPayment", "ReviewTransfer", "RecordTransfer", "VoidTransfer", 'SaveCheck', 'ReviewCheck', 'RecordCheck', 'AssignCheckNumber', 'VoidCheck', 'ReviewDeposit', 'RecordDeposit', 'ReverseDeposit']);

// D5: append-only command names for the fixed bridge.
COMMANDS.add('ReviewCreditMemo');
COMMANDS.add('ReviewCustomerDeposit');
COMMANDS.add('ReviewCreditApplication');
COMMANDS.add('ReviewDepositApplication');
COMMANDS.add('ReviewCustomerRefund');
COMMANDS.add('RecordCreditMemo');
COMMANDS.add('RecordCustomerDeposit');
COMMANDS.add('RecordCreditApplication');
COMMANDS.add('RecordDepositApplication');
COMMANDS.add('RecordCustomerRefund');
COMMANDS.add('VoidCreditMemo');
COMMANDS.add('VoidCustomerDeposit');
COMMANDS.add('UnapplyCredit');
COMMANDS.add('UnapplyDeposit');
COMMANDS.add('VoidCustomerRefund');
const QUERIES = new Set(["NumberingSettings", 'Companies', 'Accounts', 'Register', 'Invoices', 'Schema']);

COMMANDS.add("ExportReport");
COMMANDS.add('MarkBankLine');
COMMANDS.add('BeginReconciliation');
COMMANDS.add('SaveReconciliationTicks');
COMMANDS.add('FinishReconciliation');
COMMANDS.add('UndoReconciliation');
COMMANDS.add('ExportBankRegister');
COMMANDS.add('ExportReconciliation');
COMMANDS.add('ExportBackup');

function command(payload) {
  plain(payload, 'command');
  if (!COMMANDS.has(payload.type)) throw new Error('Unknown command.');
  if (payload.type === 'SubmitForm') {
    if (typeof payload.path !== 'string' || !payload.path.startsWith('/') || typeof payload.token !== 'string' || !Array.isArray(payload.fields)) throw new Error('Invalid form.');
    if (!payload.fields.every((f) => Array.isArray(f) && f.length === 2 && typeof f[0] === 'string' && typeof f[1] === 'string')) throw new Error('Invalid form fields.');
  }
  return payload;
}

function query(payload) {
  plain(payload, 'query');
  if (!QUERIES.has(payload.type)) throw new Error('Unknown query.');
  return payload;
}

function target(payload) {
  plain(payload, 'screen request');
  if (typeof payload.target !== 'string' || !payload.target.startsWith('/') || payload.target.startsWith('//') || payload.target.length > 4096) throw new Error('Invalid screen address.');
  return payload.target;
}

function strings(payload, names, what) {
  plain(payload, what);
  const out = {};
  for (const key of Object.keys(payload)) if (!names.includes(key)) throw new Error(`Unknown ${what} field.`);
  for (const name of names) {
    const v = payload[name] ?? '';
    if (typeof v !== 'string' || v.length > 1000) throw new Error(`Invalid ${what}.`);
    out[name] = v;
  }
  return out;
}

const setup = (p) => {
  const input = strings(p, ['owner_name', 'company_name', 'lock_password', 'lock_password2', 'protect', 'starter_chart'], 'setup');
  if (!['standard', 'empty'].includes(input.starter_chart)) throw new Error('Choose a chart of accounts.');
  return input;
};
const unlock = (p) => strings(p, ['password'], 'unlock');
const restoreBook = (p) => strings(p, ['name', 'secret'], 'restore');

/** Only our own window's top frame, on our own scheme, may call. */
function sender(event, win) {
  const frame = event.senderFrame;
  if (!win || event.sender !== win.webContents || !frame || frame !== win.webContents.mainFrame || !String(frame.url).startsWith('fb://app/')) {
    throw new Error('Refused: unexpected sender.');
  }
}

function window(win, origin) {
  const contents = win.webContents;
  // No direct ICE/STUN traffic, including data channels that need no media permission.
  contents.setWebRTCIPHandlingPolicy('disable_non_proxied_udp');
  contents.setWindowOpenHandler(() => ({ action: 'deny' }));
  const allowed = (url) => {
    try {
      const u = new URL(url);
      return u.protocol === 'fb:' && u.host === 'app' && `${u.protocol}//${u.host}` === origin;
    } catch {
      return false;
    }
  };
  contents.on('will-navigate', (event, url) => {
    if (!allowed(url)) event.preventDefault();
  });
  contents.on('will-redirect', (event, url) => {
    if (!allowed(url)) event.preventDefault();
  });
  contents.on('will-attach-webview', (event) => event.preventDefault());
  contents.on('will-frame-navigate', (event) => {
    if (!event.isMainFrame || !allowed(event.url)) event.preventDefault();
  });
}

// What the session guard saw: a count of fb://app/ requests and the first refused addresses (capped).
const requests = { fb: 0, refused: [] };
function session(ses) {
  // No network from the renderer: every request that isn't our own scheme is cancelled.
  ses.webRequest.onBeforeRequest((details, callback) => {
    const ours = String(details.url).startsWith('fb://app/');
    if (ours) requests.fb += 1;
    else if (requests.refused.length < 100) requests.refused.push(String(details.url).slice(0, 300));
    callback({ cancel: !ours });
  });
  ses.setPermissionRequestHandler((_wc, _perm, callback) => callback(false));
  ses.setPermissionCheckHandler(() => false);
  ses.setDevicePermissionHandler(() => false);
  ses.on('will-download', (event) => event.preventDefault());
  ses.setProxy({ mode: 'direct' });
  // Chromium's spellchecker fetches Hunspell dictionaries from Google's CDN on its own network
  // context, outside webRequest. The app never spellchecks (webPreferences.spellcheck is false).
  if (typeof ses.setSpellCheckerEnabled === 'function') ses.setSpellCheckerEnabled(false);
  if (typeof ses.setSpellCheckerLanguages === 'function') ses.setSpellCheckerLanguages([]);
}

module.exports = { restoreBook, CSP, staticFile, command, query, target, setup, unlock, sender, window, session };


// SEC-FIX1: COMMANDS above is the historical app vocabulary, not IPC authorization.
const RENDERER_COMMANDS = new Set(['SubmitForm', 'ExportHistory', 'ExportReport', 'ExportBackup']);
const validateCommand = command;
module.exports.command = (payload, capabilities = {}) => {
  plain(payload, 'command');
  if (!RENDERER_COMMANDS.has(payload.type)) throw new Error('Unknown command.');
  const checked = validateCommand(payload);
  if (checked.type === 'SubmitForm' && !pluginAllowed(checked.path, capabilities)) throw new Error('Unknown command.');
  return checked;
};
// Release builds without plugins: the bank-feed, AI assistant and ChatGPT sign-in addresses.
function pluginPath(pathname) {
  return typeof pathname === 'string' && (/^\/c\/[^/]+\/(bank-feeds|ai)\//.test(pathname) || pathname.startsWith('/auth/openai/'));
}
module.exports.pluginPath = pluginPath;
function pluginAllowed(pathname, {plugins = false, bankFeeds = false} = {}) {
  if (!pluginPath(pathname)) return true;
  return /^\/c\/[^/]+\/bank-feeds\//.test(pathname) ? plugins || bankFeeds : plugins;
}
module.exports.pluginAllowed = pluginAllowed;
module.exports.requestLog = () => ({ fb: requests.fb, refused: [...requests.refused] });
module.exports.rendererCommands = Object.freeze([...RENDERER_COMMANDS]);

// DESKTOP-JODY1: the only outside addresses the app opens, in the system browser, from the main
// process. Exact string match: no parsing, no prefix, no case folding, no other scheme or repository.
const EXTERNAL_URLS = Object.freeze(['https://github.com/CyberJody/FreedomBooks/issues/new/choose', 'https://bridge.simplefin.org/simplefin/create']);
module.exports.EXTERNAL_URLS = EXTERNAL_URLS;
module.exports.external = payload => {
  plain(payload, 'link');
  if (Object.keys(payload).length !== 1 || typeof payload.url !== 'string' || !EXTERNAL_URLS.includes(payload.url)) throw new Error('Refused: link not allowed.');
  return payload.url;
};
module.exports.appCommands = Object.freeze([...COMMANDS]);

// WAVE4: an `Outcome::External` (the ChatGPT sign-in, `signIn`) opens in the system browser only when its
// origin is one fb-node allows (`externalOrigins()`: https://auth.openai.com; a fake-network smoke
// build adds its exact http://127.0.0.1:<port> provider). No credentials in the URL, nothing else.
module.exports.signIn = (raw, origins) => {
  if (typeof raw !== 'string' || raw.length > 8192) throw new Error('Invalid external address.');
  let url;
  try { url = new URL(raw); } catch { throw new Error('Invalid external address.'); }
  const allowed = Array.isArray(origins) && origins.some(origin => typeof origin === 'string' && origin === url.origin
    && (url.protocol === 'https:' || /^http:\/\/127\.0\.0\.1:[1-9][0-9]{0,4}$/.test(origin)));
  if (!allowed || url.username || url.password || url.hash) throw new Error('Invalid external address.');
  return url.href;
};

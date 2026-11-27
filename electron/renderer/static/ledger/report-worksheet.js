'use strict';

const worksheet = document.querySelector('#report-worksheet');
const form = worksheet.form;
const builder = form.querySelector('[data-worksheet-add]');
const scope = builder.querySelector('[data-worksheet-scope]');
const limit = Number(form.dataset.worksheetTargetLimit);
const limitError = form.dataset.worksheetLimitError;

function target(value, canonical = false) {
  const query = new URLSearchParams(new FormData(form));
  // Connected-host preload stamps forms with the session marker and strips it before Rust sees the URL.
  // Keep it on the navigated URL, but exclude it from the canonical Rust length check.
  if (canonical) query.delete('__fb_remote_page');
  if (value) query.set(worksheet.name, value.replace(/\r\n?/g, '\n'));
  else query.delete(worksheet.name);
  const preset = form.elements.preset.selectedOptions[0];
  if (canonical) {
    query.set('preset', 'custom');
    if (preset?.dataset.end) {
      query.set('start', preset.dataset.start);
      query.set('end', preset.dataset.end);
    }
  }
  // Match Rust's urlencode, which keeps tilde and escapes the asterisk.
  const encoded = query.toString().replaceAll('*', '%2A').replaceAll('%7E', '~');
  return new URL(form.action).pathname + '?' + encoded;
}

function fits(value) {
  return target(value, true).length <= limit;
}

const feedback = document.getElementById('worksheet-feedback');
let invalidField;
function clearWarning() {
  if (invalidField) {
    invalidField.setCustomValidity('');
    invalidField.removeAttribute('aria-invalid');
    invalidField = null;
  }
  feedback.textContent = '';
}
form.addEventListener('input', clearWarning);
form.addEventListener('change', clearWarning);

function warn(input, message) {
  clearWarning();
  invalidField = input;
  input.setAttribute('aria-invalid', 'true');
  feedback.textContent = message;
  input.focus();
  input.setCustomValidity(message);
  input.reportValidity();
}

if (scope) {
  scope.addEventListener('change', () => {
    const account = scope.value === 'account';
    builder.querySelector('[data-worksheet-account]').hidden = !account;
    builder.querySelector('[data-worksheet-posted]').hidden = account;
    builder.querySelector('[data-worksheet-account-id]').disabled = !account;
    builder.querySelector('[data-worksheet-posted-id]').disabled = account;
  });
}

builder.querySelector('[data-worksheet-append]').addEventListener('click', () => {
  clearWarning();
  const inputs = Array.from(builder.querySelectorAll('[data-worksheet-field]:not(:disabled)'));
  const fields = inputs.map(input => input.value.trim());
  const invalid = fields.findIndex(value => value.includes('|'));
  if (invalid !== -1) {
    warn(inputs[invalid], 'Remove the vertical bar from this field.');
    return;
  }
  const required = scope ? [1, 2, 4] : [0];
  const missing = required.find(index => !fields[index]);
  if (missing !== undefined) {
    warn(inputs[missing], 'Enter a value for this field.');
    return;
  }
  if (scope && !fields[3] && !['excluded / personal', 'unassigned'].includes(fields[2].toLowerCase())) {
    warn(inputs[3], 'Choose a Schedule C category for this business activity.');
    return;
  }
  const row = fields.join('|');
  const next = worksheet.value.trim() ? worksheet.value.trimEnd() + '\n' + row : row;
  if (!fits(next)) {
    warn(worksheet, limitError);
    return;
  }
  worksheet.value = next;
  feedback.textContent = scope ? 'Mapping added.' : 'Asset added.';
  worksheet.focus();
  worksheet.setSelectionRange(next.length - row.length, next.length);
});

form.addEventListener('submit', event => {
  event.preventDefault();
  if (fits(worksheet.value)) window.location.assign(target(worksheet.value));
  else warn(worksheet, limitError);
});
builder.hidden = false;

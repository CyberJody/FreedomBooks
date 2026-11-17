'use strict';
// The customer page's type filter applies as soon as a type is chosen; the Show button stays for keyboards.
for (const select of document.querySelectorAll('select[data-autosubmit]')) {
  select.addEventListener('change', () => select.form.requestSubmit());
}

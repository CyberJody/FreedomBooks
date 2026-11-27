/* Hybrid UI v1 shell: on narrow screens the navigation is a horizontally scrolling strip; bring the
   current entry into view. Progressive enhancement only — navigation works without it. */
document.addEventListener('DOMContentLoaded', function () {
  var nav = document.querySelector('nav.appnav');
  var current = nav && nav.querySelector('a[aria-current="page"]');
  if (!nav || !current || nav.scrollWidth <= nav.clientWidth) return;
  nav.scrollLeft = Math.max(0, current.offsetLeft - nav.offsetLeft - 8);
});

/* Title-bar menus are <details> disclosures and work without this. It adds what people expect of a menu:
   Escape closes it (focus returns to its button), a click elsewhere closes it, and opening one closes the
   other. */
(function () {
  function menus() { return Array.prototype.slice.call(document.querySelectorAll('details.menu')); }
  document.addEventListener('toggle', function (event) {
    var opened = event.target;
    if (!opened.matches || !opened.matches('details.menu') || !opened.open) return;
    menus().forEach(function (m) { if (m !== opened) m.open = false; });
  }, true);
  document.addEventListener('keydown', function (event) {
    if (event.key !== 'Escape') return;
    menus().forEach(function (m) {
      if (!m.open) return;
      m.open = false;
      var summary = m.querySelector('summary');
      if (summary && m.contains(document.activeElement)) summary.focus();
    });
  });
  document.addEventListener('click', function (event) {
    menus().forEach(function (m) { if (m.open && !m.contains(event.target)) m.open = false; });
  });
  /* Focus leaving a menu (Tab past its last entry) closes it. */
  document.addEventListener('focusout', function (event) {
    var m = event.target.closest && event.target.closest('details.menu');
    if (m && m.open && event.relatedTarget && !m.contains(event.relatedTarget)) m.open = false;
  });
})();

document.addEventListener('change', event => {
  if (event.target.matches('form[data-submit-status] select[name="status"]')) event.target.form.requestSubmit();
});

// Once a transfer has an amount, its empty-field message no longer applies.
document.addEventListener('input', function (event) {
  const input = event.target;
  if (!input.matches('#transfer-entry [name="amount"][aria-invalid="true"]') || !input.value.trim()) return;
  const errors = document.getElementById(input.id + '_error');
  if (!errors || errors.textContent.trim() !== 'This field is required.') return;
  const description = (input.getAttribute('aria-describedby') || '').split(/\s+/).filter(id => id && id !== errors.id);
  if (description.length) input.setAttribute('aria-describedby', description.join(' '));
  else input.removeAttribute('aria-describedby');
  errors.remove();
  input.removeAttribute('aria-invalid');
});

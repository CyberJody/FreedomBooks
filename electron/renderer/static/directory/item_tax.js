"use strict";
// FX1: on a NEW item, Taxable follows the type until the user ticks or clears it:
// services start not taxable, goods start taxable. Existing items are never changed here.
(() => {
  const kind = document.querySelector('select[name="kind"]');
  const taxable = document.querySelector('input[type="checkbox"][name="taxable"]');
  if (!kind || !taxable) return;
  let touched = false;
  taxable.addEventListener("change", () => { touched = true; });
  kind.addEventListener("change", () => {
    if (!touched) taxable.checked = kind.value !== "SERVICE";
  });
})();

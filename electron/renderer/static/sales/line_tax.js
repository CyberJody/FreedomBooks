"use strict";
// FX1: a new line's tax flag defaults from customer x item. An exempt customer means not
// taxable; otherwise the item's Taxable setting applies. Ticking a line by hand makes it the
// user's choice (the hidden tax_auto marker clears) and a customer change leaves it alone.
// Choosing an item deliberately re-defaults that row, like description and rate.
// The server-side rule lives in directory/tax.py (line_taxable_default); keep the two in step.
(() => {
  const lines = document.querySelector("#estimate-lines, #invoice-lines, #sales-lines");
  const catalogNode = document.querySelector("#estimate-catalog, #invoice-catalog, #sales-receipt-catalog");
  const taxNode = document.getElementById("customer-tax");
  const customer = document.querySelector('select[name="customer"]');
  if (!lines || !catalogNode || !taxNode || !customer) return;
  const catalog = JSON.parse(catalogNode.textContent);
  const customers = JSON.parse(taxNode.textContent);
  const note = document.getElementById("customer-tax-note");
  const status = () => customers[customer.value] || null;
  const parts = row => ({
    item: row.querySelector('select[name$="-item"]'),
    flag: row.querySelector('input[type="checkbox"][name$="-taxable"]'),
    auto: row.querySelector('input[name$="-tax_auto"]'),
  });
  const lineDefault = row => {
    const entry = catalog[parts(row).item?.value || ""];
    return !(status()?.exempt) && Boolean(entry && entry.taxable);
  };
  const hasContent = row => Array.from(row.querySelectorAll('select[name$="-item"], textarea[name$="-description"], input[name$="-quantity"], input[name$="-rate"]'))
    .some(field => field.value.trim() !== "");
  const applyDefault = row => {
    const {flag, auto} = parts(row);
    if (!flag || !auto) return;
    flag.checked = lineDefault(row);
    auto.value = "1";
  };
  const showNote = () => {
    if (!note) return;
    const current = status();
    if (current && current.exempt) {
      note.textContent = "Tax-exempt customer" + (current.certificate ? " (certificate " + current.certificate + ")" : "") +
        ". New lines start as not taxable.";
      note.hidden = false;
    } else {
      note.textContent = "";
      note.hidden = true;
    }
  };
  lines.addEventListener("change", event => {
    const row = event.target.closest("fieldset");
    if (!row) return;
    if (event.target.name.endsWith("-item")) applyDefault(row);
    else if (event.target.name.endsWith("-taxable")) {
      // Only a row with content becomes hand-set: a blank extra row must stay blank so it is skipped.
      const {auto} = parts(row);
      if (auto && hasContent(row)) auto.value = "";
    }
  });
  customer.addEventListener("change", () => {
    lines.querySelectorAll("fieldset").forEach(row => {
      if (parts(row).auto?.value === "1") applyDefault(row);
    });
    showNote();
  });
  showNote();
})();

"use strict";
// Bill-only behaviour: an item fills its line, terms fill the due date, and a vendor's defaults
// fill what is still blank. Lines, totals and entries are in payables/purchase-lines.js.
const setValue = (node, value) => {
  if (!node || node.value === String(value)) return;
  node.value = String(value);
  node.dispatchEvent(new Event("change", { bubbles: true }));
};
const catalogNode = document.getElementById("bill-catalog");
const lines = document.getElementById("bill-lines");
if (catalogNode && lines) {
  const catalog = JSON.parse(catalogNode.textContent);
  lines.addEventListener("change", event => {
    const name = event.target.name || "";
    if (name.endsWith("-expense_account") && event.isTrusted !== false) event.target.dataset.touched = "1";
    if (!name.endsWith("-item")) return;
    const item = catalog[event.target.value];
    if (!item) return;
    const row = event.target.closest("tr");
    row.querySelector('[name$="-description"]').value = item.description;
    row.querySelector('[name$="-rate"]').value = item.rate;
    const account = row.querySelector('[name$="-expense_account"]');
    setValue(account, item.expense_account);
    account.dataset.touched = "1";
    row.querySelector('[name$="-rate"]').dispatchEvent(new Event("input", { bubbles: true }));
  });
}

function dueFrom(dateValue, days) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(dateValue)) return "";
  const [y, m, d] = dateValue.split("-").map(Number);
  // UTC calendar components: never convert a local timestamp across timezones.
  const day = new Date(Date.UTC(y, m - 1, d));
  day.setUTCDate(day.getUTCDate() + Number(days));
  return day.toISOString().slice(0, 10);
}

const terms = document.querySelector('[name="terms"]');
const due = document.querySelector('[name="due_date"]');
const billDate = document.querySelector('[name="date"]');
if (terms) terms.addEventListener("change", () => {
  terms.dataset.touched = "1";
  if (due) due.dataset.touched = "1";
  if (terms.value === "" || !billDate) return;
  const filled = dueFrom(billDate.value, terms.value);
  if (filled && due) due.value = filled;
});

if (due) due.addEventListener("change", () => { due.dataset.touched = "1"; });

// Picker hint only. Fills the first empty expense line and, when the due date
// and terms are still blank, the bill's terms. Never overwrites a typed value.
// Does not run on load, so an existing draft is unchanged until the vendor changes;
// a new bill opened for one vendor (?vendor=) applies them as if it had been chosen.
const vendorNode = document.getElementById("vendor-defaults");
const vendorSelect = document.querySelector('[name="vendor"]');
if (vendorNode && vendorSelect) {
  const vendors = JSON.parse(vendorNode.textContent);
  const apply = () => {
    const hint = vendors[vendorSelect.value];
    if (!hint) return;
    if (hint.expense_account) {
      for (const node of document.querySelectorAll('#bill-lines [name$="-expense_account"]')) {
        if (node.closest("tr").hidden) continue;
        if (!node.value && node.dataset.touched !== "1") {
          setValue(node, hint.expense_account);
          break;
        }
      }
    }
    if (hint.terms && terms && due && !due.value && !terms.value && terms.dataset.touched !== "1" && due.dataset.touched !== "1") {
      terms.value = hint.terms;
      const filled = billDate ? dueFrom(billDate.value, hint.terms) : "";
      if (filled) due.value = filled;
    }
  };
  vendorSelect.addEventListener("change", apply);
  const form = document.getElementById("purchase-entry");
  if (form && form.dataset.vendorPreset === "1") apply();
}

"use strict";
const catalogNode = document.getElementById("bill-catalog");
if (catalogNode) {
  const catalog = JSON.parse(catalogNode.textContent);
  const lines = document.getElementById("bill-lines");
  lines.addEventListener("change", event => {
    if (event.target.name.endsWith("-expense_account")) event.target.dataset.touched = "1";
    if (!event.target.name.endsWith("-item")) return;
    const item = catalog[event.target.value];
    if (!item) return;
    const row = event.target.closest("fieldset");
    row.querySelector('[name$="-description"]').value = item.description;
    row.querySelector('[name$="-rate"]').value = item.rate;
    const account = row.querySelector('[name$="-expense_account"]');
    account.value = item.expense_account;
    account.dataset.touched = "1";
  });
  document.getElementById("add-line").addEventListener("click", () => {
    const count = document.getElementById("id_lines-TOTAL_FORMS");
    const index = Number(count.value);
    if (index >= 50) return;
    const holder = document.createElement("div");
    holder.innerHTML = document.getElementById("empty-line").innerHTML.replaceAll("__prefix__", String(index));
    lines.append(holder.firstElementChild);
    count.value = String(index + 1);
    lines.lastElementChild.querySelector("select").focus();
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
// Does not run on load, so an existing draft is unchanged until the vendor changes.
const vendorNode = document.getElementById("vendor-defaults");
const vendorSelect = document.querySelector('[name="vendor"]');
if (vendorNode && vendorSelect) {
  const vendors = JSON.parse(vendorNode.textContent);
  vendorSelect.addEventListener("change", () => {
    const hint = vendors[vendorSelect.value];
    if (!hint) return;
    if (hint.expense_account) {
      for (const node of document.querySelectorAll('#bill-lines [name$="-expense_account"]')) {
        if (!node.value && node.dataset.touched !== "1") {
          node.value = String(hint.expense_account);
          break;
        }
      }
    }
    if (hint.terms && terms && due && !due.value && !terms.value && due.dataset.touched !== "1" && terms.dataset.touched !== "1") {
      terms.value = hint.terms;
      const filled = billDate ? dueFrom(billDate.value, hint.terms) : "";
      if (filled) due.value = filled;
    }
  });
}

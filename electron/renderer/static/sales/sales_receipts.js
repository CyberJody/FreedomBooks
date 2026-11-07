"use strict";
const catalogNode = document.getElementById("sales-receipt-catalog");
if (catalogNode) {
  const catalog = JSON.parse(catalogNode.textContent);
  const lines = document.getElementById("sales-lines");
  lines.addEventListener("change", event => {
    if (!event.target.name.endsWith("-item")) return;
    const item = catalog[event.target.value];
    if (!item) return;
    const row = event.target.closest("fieldset");
    row.querySelector('[name$="-description"]').value = item.description;
    row.querySelector('[name$="-rate"]').value = item.rate;
    row.querySelector('[name$="-income_account"]').value = item.income_account;
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

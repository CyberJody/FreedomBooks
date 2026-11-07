"use strict";
(() => {
  const read = (id) => { const node = document.getElementById(id); return node ? JSON.parse(node.textContent) : null; };
  const catalog = read("item-catalog"), vendors = read("vendor-catalog"), nextNumbers = read("next-numbers");
  const form = document.getElementById("check-entry");
  if (!form) return;
  // FX4: the server-filled suggestion follows a bank change like a JS-filled one; a typed number never does.
  (() => {
    const bank = form.querySelector('[name="bank"]'), number = form.querySelector('[name="check_number"]');
    if (bank && number && nextNumbers && number.value !== "" && number.value === nextNumbers[bank.value]) number.dataset.auto = "1";
  })();

  // Item pick fills description / cost / account on that row only (editable afterwards).
  form.addEventListener("change", (event) => {
    const name = event.target.name || "";
    if (name.endsWith("-item") && catalog) {
      const item = catalog[event.target.value];
      if (!item) return;
      const row = event.target.closest("fieldset");
      row.querySelector('[name$="-description"]').value = item.description;
      row.querySelector('[name$="-rate"]').value = item.rate;
      row.querySelector('[name$="-account"]').value = item.account;
    }
    if (name === "vendor" && vendors) {
      const vendor = vendors[event.target.value];
      const address = form.querySelector('[name="address"]');
      if (vendor && address && !address.value.trim()) address.value = vendor.address;
      // First empty Expenses-tab line only. Never overwrites an account already chosen.
      if (vendor && vendor.expense_account) {
        for (const row of document.querySelectorAll("#expense-lines .tab-expense")) {
          const account = row.querySelector('[name$="-account"]');
          if (account && !account.value) { account.value = String(vendor.expense_account); break; }
        }
      }
    }
    if (name === "bank" && nextNumbers) {
      const number = form.querySelector('[name="check_number"]');
      const later = form.querySelector('[name="print_later"]');
      if (number && !(later && later.checked) && nextNumbers[event.target.value] !== undefined && (number.value === "" || number.dataset.auto === "1")) {
        number.value = nextNumbers[event.target.value]; number.dataset.auto = "1";
      }
    }
    if (name === "print_later") {
      const number = form.querySelector('[name="check_number"]');
      if (event.target.checked) { number.dataset.previous = number.value; number.value = ""; }
      else if (number.dataset.previous) { number.value = number.dataset.previous; }
    }
    if (name === "check_number") { event.target.dataset.auto = "0"; }
  });

  const add = (holderId, tab, legend) => {
    const count = document.getElementById("id_lines-TOTAL_FORMS");
    const index = Number(count.value);
    if (index >= 50) return;
    const holder = document.createElement("div");
    holder.innerHTML = document.getElementById("empty-line").innerHTML.replaceAll("__prefix__", String(index));
    const row = holder.firstElementChild;
    row.classList.add(tab === "ITEM" ? "tab-item" : "tab-expense");
    row.querySelector("legend").textContent = legend;
    row.querySelector('[name$="-tab"]').value = tab;
    row.querySelector('[name$="-quantity"]').value = "1";
    if (tab === "ITEM") { const rate = row.querySelector('[name$="-rate"]'); const label = row.querySelector(`label[for="${rate.id}"]`); if (label) label.textContent = "Cost per unit:"; }
    document.getElementById(holderId).append(row);
    count.value = String(index + 1);
    row.querySelector(tab === "ITEM" ? '[name$="-item"]' : '[name$="-account"]').focus();
  };
  document.getElementById("add-expense-line").addEventListener("click", () => add("expense-lines", "EXPENSE", "Expense line"));
  document.getElementById("add-item-line").addEventListener("click", () => add("item-lines", "ITEM", "Item line"));
})();

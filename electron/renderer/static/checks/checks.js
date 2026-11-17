"use strict";
// Write check only: payee fields, check numbers, item and vendor defaults. Lines, totals and
// entries are in payables/purchase-lines.js.
(() => {
  const read = (id) => { const node = document.getElementById(id); return node ? JSON.parse(node.textContent) : null; };
  const catalog = read("item-catalog"), vendors = read("vendor-catalog"), nextNumbers = read("next-numbers");
  const form = document.getElementById("purchase-entry");
  if (!form) return;
  const setValue = (node, value) => {
    if (!node || node.value === String(value)) return;
    node.value = String(value);
    node.dispatchEvent(new Event("change", { bubbles: true }));
  };
  // FX4: the server-filled suggestion follows a bank change like a JS-filled one; a typed number never does.
  (() => {
    const bank = form.querySelector('[name="bank"]'), number = form.querySelector('[name="check_number"]');
    if (bank && number && nextNumbers && number.value !== "" && number.value === nextNumbers[bank.value]) number.dataset.auto = "1";
  })();

  // Show the vendor list or the payee name box, whichever "Pay to" asks for.
  const payee = () => {
    const chosen = form.querySelector('[name="payee_kind"]:checked');
    const value = chosen ? chosen.value : "VENDOR";
    for (const node of form.querySelectorAll("[data-payee]")) node.hidden = node.dataset.payee !== value;
  };
  payee();

  const vendorDefaults = (vendorId) => {
    const vendor = vendors && vendors[vendorId];
    if (!vendor) return;
    const address = form.querySelector('[name="address"]');
    if (address && !address.value.trim()) address.value = vendor.address;
    // First empty line only. Never overwrites an account already chosen.
    if (vendor.expense_account) {
      for (const row of document.querySelectorAll("#check-lines tr.purchase-line")) {
        if (row.hidden) continue;
        const account = row.querySelector('[name$="-account"]');
        if (account && !account.value) { setValue(account, vendor.expense_account); break; }
      }
    }
  };

  form.addEventListener("change", (event) => {
    const name = event.target.name || "";
    if (name === "payee_kind") payee();
    if (name.endsWith("-item") && catalog) {
      const item = catalog[event.target.value];
      if (!item) return;
      const row = event.target.closest("tr");
      row.querySelector('[name$="-description"]').value = item.description;
      row.querySelector('[name$="-rate"]').value = item.rate;
      setValue(row.querySelector('[name$="-account"]'), item.account);
      row.querySelector('[name$="-rate"]').dispatchEvent(new Event("input", { bubbles: true }));
    }
    if (name === "vendor") vendorDefaults(event.target.value);
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
  if (form.dataset.vendorPreset === "1") {
    const vendor = form.querySelector('[name="vendor"]');
    if (vendor) vendorDefaults(vendor.value);
  }
})();

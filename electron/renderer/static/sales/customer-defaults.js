"use strict";
(() => {
  const field = name => document.querySelector(`[name="${name}"]`);
  const customer = field("customer");
  const hints = document.getElementById("customer-tax");
  const taxDefault = document.getElementById("company-tax-default");
  const customers = hints ? JSON.parse(hints.textContent) : {};
  const companyTax = taxDefault ? JSON.parse(taxDefault.textContent) : "";
  const terms = field("terms"), date = field("date"), due = field("due_date");
  const termsAuto = field("terms_auto");
  const tax = field("tax_mode"), taxAuto = field("tax_header_auto");

  function fillDue() {
    if (!terms || !date || !due || !["0", "15", "30", "60"].includes(terms.value)) return;
    if (!/^\d{4}-\d{2}-\d{2}$/.test(date.value) || date.value.startsWith("0000-")) return;
    const [year, month, day] = date.value.split("-").map(Number);
    const result = new Date(0);
    result.setUTCFullYear(year, month - 1, day);
    if (result.toISOString().slice(0, 10) !== date.value) return;
    result.setUTCDate(result.getUTCDate() + Number(terms.value));
    if (result.getUTCFullYear() <= 9999) due.value = result.toISOString().slice(0, 10);
  }

  if (terms && date && due) {
    terms.addEventListener("change", () => {
      if (termsAuto) termsAuto.value = "";
      fillDue();
    });
    for (const event of ["input", "change"]) {
      date.addEventListener(event, fillDue);
      due.addEventListener(event, () => {
        terms.value = "";
        if (termsAuto) termsAuto.value = "";
      });
    }
  }
  if (tax && taxAuto) tax.addEventListener("change", () => { taxAuto.value = ""; });

  function applyCustomer() {
    const hint = customers[customer.value];
    if (terms && termsAuto?.value === "1") {
      terms.value = hint?.terms ?? "30";
      fillDue();
    }
    if (tax && taxAuto?.value === "1") {
      const choice = hint?.exempt ? "NONE" : companyTax;
      if (Array.from(tax.options).some(option => option.value === choice)) tax.value = choice;
    }
  }

  if (customer && hints) {
    customer.addEventListener("change", applyCustomer);
    document.querySelector("[data-use-customer-defaults]")?.addEventListener("click", () => {
      if (taxAuto) taxAuto.value = "1";
      if (termsAuto) termsAuto.value = "1";
      customer.dispatchEvent(new Event("change", { bubbles: true }));
    });
  }
})();

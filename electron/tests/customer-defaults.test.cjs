const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");

const defaults = fs.readFileSync(require.resolve("../renderer/static/sales/customer-defaults.js"), "utf8");
const lineTax = fs.readFileSync(require.resolve("../renderer/static/sales/line_tax.js"), "utf8");

function editor({ companyTax = "Trate", automatic = true, paymentTerms = true } = {}) {
  const fields = Object.fromEntries(Object.entries({
    customer: "taxable", date: "2028-02-29", tax_mode: companyTax,
    tax_header_auto: automatic ? "1" : "",
    ...(paymentTerms ? { terms: "15", due_date: "2028-03-15", terms_auto: automatic ? "1" : "" } : {}),
  }).map(([name, value]) => [name, Object.assign(new EventTarget(), { value })]));
  fields.tax_mode.options = [...new Set(["", "NONE", "Trate", "Tmanual", companyTax])].map(value => ({ value }));
  const catalog = { taxable: { taxable: true }, untaxed: { taxable: false } };
  const customerHints = { taxable: { terms: "15" }, exempt: { exempt: true, terms: "0", certificate: "RES 81" }, noTerms: {} };
  const node = value => ({ textContent: JSON.stringify(value) });
  const note = { hidden: true };
  const useDefaults = new EventTarget();
  const row = (item, checked, auto) => ({
    item: { value: item },
    flag: { checked },
    auto: { value: auto },
    querySelector(selector) {
      return selector.includes("tax_auto") ? this.auto : selector.includes("taxable") ? this.flag : this.item;
    },
    querySelectorAll() { return [this.item]; },
  });
  const rows = [row("taxable", true, "1"), row("untaxed", false, "1"), row("taxable", true, "")];
  const changeListeners = [];
  const lines = {
    addEventListener(event, fn) { if (event === "change") changeListeners.push(fn); },
    querySelectorAll() { return rows; },
  };
  const document = {
    getElementById(id) { return { "customer-tax": node(customerHints), "company-tax-default": node(companyTax), "customer-tax-note": note }[id]; },
    querySelector(selector) {
      if (selector.startsWith("#estimate-lines")) return lines;
      if (selector.startsWith("#estimate-catalog")) return node(catalog);
      if (selector === "[data-use-customer-defaults]") return useDefaults;
      return fields[selector.split('"')[1]];
    },
  };
  const context = { document, Event };
  vm.runInNewContext(defaults, context);
  vm.runInNewContext(lineTax, context);
  const change = (name, value, event = "change") => {
    fields[name].value = value;
    fields[name].dispatchEvent(new Event(event));
  };
  const itemSelected = (row, value) => {
    row.item.value = value;
    changeListeners.forEach(fn => fn({ target: { name: "lines-0-item", closest: () => row } }));
  };
  return { fields, change, note, useDefaults, rows, itemSelected };
}

test("customer switches update automatic terms, header tax and item tax through leap day", () => {
  const { fields, change, note, rows } = editor();
  change("customer", "exempt");
  assert.equal(fields.terms.value, "0");
  assert.equal(fields.due_date.value, "2028-02-29");
  assert.equal(fields.tax_mode.value, "NONE");
  assert.deepEqual(rows.map(row => row.flag.checked), [false, false, true]);
  assert.match(note.textContent, /RES 81/);
  change("customer", "taxable");
  assert.equal(fields.due_date.value, "2028-03-15");
  assert.equal(fields.tax_mode.value, "Trate");
  assert.deepEqual(rows.map(row => row.flag.checked), [true, false, true]);
  assert.equal(note.hidden, true);
  change("customer", "noTerms");
  assert.equal(fields.terms.value, "30");
  assert.equal(fields.due_date.value, "2028-03-30");
});

test("explicit or saved dates, rates and line overrides survive a customer change", () => {
  const { fields, change, rows, itemSelected, useDefaults } = editor({ automatic: false });
  change("customer", "exempt");
  assert.equal(fields.terms.value, "15");
  assert.equal(fields.tax_mode.value, "Trate");
  change("terms", "60");
  change("tax_mode", "Tmanual");
  change("date", "2028-03-02", "input");
  assert.equal(fields.due_date.value, "2028-05-01");
  change("due_date", "2028-05-20", "input");
  change("customer", "taxable");
  assert.equal(fields.terms.value, "");
  assert.equal(fields.due_date.value, "2028-05-20");
  assert.equal(fields.tax_mode.value, "Tmanual");
  change("customer", "exempt");
  assert.equal(rows[2].flag.checked, true);
  itemSelected(rows[2], "taxable");
  assert.equal(rows[2].flag.checked, false);
  useDefaults.dispatchEvent(new Event("click"));
  assert.equal(fields.tax_mode.value, "NONE");
  assert.equal(fields.due_date.value, "2028-03-02");
});

test("estimated and paid forms can default tax without a terms field, including no rate and tax off", () => {
  for (const companyTax of ["", "NONE"]) {
    const { fields, change } = editor({ companyTax, paymentTerms: false });
    change("customer", "exempt");
    assert.equal(fields.tax_mode.value, "NONE");
    change("customer", "taxable");
    assert.equal(fields.tax_mode.value, companyTax);
    assert.equal(fields.due_date, undefined);
  }
});

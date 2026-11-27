const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");

test("bill date follows selected terms and preserves an explicit due date", () => {
  const fields = {};
  for (const name of ["terms", "due_date", "date"]) {
    fields[name] = Object.assign(new EventTarget(), { value: "", dataset: {} });
  }
  const document = {
    getElementById: () => null,
    querySelector: selector => fields[selector.split('"')[1]],
  };
  vm.runInNewContext(fs.readFileSync(require.resolve("../renderer/static/payables/bills/bills.js"), "utf8"), { document });
  function change(name, value) {
    fields[name].value = value;
    fields[name].dispatchEvent(new Event("change"));
  }
  change("date", "2025-07-01");
  change("terms", "30");
  change("date", "2025-08-11");
  assert.equal(fields.due_date.value, "2025-09-10");
  change("due_date", "2025-09-15");
  change("date", "2025-08-12");
  assert.equal(fields.due_date.value, "2025-09-15");
  change("terms", "0");
  assert.equal(fields.due_date.value, "2025-08-12");
});

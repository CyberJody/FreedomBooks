"use strict";
// Bills list: "Pay selected" opens Pay bills with the ticked bills (bill=<id> for each).
(() => {
  const form = document.getElementById("pay-selected");
  if (!form) return;
  const status = document.getElementById("pay-selected-count");
  const ticked = () => form.querySelectorAll('input[name="bill"]:checked').length;
  const update = () => {
    const n = ticked();
    status.textContent = n ? `${n} bill${n === 1 ? "" : "s"} selected.` : "Tick the bills to pay, or use Pay on a row.";
  };
  form.addEventListener("change", update);
  form.addEventListener("submit", (event) => {
    if (ticked() === 0) {
      event.preventDefault();
      status.textContent = "Tick at least one bill to pay.";
    }
  });
})();

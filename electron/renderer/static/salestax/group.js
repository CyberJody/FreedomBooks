"use strict";
// FX11: live group rate on the tax group form (display only; the server computes the saved rate).
(() => {
  const node = document.getElementById("tax-rates");
  const out = document.getElementById("group-rate");
  if (!node || !out) return;
  const rates = JSON.parse(node.textContent);
  const boxes = document.querySelectorAll('input[type="checkbox"][name="members"]');
  // Sum in ten-thousandths of a percent (rates have at most 4 decimals): no float drift.
  const units = text => {
    const [whole, frac = ""] = String(text).split(".");
    return Number(whole) * 10000 + Number((frac + "0000").slice(0, 4));
  };
  const show = total => {
    const whole = Math.floor(total / 10000);
    const frac = String(total % 10000).padStart(4, "0").replace(/0+$/, "");
    return whole + (frac ? "." + frac : "") + "%";
  };
  const update = () => {
    let total = 0;
    boxes.forEach(box => { if (box.checked && rates[box.value] !== undefined) total += units(rates[box.value]); });
    out.textContent = show(total);
  };
  boxes.forEach(box => box.addEventListener("change", update));
  update();
})();

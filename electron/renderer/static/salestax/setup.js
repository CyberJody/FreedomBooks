'use strict';
(() => {
 for (const form of document.querySelectorAll('[data-tax-setup]')) {
  const show=()=>{
   const collect=form.querySelector('[name="charge_tax"]').value==='yes';
   form.querySelector('[data-collect-tax]').hidden=!collect;
   const rate=form.querySelector('[name="default_item"]');
   rate.closest('.document-field').hidden=rate.options.length===1;
   form.querySelector('[data-new-tax-rate]').hidden=!!rate.value;
  };
  form.addEventListener('change',show);show();
 }
})();

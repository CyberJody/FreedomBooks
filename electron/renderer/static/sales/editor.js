'use strict';
(() => {
  const fixed=(value,scale)=>{const m=/^\+?(\d*)(?:\.(\d*))?$/.exec(value.trim());if(!m||!(m[1]||m[2]))return null;const whole=m[1]||'0',fraction=(m[2]||'').replace(/0+$/,'');if(whole.length>16||fraction.length>scale)return null;return BigInt(whole)*10n**BigInt(scale)+BigInt(fraction.padEnd(scale,'0'));};
  const money=value=>{const sign=value<0n?'-':'',n=value<0n?-value:value;return `${sign}${(n/100n).toLocaleString('en-US')}.${String(n%100n).padStart(2,'0')}`;};
  const lineAmount=(quantity,rate,kind='CHARGE')=>{
    const q=fixed(quantity,4),r=fixed(rate,2);
    if(q===null||r===null||!['CHARGE','DISCOUNT','NOTE'].includes(kind))return null;
    if(kind==='NOTE')return q===0n&&r===0n?0n:null;
    if(q<=0n)return null;
    const n=(q*r+5000n)/10000n;
    return n>9999999999999999n?null:kind==='DISCOUNT'?-n:n;
  };
  const invoiceTax=(taxable,rates)=>{
    if(taxable<0n||!Array.isArray(rates))return null;
    let tax=0n;
    for(const rate of rates){const r=typeof rate==='string'?fixed(rate,4):null;if(r===null||r>1000000n)return null;tax+=(taxable*r+500000n)/1000000n;}
    return tax;
  };
  if(typeof document==='undefined'){module.exports={fixed,money,lineAmount,invoiceTax};return;}
  const editor=document.querySelector('.document-editor');if(!editor)return;
  const form=editor.querySelector('form'),lines=editor.querySelector('#invoice-lines, #estimate-lines');
  const update=()=>{
    let sum=0n,taxable=0n,valid=true;
    for(const row of lines.querySelectorAll('fieldset')){
      const read=s=>row.querySelector(`[name$="-${s}"]`);
      const output=row.querySelector('[data-line-amount]');if(!output)continue;
      const empty=['item','description','quantity','rate'].every(k=>!read(k)?.value.trim());
      if(empty||read('DELETE')?.checked){output.textContent=empty?'—':'Removed';continue;}
      const kind=read('line_kind')?.value||'CHARGE';
      const amount=lineAmount(read('quantity')?.value||'',read('rate')?.value||'',kind);
      if(amount===null){output.textContent='Calculated on save';valid=false;continue;}
      sum+=amount;if(read('taxable')?.checked)taxable+=amount;output.textContent=money(amount);
    }
    valid=valid&&taxable>=0n&&sum-taxable>=0n&&sum<=9999999999999999n;
    editor.querySelector('[data-subtotal]').textContent=valid?money(sum):'Calculated on save';
    const rates=document.getElementById('invoice-tax-rates');
    if(rates){const tax=valid?invoiceTax(taxable,JSON.parse(rates.textContent)[form.querySelector('[name="tax_mode"]').value]):null;
      editor.querySelector('[data-tax]').textContent=tax===null?'Calculated on save':money(tax);
      editor.querySelector('[data-total]').textContent=tax===null||sum+tax>9999999999999999n?'Calculated on save':money(sum+tax);
    }
    // The sticky action bar repeats the total so it stays in view while lines are entered.
    const bar=editor.querySelector('[data-total-bar]'),total=editor.querySelector('[data-total]');
    if(bar&&total)bar.textContent=total.textContent;
  };
  form.addEventListener('input',update);form.addEventListener('change',event=>{
    if(event.target.name?.endsWith('-line_kind')){
      const row=event.target.closest('fieldset'),read=k=>row.querySelector(`[name$="-${k}"]`);
      if(event.target.value==='NOTE'){
        for(const k of ['item','income_account']){if(read(k))read(k).value='';}
        read('quantity').value='0';read('rate').value='0';read('taxable').checked=false;
      }else if(read('quantity').value==='0'){read('quantity').value='1';}
    }
    update();
  });
  new MutationObserver(update).observe(lines,{childList:true});update();
  const keepFocusVisible=()=>requestAnimationFrame(()=>{
    const field=document.activeElement;if(!form.contains(field)||!field.matches('input,select,textarea,summary'))return;
    const rect=field.getBoundingClientRect(),bar=editor.querySelector('.editor-actions').getBoundingClientRect();
    if(rect.bottom>bar.top-10&&rect.top<bar.bottom)field.scrollIntoView({block:'center'});
  });
  form.addEventListener('focusin',keepFocusVisible);window.addEventListener('resize',keepFocusVisible);
  form.addEventListener('keydown',event=>{
    if((event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==='s'){event.preventDefault();form.requestSubmit(form.querySelector('[name="editor_action"][value="save"]'));}
    if(event.altKey&&event.key==='Enter'){event.preventDefault();editor.querySelector('#add-line').click();}
    if(event.altKey&&['ArrowUp','ArrowDown'].includes(event.key)){
      const row=event.target.closest('fieldset'),name=event.target.name?.replace(/^lines-\d+-/,'');if(!row||!name)return;
      const rows=[...lines.querySelectorAll('fieldset')],target=rows[rows.indexOf(row)+(event.key==='ArrowDown'?1:-1)];
      const next=target?.querySelector(`[name$="-${name}"]`);if(next){event.preventDefault();next.focus();}
    }
  });
})();

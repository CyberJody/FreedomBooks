"""Remove the invented credit-print surface; retain only frozen-Django print routes."""
from pathlib import Path
import json
import subprocess
ROOT=Path(__file__).resolve().parents[1]
def edit(name,old,new):
    p=ROOT/name;text=p.read_text();assert text.count(old)==1,(name,old,text.count(old));p.write_text(text.replace(old,new))

edit('crates/fb-app/src/screens/printing.rs','\n            | "credits:print"','')
p=ROOT/'crates/fb-app/src/screens/printing.rs';text=p.read_text()
start=text.index('    } else {\n        let root = super::d5::records::source(app, c, &r.args["credit_memo_id"]')
end=text.index('    };',start)
text=text[:start]+text[end:]
text=text.replace('    } else if r.name == "sales_receipts:print" {','    } else {')
text=text.replace('matches!(kind, "sales_receipt" | "credit_memo")','kind == "sales_receipt"')
old='''    } else {
        doc["posting_note"] = json!(if state == "Voided" {
            "Voided — the original posting is cancelled by its linked reversal."
        } else {
            "Credit on account — not an invoice. No amount due."
        });
'''
assert text.count(old)==1;text=text.replace(old,'')
# The removed credit branch and its preceding receipt branch shared a closing brace.
text=text.replace('            1,\n            1,\n        )\n    };','            1,\n            1,\n        )\n    };')
p.write_text(text)
p=ROOT/'crates/fb-templates/urls.json';routes=json.loads(p.read_text());assert routes.pop('credits:print');p.write_text(json.dumps(routes,indent=1,sort_keys=True)+'\n')
p=ROOT/'crates/fb-screendiff/screens.json';screens=json.loads(p.read_text())
removed=[key for key,value in screens.items() if '/credit-memos/' in value.get('path','') and value['path'].endswith('/print/')]
assert set(removed)=={'print-d5-credit-detail','print-d5-credit-voided','print-d5-credit-fully-used'}
for key in removed:del screens[key]
p.write_text(json.dumps(screens,indent=2)+'\n')
edit('crates/fb-screendiff/py/render_django.py',"        elif '/credit-memos/' in path and path.endswith('/print/'):\n            from print_credit import render_credit\n            response=render_credit(client,path)\n",'')
(ROOT/'crates/fb-screendiff/py/print_credit.py').unlink()
edit('electron/main/printing.cjs','|customer-deposits|credit-memos)','|customer-deposits)')
edit('electron/tests/printing.test.cjs',"const credit=fixture('/c/0195b4a4-55af-719e-96cb-a2f946b067bf/credit-memos/0195b4a4-55af-719e-96cb-a2f946b067c0/');\n  await credit.controller.perform(credit.win,'pdf');assert.match(credit.state.url,/credit-memos\\/[^/]+\\/print\\/$/);\n  assert.equal(credit.state.calls[0],'navigate');assert.equal(credit.state.saved.length,1);", "const invoice=fixture('/c/0195b4a4-55af-719e-96cb-a2f946b067bf/invoices/0195b4a4-55af-719e-96cb-a2f946b067c0/');\n  await invoice.controller.perform(invoice.win,'pdf');assert.match(invoice.state.url,/invoices\\/[^/]+\\/print\\/$/);\n  assert.equal(invoice.state.calls[0],'navigate');assert.equal(invoice.state.saved.length,1);")
edit('electron/tests/printing.test.cjs', "['locked','normal','external']", "['locked','normal','external','credit-memo']")
edit('electron/tests/printing.test.cjs', "    if(variant==='normal')state.url='fb://app/';", "    if(variant==='normal')state.url='fb://app/';\n    if(variant==='credit-memo')state.url='fb://app/c/0195b4a4-55af-719e-96cb-a2f946b067bf/credit-memos/0195b4a4-55af-719e-96cb-a2f946b067c0/';")
# Keep regeneration tools from reintroducing the removed feature.
p=ROOT/'tools/print_hand_fixes.py';text=p.read_text();start=text.index("    if name == 'sales/print/document':");end=text.index('    return text',start);p.write_text(text[:start]+text[end:])
subprocess.run(['python3','tools/print_hand_fixes.py'],cwd=ROOT,check=True)
p=ROOT/'tools/print_wire.py';text=p.read_text();start=text.index("p=ROOT/'crates/fb-templates/urls.json'");end=text.index("print('PRINT:",start);p.write_text(text[:start]+text[end:])
for name in ['tools/print_menu_routes.py','tools/print_diff.py']:
    p=ROOT/name;text=p.read_text().replace('|customer-deposits|credit-memos)','|customer-deposits)').replace('including credit memos','for the existing Django print routes');p.write_text(text)
p=ROOT/'tools/print_reference_wire.py';text=p.read_text();text='\n'.join(line for line in text.splitlines() if "from print_credit import render_credit" not in line)+'\n';text=text.replace('reference credit context and isolated gate resources','isolated gate resources');p.write_text(text)
(ROOT/'tools/print_credit_label.py').unlink()
old='''- Credit-memo print has no frozen Django URL. Its reference-tooling adapter renders the existing
  Django sales print template directly from the immutable Django credit-memo record, with the same
  added credit-detail back-link branch as the desktop hand-fix script. This is new print-kind coverage,
  not normalization of a differing existing Python service. See `PRINT.md`.
'''
for name in ['crates/fb-screendiff/NORMALIZATION.md','tools/print_docs.py']:edit(name,old,'')
p=ROOT/'PRINT.md';text=p.read_text();start=text.index('The frozen Python app has no credit-memo print URL.');end=text.index('\nNo accounting service',start)
text=text[:start]+'''The frozen Python app has no credit-memo print URL, so the desktop offers none either (lead decision,
2026-11-02). The credit memo screen stays exactly as Django renders it; the File print actions refuse
it. The invented route, context adapter and three synthetic print cases have been removed. Credit-memo
printing is a **post-launch candidate**, to be designed only after approval. The print inventory now
has 110 full print cases and 622 total screen comparisons.
'''+text[end:];p.write_text(text)
print('Removed credit-print route, File dispatch, adapter, 3 cases and regeneration hooks')

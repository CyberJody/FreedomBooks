from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
p=ROOT/'crates/fb-screendiff/src/normalize.rs';text=p.read_text();old='    let html=printing::normalize(html);';assert text.count(old)==1
p.write_text(text.replace(old,'    let html=printing::normalize(html,ids);'))
p=ROOT/'crates/fb-screendiff/NORMALIZATION.md'
p.write_text(p.read_text()+'''
- Reconciliation print headers contain the same stored completion/undo instants already normalized
  in reconciliation screen/history. Rewrite only a known `reconciliation_stamp` from that side's
  database, in the completed/undone field of a `rec-paper` report header; preserve dates in reasons,
  other body text and changed/unknown instants. A must-DIFF regression exercises a changed minute.
- The frozen sales-receipt print toolbar incorrectly links `invoices:detail` with the receipt's PK.
  In that exact generated back-link of a SALES RECEIPT print only, map the argument as a receipt,
  preserving the broken route and its label. Numeric Django PK collisions must not map it to an
  unrelated invoice. Must-DIFF tests preserve changed receipt identities and unrelated anchors.
''')
p=ROOT/'PRINT.md'
p.write_text(p.read_text()+'''

Inherited reference defect: sales-receipt prints' back link is labelled `Back to sales_receipt` and
uses the invoice-detail route with the receipt ID. PRINT retains it to meet the required Django
parity. The File print actions work independently of that link. A follow-up can correct the link and
its wording on both sides after the lead authorizes the reference change.
''')
print('PRINT: added only model/context-specific receipt identity and stored reconciliation-time normalization')

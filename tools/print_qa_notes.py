"""Retain the visual QA and clarify exactly which generated timestamps normalize."""
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
old='''  defaults. No printed date, document number, balance, tax, total or footer is normalized away.'''
new='''  defaults. Print-generation instants, document dates/numbers, balances, taxes, totals and footers
  are compared exactly. Stored completion/undo instants use the scoped rule below.'''
for name in ['crates/fb-screendiff/NORMALIZATION.md','tools/print_docs.py']:
    p=ROOT/name;text=p.read_text();assert text.count(old)==1,(name,old);p.write_text(text.replace(old,new))
p=ROOT/'PRINT.md'
p.write_text(p.read_text()+'''

## Visual verification

Inspected the first and bottom 1440×1000 captures of all 113 full print cases (the pre-existing toolbar
fragment is counted separately), using the 26 labelled contact sheets in `staging/print/review/`.
Also inspected the invoice screen, credit memo, dense register and undone reconciliation at full size.
All columns and totals remain visible. Register/reconciliation text wraps tightly in the same screen
sheet width as Django; no styles were redesigned to conceal that inherited layout.

Inspected all five pages of the 40-line invoice PDF: rows remain whole, table headings repeat,
the last-page subtotal/tax/total stay together, and each page has its document/revision/footer/page count.
Also inspected page 1 of the invoice and P&L PDFs, and the landscape register PDF. The register's
ten columns fit on landscape Letter; long names, memo and number wrap within their cells. Its PDF
test checks cell glyph order with raw text extraction as well as retaining spatial text for review.

Native evidence: invoice `INV-000001`, total/balance 180.00; P&L net income 180.00; five-page draft
`INV-000003`, 40 lines and total 1,000.00; landscape register `PRINT-REGISTER-1`, ending balance 1234.56.
Each PDF opens with Poppler and has a page-1 PNG under `staging/print/`; all five long-invoice pages
are rendered. No physical printer was used: the smoke verifies main-process native-print dispatch
with its printer callback stub, while `printToPDF` runs for real.
''')
print('PRINT: retained visual inspection scope and exact normalization limits')

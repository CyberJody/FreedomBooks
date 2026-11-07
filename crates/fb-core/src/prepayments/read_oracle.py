"""Read actual migrated PostgreSQL guard bodies, in a read-only transaction."""
from pathlib import Path
import psycopg
names = ['fb_customer_deposit_guard','fb_customer_deposit_reversal','fb_deposit_application_guard',
         'fb_deposit_application_complete','fb_deposit_unapplication_guard','fb_deposit_refund_guard',
         'fb_deposit_refund_complete','fb_deposit_refund_void_guard','fb_prepayment_journal_owner',
         'fb_prepayment_reference_insert','fb_invoice_live_deposits','fb_invoice_live_settled']
with psycopg.connect('dbname=fbv2_l3d_ref') as conn:
    conn.execute('SET TRANSACTION READ ONLY')
    rows = conn.execute('SELECT proname,prosrc FROM pg_proc WHERE proname=ANY(%s) ORDER BY proname', (names,)).fetchall()
    target = Path(__file__).with_name('oracle-guards.txt')
    text = '\n\n'.join(name+'\n'+body for name,body in rows)
    target.write_text('\n'.join(line.rstrip() for line in text.splitlines())+'\n')
    print(f'Read {len(rows)} live pg_proc.prosrc bodies into {target.name}')

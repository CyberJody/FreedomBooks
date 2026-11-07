"""Execute the frozen service validators, not a reimplementation, for native vectors."""
import os,sys,json
sys.path.insert(0,os.environ['FB_REFERENCE'])
import django
django.setup()
from sales.invoice_services import _date
from django.core.exceptions import ValidationError
values=['2026-10-30','20261030','2026-W44-5','2026W445','2026-W44','2026W44','2026-W445','2026W44-5','2026--W44-5','2026-1-1','2026-10-1','2026-10-30\n',' 2026-10-30','2026-02-30','0001-01-01','9999-12-31','0000-01-01','2026-W00-1','2026-W54-1','2026-W44-0','2026-W44-8','２０２６-１０-３０','',None,1,True,1.2]
rows=[]
for value in values:
    try:rows.append(dict(input=value,output=str(_date(value))))
    except ValidationError as exc:rows.append(dict(input=value,error='; '.join(exc.messages)))
from pathlib import Path
out=Path(__file__).resolve().parents[2]/'fb-core/src/sales/date_vectors.json'
out.write_text(json.dumps(rows,ensure_ascii=False,indent=1)+'\n')
print('Executed',len(rows),'Python sales date vectors:',out)

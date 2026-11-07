"""Credit memo lines have an exact frozen-service key set, including the optional item."""
from pathlib import Path
import json
p=Path('crates/fb-screendiff/scenarios/screens-d3.json');s=json.loads(p.read_text())
for row in s['steps']:
    if row['op'] in ['credits_review_memo','credits_record_memo']:
        for line in row['values']['lines']:line['item_id']=None
p.write_text(json.dumps(s,indent=2)+'\n')

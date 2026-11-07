"""Generate the frozen audit action/type vocabulary; these are product labels, not new copy."""
from pathlib import Path
import ast
import json

source=Path('/mnt/Data/Projects/FreedomBooks/wt/reference/ledger/history.py')
module=ast.parse(source.read_text())
metadata={}
for statement in module.body:
    if isinstance(statement,ast.Assign):
        for target in statement.targets:
            if isinstance(target,ast.Name) and target.id in ['ACTION_LABELS','DOCUMENT_TYPES','TYPE_NOUN','NAMED_TARGETS']:
                metadata[target.id]=ast.literal_eval(statement.value)
Path('crates/fb-app/src/screens/d2').mkdir(exist_ok=True)
Path('crates/fb-app/src/screens/d2/history_labels.json').write_text(json.dumps(metadata,indent=2)+'\n')

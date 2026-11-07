"""A retained snapshot lookup must not move Django's original field validation order."""
from pathlib import Path
p=Path(__file__).resolve().parents[1]/'crates/fb-core/src/bankfeeds/rules.rs'
t=p.read_text()
old='''                .optional()?.ok_or_else(||err("Choose a rule of this company."))
        }).transpose()?;'''
new='''                .optional()
        }).transpose()?.flatten();'''
assert t.count(old)==1
p.write_text(t.replace(old,new))

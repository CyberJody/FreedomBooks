"""Django's company list uses creation order; D2's app query sorted names instead."""
from pathlib import Path
p=Path('crates/fb-app/src/session.rs');s=p.read_text();old='WHERE m.user_id=?1 ORDER BY c.name, c.id",';assert old in s;p.write_text(s.replace(old,'WHERE m.user_id=?1 ORDER BY c.id",'))

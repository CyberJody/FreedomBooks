"""A tax list renders up to 500 action forms; keep its whole bounded token batch alive."""
from pathlib import Path
p=Path('crates/fb-app/src/session.rs');s=p.read_text()
replacements=[
('use std::collections::HashMap;','use std::collections::{HashMap, VecDeque};'),
('    by_token: HashMap<String, String>,','    by_token: HashMap<String, String>,\n    order: VecDeque<String>,'),
('const MAX_LIVE_TOKENS: usize = 64;','const MAX_LIVE_TOKENS: usize = 1024;'),
('''        if self.by_token.len() >= MAX_LIVE_TOKENS {
            self.by_token.clear();
        }''','''        if self.order.len() >= MAX_LIVE_TOKENS && let Some(oldest)=self.order.pop_front() {
            self.by_token.remove(&oldest);
        }'''),
('        self.by_token.insert(token.clone(), path.to_owned());','        self.order.push_back(token.clone());\n        self.by_token.insert(token.clone(), path.to_owned());'),
('        self.by_token.clear();\n    }','        self.by_token.clear();\n        self.order.clear();\n    }'),
]
for old,new in replacements:assert old in s,old;s=s.replace(old,new)
p.write_text(s)

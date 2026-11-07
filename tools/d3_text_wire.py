"""Wire the frozen Django truncator instead of a character-slicing approximation."""
from pathlib import Path
p=Path('crates/fb-templates/src/lib.rs');s=p.read_text();old='mod d3;';assert old in s;s=s.replace(old,old+'\nmod d3_text;');p.write_text(s)
p=Path('crates/fb-templates/src/d3.rs');s=p.read_text();a=s.index('    env.add_filter("truncatechars",');b=s.index('    env.add_filter("linebreaksbr",',a);s=s[:a]+'''    env.add_filter("truncatechars", |v: Value, n: usize| crate::d3_text::truncate(&text(&v),n));
'''+s[b:];p.write_text(s)

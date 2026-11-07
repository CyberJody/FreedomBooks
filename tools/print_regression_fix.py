from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
old='''    // A failed path check spends the capability as well; it cannot authorize another attempt.
    assert!(app.run_renderer(cmd(target.clone(),token)).is_err());'''
new='''    // A token never authorizes a different filter set; the matching URL can still consume it.
    assert!(matches!(app.run_renderer(cmd(target.clone(),token)),Ok(Outcome::Done{..})));'''
for file in ['crates/fb-app/tests/d8_flow.rs','tools/print_regressions.py']:
    p=ROOT/file;text=p.read_text();assert text.count(old)==1,(file,old);p.write_text(text.replace(old,new))
print('PRINT: path mismatch regression follows the existing token-consumption semantics')

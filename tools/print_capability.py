"""Bind print capabilities to the exact displayed query, including equivalent URL spellings."""
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def edit(name,old,new):
    p=ROOT/name;text=p.read_text();assert text.count(old)==1,(name,old);p.write_text(text.replace(old,new))
edit('crates/fb-app/src/screens/printing.rs','vm: &mut Value, target: &str, audience:', 'vm: &mut Value, _target: &str, audience:')
edit('crates/fb-app/src/screens/printing.rs','''    let html = app.screens.render(template, vm)?;
    let button''','''    Ok((app.screens.render(template, vm)?, 200))
}

pub(super) fn capability(app: &mut App, html: String, target: &str) -> String {
    let button''')
edit('crates/fb-app/src/screens/printing.rs','    Ok((html.replacen(button, &replacement, 1), 200))','    html.replacen(button, &replacement, 1)')
edit('crates/fb-app/src/screens.rs','    Ok(Rendered { html, status, route: name })','    let html = if status == 200 && html.contains("id=\\\"print-now\\\"") { printing::capability(app, html, target) } else { html };\n    Ok(Rendered { html, status, route: name })')
print('PRINT: token issuance bound to the exact original target')

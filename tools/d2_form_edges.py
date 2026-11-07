"""Python's actual integer grammar and form management values, plus setup flash messages."""
from pathlib import Path


def replace(path,old,new):
    p=Path(path);text=p.read_text();assert old in text,(path,old);p.write_text(text.replace(old,new))


p=Path('crates/fb-app/src/screens/d2.rs');t=p.read_text();a=t.index('fn clean_integer(');b=t.index('fn clean_revision(',a)
t=t[:a]+'''fn clean_integer(value: &str) -> Option<i64> {
    let value=py::py_strip(value);
    let value=if let Some((left,right))=value.rsplit_once('.') {if right.chars().all(|c|c=='0'){left}else{value}}else{value};
    forms::python_integer(value)
}
'''+t[b:];p.write_text(t)
replace('crates/fb-templates/src/vm.rs','''match requested.map(str::trim).map(str::parse::<i64>) {
            Some(Ok(n)) if n >= 1 =>''','''match requested.and_then(crate::forms::python_integer) {
            Some(n) if n >= 1 =>''')
replace('crates/fb-templates/src/vm.rs','''            Some(Ok(_)) => num_pages,''','''            Some(_) => num_pages,''')
replace('crates/fb-app/src/screens/d2.rs','''[("TOTAL_FORMS",total.to_string()),''','''[("TOTAL_FORMS",posted.map(|p|get(p,"lines-TOTAL_FORMS").unwrap_or("").to_owned()).unwrap_or(total.to_string())),''')
replace('crates/fb-app/src/session.rs','''    pub tokens: Tokens,''','''    pub tokens: Tokens,
    pub messages: Vec<String>,''')
replace('crates/fb-app/src/session.rs','''tokens: Tokens::default(), today: local_today''','''tokens: Tokens::default(), messages: vec![], today: local_today''')
replace('crates/fb-app/src/session.rs','''        self.tokens.clear();''','''        self.tokens.clear();
        self.messages.clear();''')
replace('crates/fb-app/src/screens/d2.rs','''    let route_name=route.name.as_str();''','''    let route_name=route.name.as_str();
    if route_name=="documents:setup" {put(&mut vm,"messages",std::mem::take(&mut app.messages));}''')
replace('crates/fb-app/src/screens/d2.rs','''    let state=app.open_mut()?.book.read(|tx|fb_core::first_run::checklist(tx,c))?;''','''    put(&mut vm,"messages",std::mem::take(&mut app.messages));
    let state=app.open_mut()?.book.read(|tx|fb_core::first_run::checklist(tx,c))?;''')
replace('crates/fb-app/src/screens/d2.rs','''Ok(_)=>Ok(Outcome::Redirect{location:url("setup_start",&[&c.to_db()])}),''','''Ok(_)=>{app.messages.push("Accounting setup filled from your chart. You can still change every choice.".into());Ok(Outcome::Redirect{location:url("setup_start",&[&c.to_db()])})},''')
replace('crates/fb-app/src/screens/d2.rs','''            let name=match route.name.as_str(){''','''            if route.name=="documents:setup" {app.messages.push(format!("Accounting setup saved (revision {}).",data["revision"]));}
            let name=match route.name.as_str(){''')

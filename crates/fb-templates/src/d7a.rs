//! Check reference snapshots use Python dictionary display, as the frozen template does.
use minijinja::{Environment, Value};
fn repr(v: &Value) -> String {
    match v.kind() {
        minijinja::value::ValueKind::String => {
            let s = v.as_str().unwrap();
            let quote = if s.contains('\'') && !s.contains('"') {
                '"'
            } else {
                '\''
            };
            let mut out = String::new();
            out.push(quote);
            for c in s.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if c == quote => {
                        out.push('\\');
                        out.push(c);
                    }
                    c if !printable(c) => {
                        let n=c as u32;
                        out.push_str(&if n<=0xff {format!("\\x{n:02x}")} else if n<=0xffff {format!("\\u{n:04x}")} else {format!("\\U{n:08x}")});
                    }
                    c => out.push(c),
                }
            }
            out.push(quote);
            out
        }
        minijinja::value::ValueKind::Map => format!(
            "{{{}}}",
            v.try_iter()
                .unwrap()
                .map(|k| format!("{}: {}", repr(&k), repr(&v.get_item(&k).unwrap())))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        minijinja::value::ValueKind::Seq => format!(
            "[{}]",
            v.try_iter()
                .unwrap()
                .map(|x| repr(&x))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => crate::django::display(v),
    }
}
fn printable(c:char)->bool {
    let n=c as u32;let ranges=crate::d7a_unicode::NONPRINTABLE;
    let i=ranges.partition_point(|(start,_)|*start<=n);
    i==0||ranges[i-1].1<n
}
pub fn register(env: &mut Environment<'_>) {
    env.add_filter("pyrepr", |v: Value| repr(&v));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_repr_preserves_python_quotes_and_nonprintable_escapes(){
        for (input,expected) in [("Café","'Café'"),("a'b","\"a'b\""),("a'\"b","'a\\'\"b'"),("a\nb","'a\\nb'"),("\u{a0}","'\\xa0'"),("\u{200b}","'\\u200b'"),("\u{897}","'\\u0897'"),("\u{10ffff}","'\\U0010ffff'")] {
            assert_eq!(repr(&Value::from(input)),expected,"{input:?}");
        }
        assert_eq!(repr(&Value::from_serialize(serde_json::json!({"id":"known","active":true,"missing":null}))),"{'active': True, 'id': 'known', 'missing': None}");
    }
}

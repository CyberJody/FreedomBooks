//! Display-only frozen sales print filters. Never use floating point for stored amounts.
use minijinja::{Environment, Value};

fn text(v: Value) -> String {
    if v.is_none() || v.is_undefined() {
        String::new()
    } else {
        v.to_string()
    }
}
fn qty(v: Value) -> String {
    let s = text(v);
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    }
}
fn css_string(v: Value) -> Value {
    Value::from_safe_string(
        text(v)
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace(['\n', '\r', '\u{c}', '\0'], " ")
            .replace('<', "\\3C ")
            .replace('>', "\\3E ")
            .replace('&', "\\26 "),
    )
}
pub fn register(env: &mut Environment<'_>) {
    env.add_filter("print_qty", qty);
    env.add_filter("css_string", css_string);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn footer_text_cannot_escape_css_or_the_style_element() {
        assert_eq!(
            css_string(Value::from("a\"\\\n\r\u{c}\0</style>&")).as_str(),
            Some("a\\\"\\\\    \\3C /style\\3E \\26 ")
        );
        assert_eq!(qty(Value::from("100.0100")), "100.01");
        assert_eq!(qty(Value::from("0.0000")), "0");
        assert_eq!(qty(Value::from("100")), "100");
    }
}

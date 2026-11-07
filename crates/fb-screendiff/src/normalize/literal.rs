//! Read Python's repr of JSON data for the journal audit <pre> cells. Free-text values stay strings:
//! the caller normalizes only typed snapshot/line identity fields, never text that resembles a key.
use serde_json::{Map, Value, json};

struct Parser {
    chars: Vec<char>,
    at: usize,
}
impl Parser {
    fn whitespace(&mut self) {
        while self.chars.get(self.at).is_some_and(|c| c.is_whitespace()) {
            self.at += 1;
        }
    }
    fn take(&mut self, c: char) -> bool {
        self.whitespace();
        if self.chars.get(self.at) == Some(&c) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn quoted(&mut self) -> Option<String> {
        self.whitespace();
        let quote = *self.chars.get(self.at)?;
        if !['\'', '"'].contains(&quote) {
            return None;
        }
        self.at += 1;
        let mut out = String::new();
        while let Some(&c) = self.chars.get(self.at) {
            self.at += 1;
            if c == quote {
                return Some(out);
            }
            if c != '\\' {
                out.push(c);
                continue;
            }
            let c = *self.chars.get(self.at)?;
            self.at += 1;
            match c {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                '\\' | '\'' | '"' => out.push(c),
                'x' | 'u' | 'U' => {
                    let digits = match c {
                        'x' => 2,
                        'u' => 4,
                        _ => 8,
                    };
                    let mut n = 0u32;
                    for _ in 0..digits {
                        n = n
                            .checked_mul(16)?
                            .checked_add(self.chars.get(self.at)?.to_digit(16)?)?;
                        self.at += 1;
                    }
                    out.push(char::from_u32(n)?);
                }
                _ => return None,
            }
        }
        None
    }
    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth > 32 {
            return None;
        }
        self.whitespace();
        let c = *self.chars.get(self.at)?;
        if ['\'', '"'].contains(&c) {
            return self.quoted().map(Value::String);
        }
        if self.take('{') {
            let mut map = Map::new();
            if self.take('}') {
                return Some(Value::Object(map));
            }
            loop {
                let key = self.quoted()?;
                if !self.take(':') {
                    return None;
                }
                let value = self.value(depth + 1)?;
                if map.insert(key, value).is_some() {
                    return None;
                }
                if self.take('}') {
                    break;
                }
                if !self.take(',') {
                    return None;
                }
            }
            return Some(Value::Object(map));
        }
        if self.take('[') {
            let mut list = vec![];
            if self.take(']') {
                return Some(Value::Array(list));
            }
            loop {
                list.push(self.value(depth + 1)?);
                if self.take(']') {
                    break;
                }
                if !self.take(',') {
                    return None;
                }
            }
            return Some(Value::Array(list));
        }
        let start = self.at;
        while self
            .chars
            .get(self.at)
            .is_some_and(|c| !c.is_whitespace() && !['}', ']', ','].contains(c))
        {
            self.at += 1;
        }
        let token: String = self.chars[start..self.at].iter().collect();
        match token.as_str() {
            "None" => Some(Value::Null),
            "True" => Some(json!(true)),
            "False" => Some(json!(false)),
            _ => serde_json::from_str(&token).ok(),
        }
    }
}

pub(super) fn parse(text: &str) -> Option<Value> {
    if text.len() > 1_000_000 {
        return None;
    }
    let mut parser = Parser {
        chars: text.chars().collect(),
        at: 0,
    };
    let value = parser.value(0)?;
    parser.whitespace();
    (parser.at == parser.chars.len()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repr_strings_are_data_and_duplicates_or_trailing_input_are_refused() {
        assert_eq!(
            parse(
                r#"{'memo': "'account_id': 1", 'lines': [{'account_id': 1}], 'n': None, 'yes': True}"#
            ),
            Some(json!({"memo":"'account_id': 1","lines":[{"account_id":1}],"n":null,"yes":true}))
        );
        assert_eq!(
            parse(r"{'text': '\\u1234\n\x00\U0001f600'}"),
            Some(json!({"text":"\\u1234\n\0😀"}))
        );
        assert!(parse("{'x': 1, 'x': 2}").is_none());
        assert!(parse("{}more").is_none());
    }
}

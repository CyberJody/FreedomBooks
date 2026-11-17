//! Plain-text substitutions only. The email template still escapes the resulting values.
pub fn expand(text: &str, fields: &[(&str, &str)]) -> String {
    let mut remaining = text;
    let mut output = String::new();
    while let Some(start) = remaining.find('{') {
        output.push_str(&remaining[..start]);
        remaining = &remaining[start..];
        let Some(end) = remaining.find('}') else { break; };
        let name = &remaining[1..end];
        if let Some((_, value)) = fields.iter().find(|(key, _)| *key == name) {
            output.push_str(value);
        } else {
            output.push_str(&remaining[..=end]);
        }
        remaining = &remaining[end+1..];
    }
    output.push_str(remaining);
    output
}

#[cfg(test)]
mod tests {
    #[test]
    fn substitutions_are_single_pass_and_never_execute_templates() {
        assert_eq!(super::expand("Hi {customer_name}, {business_name} {unknown} {", &[
            ("customer_name", "{business_name}"), ("business_name", "<Sample> & Co.")
        ]), "Hi {business_name}, <Sample> & Co. {unknown} {");
    }
}

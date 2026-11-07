//! Django EmailValidator's accepted address grammar (not a deliverability check).
//! Model max_length (254) is checked separately after this validator.
pub fn valid_email(s: &str) -> bool {
    if s.is_empty() || s.chars().count() > 320 {
        return false;
    }
    let Some((local, domain)) = s.rsplit_once('@') else {
        return false;
    };
    let ascii_letter = |c: char| c.is_ascii_alphabetic() || ['İ', 'ı', 'ſ', 'K'].contains(&c);
    let atom = |c: char| ascii_letter(c) || c.is_ascii_digit() || "-!#$%&'*+/=?^_`{}|~".contains(c);
    let dot_atom = !local.is_empty() && local.split('.').all(|a| !a.is_empty() && a.chars().all(atom));
    let quoted = if local.starts_with('"') && local.ends_with('"') && local.len() >= 2 {
        let mut chars = local[1..local.len() - 1].chars();
        let mut ok = true;
        while let Some(c) = chars.next() {
            if c == '\\' {
                if !chars.next().is_some_and(|e| matches!(e as u32,1..=9|11..=12|14..=127)) {
                    ok = false;
                    break;
                }
            } else if !matches!(c as u32,1..=8|11..=12|14..=31|33|35..=91|93..=127) {
                ok = false;
                break;
            }
        }
        ok
    } else {
        false
    };
    if !dot_atom && !quoted {
        return false;
    }
    if domain == "localhost" {
        return true;
    }
    if let Some(ip) = domain.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        return ip.parse::<std::net::IpAddr>().is_ok();
    }
    let unicode = |c: char| matches!(c as u32, 0xa1..=0xffff);
    let alnum = |c: char| ascii_letter(c) || c.is_ascii_digit() || unicode(c);
    let labels = domain.split('.').collect::<Vec<_>>();
    if labels.len() < 2 {
        return false;
    }
    for label in &labels {
        if label.is_empty()
            || label.chars().count() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label.chars().all(|c| alnum(c) || c == '-')
        {
            return false;
        }
    }
    let last = labels.last().unwrap();
    (last.chars().count() >= 2 && last.chars().all(|c| ascii_letter(c) || unicode(c) || c == '-'))
        || last
            .strip_prefix("xn--")
            .is_some_and(|s| !s.is_empty() && s.len() <= 59 && s.chars().all(|c| c.is_ascii_alphanumeric()))
}

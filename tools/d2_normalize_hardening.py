"""Keep audit identity normalization inside parsed, typed event detail cells."""
from pathlib import Path


def replace(path,old,new):
    p=Path(path);t=p.read_text();assert old in t,(path,old);p.write_text(t.replace(old,new))


replace('crates/fb-screendiff/src/normalize.rs','''use regex::{Captures, Regex};''','''mod literal;
use regex::{Captures, Regex};''')
replace('crates/fb-screendiff/src/normalize.rs','''static AUDIT_ACCOUNT: LazyLock<Regex> = LazyLock::new(||Regex::new(r"(&#x27;account_id&#x27;: )(?:&#x27;([0-9a-f-]{36})&#x27;|([0-9]+))").unwrap());''','''static AUDIT_PRE: LazyLock<Regex> = LazyLock::new(||Regex::new(r"(?s)<pre>(.*?)</pre>").unwrap());''')
replace('crates/fb-screendiff/src/normalize.rs','''    let text = AUDIT_ACCOUNT.replace_all(&text, |c:&Captures| {
        let raw=c.get(2).or_else(||c.get(3)).unwrap().as_str();
        match ids.get("account",raw){Some(sym)=>format!("{}&#x27;{sym}&#x27;",&c[1]),None=>c[0].to_owned()}
    });''','''    let text = AUDIT_PRE.replace_all(&text, |c:&Captures| {
        let plain=c[1].replace("&#x27;","'").replace("&quot;","\\\"").replace("&lt;","<").replace("&gt;",">").replace("&amp;","&");
        let Some(mut value)=literal::parse(&plain)else{return c[0].to_owned()};
        for key in ["before","after"] {
            if let Some(lines)=value.get_mut(key).and_then(|v|v.get_mut("lines")).and_then(Value::as_array_mut) {
                for line in lines {if let Some(raw)=line.get_mut("account_id") {
                    let id=raw.as_str().map(str::to_owned).unwrap_or_else(||raw.to_string());
                    if let Some(sym)=ids.get("account",&id){*raw=Value::String(sym.into());}
                }}
            }
        }
        if let Some(raw)=value.get_mut("reversal_id") {
            let id=raw.as_str().map(str::to_owned).unwrap_or_else(||raw.to_string());
            if let Some(sym)=ids.get("journal",&id){*raw=Value::String(sym.into());}
        }
        format!("<pre>{}</pre>",fb_templates::django::escape(&serde_json::to_string(&canonical(value)).unwrap()))
    });''')
replace('crates/fb-screendiff/src/normalize.rs','''Some(("customer", v)) => format!("customer={}", ids.get("customer", v).unwrap_or(v)),''','''Some(("customer", v)) => format!("customer={}", ids.get("customer", v).unwrap_or(v)),
                    Some(("user", v)) => format!("user={}", ids.get("user", v).unwrap_or(v)),''')

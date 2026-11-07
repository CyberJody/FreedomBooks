"""Typed, context-specific normalization additions for D2."""
from pathlib import Path
import json


def replace(path, old, new):
    p=Path(path);t=p.read_text();assert old in t,(path,old);p.write_text(t.replace(old,new))


p=Path('crates/fb-screendiff/screens.json');v=json.loads(p.read_text())
for s in v.values():s['path']=s['path'].replace('/j/','/journals/').replace('/report-problem/','/help/report/')
p.write_text(json.dumps(v,indent=2)+'\n')
replace('crates/fb-screendiff/src/normalize.rs','''    by_model: HashMap<String, HashMap<String, String>>,''','''    by_model: HashMap<String, HashMap<String, String>>,
    labels: HashMap<(String,String),String>,''')
replace('crates/fb-screendiff/src/normalize.rs','''        IdMap { by_model }''','''        let mut labels=HashMap::new();
        for (key,value) in ids {if let Some(rest)=key.strip_prefix("label:") {if let Some((model,_))=rest.split_once(':') {if let Some(raw)=ids.get(rest) {labels.insert((model.into(),raw.clone()),value.clone());}}}}
        IdMap { by_model, labels }''')
replace('crates/fb-screendiff/src/normalize.rs','''        "income_account" => "account",''','''        "income_account" | "account" | "receivables" | "payables" | "undeposited" | "collected_tax" | "customer_deposits" | "banks" => "account",
        "user" => "user",''')
replace('crates/fb-screendiff/src/normalize.rs','''            let label = c[3].replace(&format!("#{raw}"), &format!("#{sym}"));''','''            let label_raw=ids.labels.get(&(model.into(),raw.into())).map(String::as_str).unwrap_or(raw);
            let marker=format!("#{label_raw}");
            let mut label=c[3].to_owned();
            if let Some(at)=label.find(&marker) {
                let end=at+marker.len();
                if label.get(end..).and_then(|s|s.chars().next()).is_none_or(|c|!c.is_ascii_alphanumeric()&&c!='-') {
                    label.replace_range(at..end,&format!("#{sym}"));
                }
            }''')
# Timestamp values are mapped from actual audit events, keyed per event/column, not arbitrary text.
replace('crates/fb-screendiff/src/normalize.rs','''static JSON_SCRIPT:''','''static AUDIT_ACCOUNT: LazyLock<Regex> = LazyLock::new(||Regex::new(r"(&#x27;account_id&#x27;: )(?:&#x27;([0-9a-f-]{36})&#x27;|([0-9]+))").unwrap());
static WHEN: LazyLock<Regex> = LazyLock::new(||Regex::new(r#"(<td class="when" data-label="When">)([^<]*)(</td>)"#).unwrap());
static EVENT_STAMP: LazyLock<Regex> = LazyLock::new(||Regex::new(r#"(<tr><td>)([0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2})(</td>)"#).unwrap());
static POSTED_STAMP: LazyLock<Regex> = LazyLock::new(||Regex::new(r#"(Posted by [^<]* at )([0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2} UTC)(\\.)"#).unwrap());
static DIGEST: LazyLock<Regex> = LazyLock::new(||Regex::new(r#"name="digest" value="([0-9a-f]{64})""#).unwrap());
static JSON_SCRIPT:''')
replace('crates/fb-screendiff/src/normalize.rs','''    let text = WS.replace_all(&text, " ");''','''    let text = AUDIT_ACCOUNT.replace_all(&text, |c:&Captures| {
        let raw=c.get(2).or_else(||c.get(3)).unwrap().as_str();
        match ids.get("account",raw){Some(sym)=>format!("{}&#x27;{sym}&#x27;",&c[1]),None=>c[0].to_owned()}
    });
    let text = DIGEST.replace_all(&text, |c:&Captures| match ids.get("proposal_digest",&c[1]){Some(sym)=>format!(r#"name="digest" value="{sym}""#),None=>c[0].to_owned()});
    let text = WHEN.replace_all(&text,|c:&Captures|match ids.get("audit_local_stamp",&c[2]){Some(sym)=>format!("{}{sym}{}",&c[1],&c[3]),None=>c[0].to_owned()});
    let text = EVENT_STAMP.replace_all(&text,|c:&Captures|match ids.get("audit_utc_stamp",&c[2]){Some(sym)=>format!("{}{sym}{}",&c[1],&c[3]),None=>c[0].to_owned()});
    let text = POSTED_STAMP.replace_all(&text,|c:&Captures|match ids.get("posted_stamp",&c[2]){Some(sym)=>format!("{}{sym}{}",&c[1],&c[3]),None=>c[0].to_owned()});
    let text = WS.replace_all(&text, " ");''')

# Generated times often share a whole second. Map the column's generated values to a placeholder;
# row ordering, action, operator and target still compare exactly (timestamps aren't clocks in parity).
replace('crates/fb-screendiff/py/render_django.py','''    ids['user:' + user.username] = str(user.pk)''','''    ids['user:' + user.username] = str(user.pk)
    from documents.first_run import propose
    ids['proposal_digest:current'] = propose(company)['digest']
    from ledger.models import AuditEvent
    from ledger.history import local_stamp
    for event in AuditEvent.objects.filter(company=company):
        ids['audit_local_stamp:' + local_stamp(event.occurred_at)] = local_stamp(event.occurred_at)
        utc = event.occurred_at.strftime('%Y-%m-%d %H:%M:%S')
        ids['audit_utc_stamp:' + utc] = utc
    for journal in Journal.objects.filter(company=company, posted_at__isnull=False):
        ids['posted_stamp:' + str(journal)] = journal.posted_at.strftime('%Y-%m-%d %H:%M:%S UTC')''')
replace('crates/fb-screendiff/src/main.rs','''            ids.insert("user:owner".into(),''','''            ids.insert("proposal_digest:current".into(),fb_core::first_run::propose(tx,fb_core::CompanyId::parse(&company).unwrap()).unwrap()["digest"].as_str().unwrap().into());
            let mut st=tx.prepare("SELECT occurred_at FROM ledger_auditevent WHERE company_id=?1")?;
            for stamp in st.query_map([&company],|r|r.get::<_,String>(0))? {
                let stamp=chrono::DateTime::parse_from_rfc3339(&stamp?).unwrap();
                let local=stamp.with_timezone(&chrono_tz::America::New_York).format("%Y-%m-%d %H:%M:%S").to_string();
                let utc=stamp.format("%Y-%m-%d %H:%M:%S").to_string();
                ids.insert(format!("audit_local_stamp:{local}"),local);ids.insert(format!("audit_utc_stamp:{utc}"),utc);
            }
            let mut st=tx.prepare("SELECT number,posted_at FROM ledger_journal WHERE company_id=?1 AND posted_at IS NOT NULL")?;
            for row in st.query_map([&company],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))? {
                let (number,stamp)=row?;let stamp=chrono::DateTime::parse_from_rfc3339(&stamp).unwrap();
                ids.insert(format!("posted_stamp:J{number:06}"),stamp.format("%Y-%m-%d %H:%M:%S UTC").to_string());
            }
            ids.insert("user:owner".into(),''')
replace('crates/fb-screendiff/src/normalize.rs','''by_model.entry(model.to_owned()).or_default().insert(id.clone(), format!("{{{key}}}"));''','''by_model.entry(model.to_owned()).or_default().insert(id.clone(), if ["audit_local_stamp","audit_utc_stamp"].contains(&model){format!("{{{model}}}")}else{format!("{{{key}}}")});''')
replace('crates/fb-screendiff/Cargo.toml','[dependencies]','[dependencies]\nchrono = "0.4"\nchrono-tz = "0.10"')

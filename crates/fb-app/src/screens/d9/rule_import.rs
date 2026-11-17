//! Native chooser -> immutable, company-bound preview -> validated atomic import.
use super::*;
use bf::qbo;
use std::path::Path;

pub(crate) struct Preview {
    pub company: CompanyId,
    pub source: qbo::Import,
    pub token: String,
    pub at: std::time::Instant,
    pub posted: Vec<(String, String)>,
}
fn location(c: CompanyId) -> Outcome {
    Outcome::Redirect {
        location: format!("/c/{}/bank-feeds/rules/", c.to_db()),
    }
}
pub(crate) fn picked(app: &mut App, c: CompanyId, path: Option<&Path>) -> AppResult<Outcome> {
    let owner = actor(app)?;
    app.open_mut()?
        .book
        .read(|q| fb_core::services::access(q, owner, c))?;
    if let Some(path) = path {
        let bytes =
            attachments::read_limited(path, qbo::MAX_FILE as u64).map_err(AppError::Invalid)?;
        let source = qbo::parse_xls(&bytes)?;
        app.backups.rule_import = Some(Preview {
            company: c,
            source,
            token: fb_ids::new_v7().to_string(),
            at: std::time::Instant::now(),
            posted: vec![],
        });
    }
    Ok(location(c))
}
fn preview(app: &App, c: CompanyId) -> AppResult<&Preview> {
    app.backups
        .rule_import
        .as_ref()
        .filter(|p| p.company == c && p.at.elapsed().as_secs() < 1800)
        .ok_or_else(|| {
            AppError::Invalid("The import preview expired. Choose the file again.".into())
        })
}
pub(super) fn page(
    app: &mut App,
    r: &Route,
    path: &str,
    error: Option<String>,
) -> AppResult<String> {
    let c = arg_company(r)?;
    let owner = actor(app)?;
    let mut vm = base(app, r, path)?;
    let accounts = bf::rule_accounts(&mut app.open_mut()?.book, owner, c)?;
    let (feeds,payees,imported,names)=app.open_mut()?.book.read(|q|->Result<_,FbError>{
        fb_core::services::access(q,owner,c)?;
        let mut st=q.prepare("SELECT f.id,f.name FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE c.company_id=?1 AND c.active=1 AND f.active=1 AND f.ledger_account_id IS NOT NULL ORDER BY f.name")?;
        let feeds=st.query_map([c.to_db()],|r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?})))?.collect::<Result<Vec<_>,_>>()?;
        let mut payees=vec![];
        for (table,label) in [("directory_vendor","Vendor"),("directory_customer","Customer")] {
            let mut st=q.prepare(&format!("SELECT id,display_name FROM {table} WHERE company_id=?1 AND active=1 ORDER BY display_name"))?;
            payees.extend(st.query_map([c.to_db()],|r|{let id:String=r.get(0)?;Ok(json!({"id":format!("{table}:{id}"),"name":r.get::<_,String>(1)?,"kind":label}))})?.collect::<Result<Vec<_>,_>>()?);
        }
        let mut st=q.prepare("SELECT source_key FROM bankfeeds_rule_policy WHERE company_id=?1")?;
        let imported=st.query_map([c.to_db()],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
        let mut st=q.prepare("SELECT name FROM bankfeeds_rule WHERE company_id=?1 AND active=1")?;let names=st.query_map([c.to_db()],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
        Ok((feeds,payees,imported,names))
    })?;
    let p = preview(app, c)?;
    let rows = p
        .source
        .rows
        .iter()
        .map(|row| {
            let mut v = serde_json::to_value(row).unwrap_or(Value::Null);
            let matches = accounts
                .iter()
                .filter(|a| {
                    bf::text::py_lower(&a.name) == bf::text::py_lower(&row.category)
                        || bf::text::py_lower(&format!("{} {}", a.code, a.name))
                            == bf::text::py_lower(&row.category)
                })
                .collect::<Vec<_>>();
            let payee_matches = payees
                .iter()
                .filter(|p| {
                    bf::text::py_lower(p["name"].as_str().unwrap_or(""))
                        == bf::text::py_lower(&row.payee)
                })
                .collect::<Vec<_>>();
            v["account_match"] = json!(if matches.len() == 1 {
                matches[0].id.to_db()
            } else {
                String::new()
            });
            v["payee_match"] = json!(if payee_matches.len() == 1 {
                payee_matches[0]["id"].as_str().unwrap_or("")
            } else {
                ""
            });
            let mut mapping_notes=vec![];
            if matches.is_empty(){mapping_notes.push("No eligible category matched this name. Choose an active posting account; missing, inactive and control accounts cannot be used.");}
            if matches.len()>1{mapping_notes.push("More than one eligible category matches. Choose the intended destination account.");}
            if !row.payee.is_empty() && payee_matches.is_empty(){mapping_notes.push("No active customer or vendor matched this payee. Choose the intended payee before importing.");}
            if !row.payee.is_empty() && payee_matches.len()>1{mapping_notes.push("This payee name is ambiguous. Choose the correct customer or vendor.");}
            v["mapping_notes"]=json!(mapping_notes);
            v["duplicate"] = json!(
                imported.contains(&row.source_key)
                    || names
                        .iter()
                        .any(|n| bf::text::py_lower(n) == bf::text::py_lower(&row.name))
            );
            if !p.posted.is_empty() {
                v["account_match"] =
                    json!(get(&p.posted, &format!("category_{}", row.row)).unwrap_or(""));
                v["payee_match"] =
                    json!(get(&p.posted, &format!("payee_{}", row.row)).unwrap_or(""));
            }
            v["selected"] = json!(checked(&p.posted, &format!("select_{}", row.row)));
            v["scope"] = json!(get(&p.posted, &format!("scope_{}", row.row)).unwrap_or(""));
            v["kind"] = json!(get(&p.posted, &format!("kind_{}", row.row)).unwrap_or(""));
            v
        })
        .collect::<Vec<_>>();
    put(
        &mut vm,
        "description_mapping",
        checked(&p.posted, "description_mapping"),
    );
    put(&mut vm, "import_token", &p.token);
    put(&mut vm, "preserve_auto_post", checked(&p.posted, "preserve_auto_post"));
    put(&mut vm, "source_hash", &p.source.hash);
    put(&mut vm, "rows", rows);
    put(&mut vm, "feeds", feeds);
    put(&mut vm, "payees", payees);
    put(
        &mut vm,
        "accounts",
        accounts
            .iter()
            .map(|a| json!({"id":a.id.to_db(),"label":account_label(a)}))
            .collect::<Vec<_>>(),
    );
    put(&mut vm, "error", error);
    draw(app, "bankfeeds/rule_import.html", &vm)
}
pub(super) fn submit(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    let owner = actor(app)?;
    match get(p, "import_action").unwrap_or("") {
        "choose" => {
            let ticket = app.backups.pick_rules(c)?;
            Ok(Outcome::Done {
                data: json!({"desktop":"pick-bank-rules","ticket":ticket}),
            })
        }
        "cancel" => {
            app.backups.rule_import = None;
            Ok(location(c))
        }
        "confirm" => {
            let pre = preview(app, c)?;
            if get(p, "import_token") != Some(pre.token.as_str()) {
                return Err(AppError::Invalid(
                    "This import preview is no longer current.".into(),
                ));
            }
            let source = pre.source.clone();
            if let Some(pre) = app.backups.rule_import.as_mut() {
                pre.posted = p.to_vec();
            }
            let mut selections = vec![];
            for row in &source.rows {
                let key = |s: &str| format!("{s}_{}", row.row);
                if !checked(p, &key("select")) {
                    continue;
                }
                let payee = get(p, &key("payee")).unwrap_or("");
                let (table, id) = payee.split_once(':').unwrap_or(("", ""));
                selections.push(qbo::Selection {
                    row: row.row,
                    preserve_auto_post: checked(p, "preserve_auto_post"),
                    account_id: get(p, &key("category")).unwrap_or("").into(),
                    scope: get(p, &key("scope")).unwrap_or("").into(),
                    kind: get(p, &key("kind")).unwrap_or("").into(),
                    payee_id: id.into(),
                    payee_table: table.into(),
                });
            }
            match qbo::import_rules(
                &mut app.open_mut()?.book,
                owner,
                c,
                &source,
                &selections,
                checked(p, "description_mapping"),
                &bf::SecretSet::none(),
            ) {
                Ok(result) => {
                    app.backups.rule_import = None;
                    app.messages.push(format!("Imported {} rules; {} retain Auto-post. No existing downloaded transactions were posted.",result["imported"],result["auto_post"]));
                    Ok(location(c))
                }
                Err(e) => Ok(Outcome::Invalid {
                    html: page(app, r, path, Some(messages(e)?))?,
                }),
            }
        }
        _ => Err(AppError::Invalid("Choose an import action.".into())),
    }
}
pub(super) fn test(app: &mut App, r: &Route, path: &str, id: &str) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    let owner = actor(app)?;
    let rules = bf::all_rules(&mut app.open_mut()?.book, owner, c)?;
    let mut rule = rules
        .iter()
        .find(|r| r.id == id)
        .cloned()
        .ok_or_else(|| AppError::NotFound("rule".into()))?;
    rule.active = true;
    let matches=app.open_mut()?.book.read(|q|->Result<_,FbError>{
        let mut st=q.prepare("SELECT t.id FROM bankfeeds_transaction t JOIN bankfeeds_account a ON a.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=a.connection_id WHERE c.company_id=?1 AND t.status='NEW' ORDER BY t.ordinal LIMIT 5000")?;
        let ids=st.query_map([c.to_db()],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
        let mut rows=vec![];
        for id in ids {let item=bf::item(q,c,&id)?;if bf::rule_for(std::slice::from_ref(&rule),&item).is_some(){
            let first=bf::rule_for(&rules,&item);let possible=bf::match_candidates(q,c,&item,1)?.len();
            rows.push(json!({"date":item.date(),"description":item.description,"amount":cents(item.amount),"pending":item.pending,"first":first.map(|r|r.name.clone()),"existing":possible>0}));
        }}Ok(rows)
    })?;
    let mut vm = base(app, r, path)?;
    let scope_label = if let Some(policy) = &rule.policy {
        if policy.scope == "ALL" { "All bank feeds in this company".to_owned() }
        else { app.open_mut()?.book.read(|q| q.query_row("SELECT name FROM bankfeeds_account WHERE id=?1", [&policy.scope], |r| r.get::<_,String>(0)))? }
    } else { "All bank feeds in this company".to_owned() };
    put(&mut vm, "test_scope", scope_label);
    put(&mut vm, "test_name", rule.name);
    put(&mut vm, "matches", matches);
    put(&mut vm, "policy", rule.policy);
    Ok(Outcome::Page {
        html: draw(app, "bankfeeds/rule_test.html", &vm)?,
        status: 200,
    })
}

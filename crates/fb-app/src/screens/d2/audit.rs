//! Frozen ledger.history presentation and journal detail, with company-scoped target resolution.
use super::*;
use std::sync::LazyLock;

static LABELS: LazyLock<Value> =
    LazyLock::new(|| serde_json::from_str(include_str!("history_labels.json")).unwrap());
/// SIM-FIX (deliberate divergence, owner approved 11/03): plain labels for actions the frozen
/// `ACTION_LABELS` never named, so the history no longer shows code names like `salestax.rate.created`.
/// Kept apart from `history_labels.json` (the frozen v1 copy). Bank-feed actions stay codes (left open).
pub const SIM_FIX_ACTION_LABELS: [(&str, &str); 19] = [
    ("salestax.rate.created", "Sales tax rate created"),
    ("salestax.rate.updated", "Sales tax rate updated"),
    ("salestax.rate.deactivated", "Sales tax rate made inactive"),
    ("salestax.rate.reactivated", "Sales tax rate reactivated"),
    ("salestax.group.created", "Sales tax group created"),
    ("salestax.group.updated", "Sales tax group updated"),
    ("salestax.group.deactivated", "Sales tax group made inactive"),
    ("salestax.group.reactivated", "Sales tax group reactivated"),
    ("salestax.settings.updated", "Sales tax settings saved"),
    ("salestax.bulk_step", "Sales tax turn-on step finished"),
    ("customer_deposit.recorded", "Customer deposit recorded"),
    ("customer_deposit.applied", "Customer deposit applied"),
    ("customer_deposit.unapplied", "Customer deposit unapplied"),
    ("customer_deposit.voided", "Customer deposit voided"),
    ("deposit_refund.recorded", "Deposit refund recorded"),
    ("deposit_refund.voided", "Deposit refund voided"),
    ("backup.create", "Backup created"),
    ("backup.download", "Backup copy saved"),
    ("backup.restore", "Backup restored as a new book"),
];
fn sim_fix_labels() -> impl Iterator<Item = (&'static str, &'static str)> {
    SIM_FIX_ACTION_LABELS.into_iter()
}
const DOCUMENT_DETAILS_ACTION: (&str, &str) = ("company.document_details_updated", "Company document details changed");
fn action(code: &str) -> String {
    if code == DOCUMENT_DETAILS_ACTION.0 { return DOCUMENT_DETAILS_ACTION.1.into(); }
    if code=="numbering.changed" {return "Next document number changed".into();}
    LABELS["ACTION_LABELS"][code]
        .as_str()
        .or_else(|| sim_fix_labels().find(|(k, _)| *k == code).map(|(_, v)| v))
        .unwrap_or(code)
        .to_owned()
}

fn history_csv(rows: &[Value]) -> String {
    fn cell(text: &str) -> String {
        if text.is_empty() { "'".into() } else { fb_core::reports::params::csv_cell(text) }
    }
    let zone = std::env::var("FB_TIME_ZONE").unwrap_or_else(|_| "America/New_York".into());
    let mut out = format!("\u{feff}# Times shown in {zone}\nWhen,User,Action,Document,Reason\r\n");
    for row in rows.iter().take(5000) {
        out.push_str(
            &["when", "operator", "action", "number", "reason"]
                .iter()
                .map(|key| cell(row[*key].as_str().unwrap_or("")))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push_str("\r\n");
    }
    out
}

pub(crate) fn export_history(
    app: &mut App,
    c: CompanyId,
    filters: Posted<'_>,
) -> AppResult<Outcome> {
    if filters.len() > 100
        || filters
            .iter()
            .any(|(k, v)| k.len() > 100 || v.len() > 10000)
    {
        return Err(AppError::Invalid("Filters too large.".into()));
    }
    let route = Route {
        name: "history".into(),
        args: BTreeMap::from([("company_id".into(), c.to_db())]),
    };
    let mut filters: Vec<_> = filters
        .iter()
        .filter(|(k, _)| k != "format")
        .cloned()
        .collect();
    filters.push(("format".into(), "csv".into()));
    let (contents, _) = history(app, &route, &filters)?;
    if !contents.starts_with('\u{feff}') {
        return Err(AppError::Invalid(
            "Correct the history filters before downloading.".into(),
        ));
    }
    Ok(Outcome::Export {
        filename: "audit-history.csv".into(),
        contents,
    })
}

fn target(
    tx: &(impl Query + ?Sized),
    c: CompanyId,
    value: &str,
    detail: &Value,
) -> Result<(String, Option<String>), FbError> {
    if let Some(label) = LABELS["NAMED_TARGETS"][value].as_str() {
        return Ok((label.into(), None));
    }
    let (model, raw) = value.split_once(':').unwrap_or(("", ""));
    if fb_ids::parse_uuid(raw).is_some() {
        if model == "company" && raw == c.to_db() {
            return Ok((
                store::get_company(tx, c)?.name,
                Some(url("company_edit", &[&c.to_db()])),
            ));
        }
        if model == "journal"
            && let Some(j) = JournalId::parse(raw)
                .map(|j| store::journal(tx, j))
                .transpose()?
                .flatten()
                .filter(|j| j.company_id == c)
        {
            return Ok((
                if j.entry_number.trim().is_empty() {
                    format!("Journal {}", j.display())
                } else {
                    format!("General journal {}", j.entry_number.trim())
                },
                Some(url("journal", &[&c.to_db(), raw])),
            ));
        }
        if model == "reconciliation" {
            let label=tx.query_row("SELECT a.code || ' · statement ' || r.statement_date FROM banking_reconciliation r JOIN ledger_account a ON a.id=r.account_id AND a.company_id=r.company_id WHERE r.id=?1 AND r.company_id=?2",params![raw,c.to_db()],|r|r.get::<_,String>(0)).optional()?;
            if let Some(label) = label {
                return Ok((label, Some(url("reconcile:report", &[&c.to_db(), raw]))));
            }
        }
        let spec = match model {
            "account" => Some((
                "ledger_account",
                "code || ' · ' || name",
                "account_edit",
                None,
            )),
            "customer" => Some((
                "directory_customer",
                "display_name",
                "directory:detail",
                Some("customers"),
            )),
            "vendor" => Some((
                "directory_vendor",
                "display_name",
                "directory:detail",
                Some("vendors"),
            )),
            "item" => Some(("directory_item", "code", "directory:detail", Some("items"))),
            "accountingsetup" => Some((
                "documents_accountingsetup",
                "'Accounting setup'",
                "documents:setup",
                None,
            )),
            "documentidentity" => Some(("documents_documentidentity", "number", "", None)),
            // SIM-FIX: sales tax records resolve instead of "Record (no longer on file)".
            "taxitem" => Some(("salestax_taxitem", "name", "salestax:item", None)),
            "salestaxsettings" => Some(("salestax_salestaxsettings", "'Sales tax settings'", "salestax:settings", None)),
            _ => None,
        };
        if let Some((table, column, route, kind)) = spec {
            let found = tx
                .query_row(
                    &format!("SELECT {column} FROM {table} WHERE id=?1 AND company_id=?2"),
                    params![raw, c.to_db()],
                    |r| r.get::<_, String>(0),
                )
                .optional()?;
            if let Some(label) = found {
                let link = if route.is_empty() {
                    None
                } else if route == "salestax:item" {
                    let kind = tx.query_row("SELECT kind FROM salestax_taxitem WHERE id=?1", [raw], |r| r.get::<_, String>(0))?;
                    Some(url(if kind == "GROUP" { "salestax:group_edit" } else { "salestax:rate_edit" }, &[&c.to_db(), raw]))
                } else if route == "salestax:settings" {
                    Some(url(route, &[&c.to_db()]))
                } else if let Some(kind) = kind {
                    Some(url(route, &[&c.to_db(), kind, raw]))
                } else if route == "documents:setup" {
                    Some(url(route, &[&c.to_db()]))
                } else {
                    Some(url(route, &[&c.to_db(), raw]))
                };
                return Ok((label, link));
            }
        }
        let spec = match model {
            "invoice" => Some(("sales_invoice", "invoices:detail")),
            "estimate" => Some(("sales_estimate", "sales:detail")),
            "receipt" => Some(("sales_receipt", "receipts:detail")),
            "bill" => Some(("payables_bill", "bills:detail")),
            "billpayment" => Some(("payables_billpayment", "payments:detail")),
            "check" => Some(("checks_check", "checks:detail")),
            "deposit" => Some(("banking_deposit", "deposits:detail")),
            "transfer" => Some(("transfers_transfer", "transfers:detail")),
            "creditmemo" => Some(("credits_creditmemo", "credits:detail")),
            "creditrefund" => Some(("credits_creditrefund", "credits:refund_detail")),
            "depositrefund" => Some(("prepayments_depositrefund", "prepayments:refund_detail")),
            _ => None,
        };
        if let Some((table, mut route)) = spec {
            let found=tx.query_row(&format!("SELECT i.number FROM {table} d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE d.id=?1 AND d.company_id=?2"),params![raw,c.to_db()],|r|r.get::<_,String>(0)).optional()?;
            if let Some(label) = found {
                if model == "receipt"
                    && tx.query_row("SELECT kind FROM sales_receipt WHERE id=?1", [raw], |r| {
                        r.get::<_, String>(0)
                    })? == "SR"
                {
                    route = "sales_receipts:detail";
                }
                return Ok((label, Some(url(route, &[&c.to_db(), raw]))));
            }
        }
    }
    if model=="numbering" {
        return Ok((format!("{}: {} → {}",fb_core::numbering::label(detail["kind"].as_str().unwrap_or("")),detail["old_next"].as_str().unwrap_or(""),detail["new_next"].as_str().unwrap_or("")),Some(url("company_edit",&[&c.to_db()]))));
    }
    // SIM-FIX: a backup event names its file, which is not a book record.
    if model == "backup" && !raw.is_empty() {
        return Ok((format!("Backup {raw}"), None));
    }
    let label = detail["number"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(str::trim)
        .unwrap_or_else(|| LABELS["TYPE_NOUN"][model].as_str().unwrap_or("Record"));
    Ok((format!("{label} (no longer on file)"), None))
}

fn events(app: &mut App, c: CompanyId) -> AppResult<Vec<Value>> {
    Ok(app.open_mut()?.book.read(|tx|->Result<Vec<Value>,FbError>{
        let mut st=tx.prepare("SELECT id,actor_id,actor_name,occurred_at,action,target,detail FROM ledger_auditevent WHERE company_id=?1 ORDER BY occurred_at,id")?;
        let raw=st.query_map([c.to_db()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?)))?.collect::<Result<Vec<_>,_>>()?;
        raw.into_iter().map(|(id,actor_id,actor_name,occurred_at,code,key,detail)|{
            let detail:Value=fb_db::parse_unique_json(&detail).map_err(|e|FbError::validation(e.to_string()))?;
            let (number,url)=target(tx,c,&key,&detail)?;
            let detail=fb_core::card_numbers::mask_text_fields(&detail);
            let operator=UserId::parse(&actor_id).map(|id|store::user(tx,id)).transpose()?.flatten().map(|u|u.username).filter(|s|!s.trim().is_empty()).unwrap_or_else(||if actor_name.trim().is_empty(){"Unknown user".into()}else{actor_name.trim().into()});
            let stamp=chrono::DateTime::parse_from_rfc3339(&occurred_at).map_err(|e|FbError::validation(e.to_string()))?;
            let zone:chrono_tz::Tz=std::env::var("FB_TIME_ZONE").unwrap_or_else(|_|"America/New_York".into()).parse().map_err(|_|FbError::validation("Unknown display time zone."))?;
            Ok(json!({"id":id,"actor_id":actor_id,"operator":operator,"occurred_at":stamp.format("%Y-%m-%d %H:%M:%S").to_string(),"when":stamp.with_timezone(&zone).format("%Y-%m-%d %H:%M:%S").to_string(),"action":code,"action_label":action(&code),"target":key,"target_label":number,"target_url":url,"number":number,"url":url,"reason":detail["reason"].as_str().unwrap_or("").trim(),"detail":jsonb_repr(&detail)}))
        }).collect()
    })?)
}

// PostgreSQL JSONB returns objects ordered by byte length then bytes. Django prints their Python repr.
fn jsonb_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(b) => {
            if *b {
                "True".into()
            } else {
                "False".into()
            }
        }
        Value::Number(n) => n.to_string(),
        Value::String(s) => {
            let quote = if s.contains('\'') && !s.contains('"') {
                '"'
            } else {
                '\''
            };
            let mut out = quote.to_string();
            for c in s.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if c == quote => {
                        out.push('\\');
                        out.push(c)
                    }
                    c if c.is_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
                    c => out.push(c),
                }
            }
            out.push(quote);
            out
        }
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(jsonb_repr).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort_by(|a, b| a.len().cmp(&b.len()).then(a.as_bytes().cmp(b.as_bytes())));
            format!(
                "{{{}}}",
                keys.iter()
                    .map(|k| format!("{}: {}", jsonb_repr(&json!(k)), jsonb_repr(&map[*k])))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

pub(super) fn journal_page(app: &mut App, route: &Route) -> AppResult<String> {
    let mut vm = ctx(app, route)?;
    let c = arg_company(route)?;
    let j = journal_at(app, route)?.unwrap();
    let (journal,lines,debit,credit)=app.open_mut()?.book.read(|tx|->Result<_,FbError>{
        let journal=journal_vm(tx,&j)?;let mut debit=0i128;let mut credit=0i128;let mut lines=vec![];
        for line in store::lines(tx,j.id)? {
            debit=debit.checked_add(line.debit.cents()).ok_or_else(||FbError::validation("Journal total overflow."))?;
            credit=credit.checked_add(line.credit.cents()).ok_or_else(||FbError::validation("Journal total overflow."))?;
            let a=store::account(tx,line.account_id)?.unwrap();
            lines.push(json!({"account_id":line.account_id.to_db(),"account":account_vm(&a),"debit":line.debit.to_string(),"credit":line.credit.to_string(),"has_debit":line.debit.cents()!=0,"has_credit":line.credit.cents()!=0,"description":line.description,"name":line.name}));
        }
        Ok((journal,lines,debit,credit))
    })?;
    put(&mut vm, "journal", journal);
    put(&mut vm, "lines", lines);
    put(
        &mut vm,
        "totals",
        json!({"debit":cash(debit),"credit":cash(credit),"difference":cash(debit-credit),"unbalanced":debit!=credit}),
    );
    put(
        &mut vm,
        "post_form",
        forms::bind(
            &[
                Field::new("expected_revision", "Expected revision", Widget::Hidden)
                    .value(j.revision.to_string()),
            ],
            &[],
        ),
    );
    put(
        &mut vm,
        "reversal_form",
        forms::bind(
            &[
                Field::new("date", "Date", Widget::Date)
                    .required()
                    .value((app.today)().to_string()),
                Field::new("reason", "Reason", Widget::Text).required(),
            ],
            &[],
        ),
    );
    put(
        &mut vm,
        "post_token",
        token(app, &url("journal_post", &[&c.to_db(), &j.id.to_db()])),
    );
    put(
        &mut vm,
        "reverse_token",
        token(app, &url("journal_reverse", &[&c.to_db(), &j.id.to_db()])),
    );
    let key = format!("journal:{}", j.id.to_db());
    put(
        &mut vm,
        "events",
        events(app, c)?
            .into_iter()
            .filter(|e| e["target"] == key)
            .collect::<Vec<_>>(),
    );
    // The real D9 panel, on manual originals only (the template's own condition).
    if j.manual() && j.reversal_of_id.is_none() {
        let panel = crate::screens::d9::attachments::journal_panel(app, c, j.id, &vm["company"])?;
        put(&mut vm, "attachments_panel", panel);
    }
    draw(app, "ledger/journal.html", &vm)
}

pub(super) fn history(
    app: &mut App,
    route: &Route,
    params: Posted<'_>,
) -> AppResult<(String, u16)> {
    let mut vm = ctx(app, route)?;
    let c = arg_company(route)?;
    let mut errors = Errors::new();
    let start = clean_date(params, "start", false, &mut errors);
    let end = clean_date(params, "end", false, &mut errors);
    if start.as_ref().zip(end.as_ref()).is_some_and(|(s, e)| s > e) {
        error(
            &mut errors,
            "end",
            "The start date must not be after the end date.",
        );
    }
    let mut actions = vec![
        (String::new(), "All actions".into()),
        ("voids".into(), "Voids and reversals".into()),
    ];
    let mut labels: Vec<_> = LABELS["ACTION_LABELS"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
        .chain(sim_fix_labels().map(|(k, v)| (k.to_owned(), v.to_owned())))
        .chain(std::iter::once((DOCUMENT_DETAILS_ACTION.0.to_owned(), DOCUMENT_DETAILS_ACTION.1.to_owned())))
        .collect();
    labels.sort_by(|a, b| a.1.cmp(&b.1));
    actions.extend(labels);
    let mut docs = vec![(String::new(), "All types".into())];
    docs.extend(
        LABELS["DOCUMENT_TYPES"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| (v[0].as_str().unwrap().into(), v[1].as_str().unwrap().into())),
    );
    let members=app.open_mut()?.book.read(|tx|->Result<Vec<(String,String)>,FbError>{
        let mut st=tx.prepare("SELECT u.id,u.username FROM fb_user u JOIN ledger_company_members m ON m.user_id=u.id WHERE m.company_id=?1 ORDER BY u.username COLLATE fb_text")?;
        Ok(st.query_map([c.to_db()],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<Result<_,_>>()?)
    })?;
    let mut users = vec![(String::new(), "All users".into())];
    users.extend(members);
    let mut fields = vec![
        Field::new("start", "From", Widget::Date)
            .value_opt(get(params, "start").map(str::to_owned)),
        Field::new("end", "Through", Widget::Date).value_opt(get(params, "end").map(str::to_owned)),
    ];
    for (name, label, choices) in [
        ("action", "Action", actions),
        ("doc", "Document type", docs),
        ("user", "User", users),
    ] {
        let value = get(params, name).unwrap_or("");
        if !choices.iter().any(|(v, _)| v == value) {
            error(
                &mut errors,
                name,
                format!("Select a valid choice. {value} is not one of the available choices."),
            );
        }
        fields.push(Field::new(name, label, Widget::Select { choices }).value(value));
    }
    let valid = errors.is_empty();
    let mut rows = events(app, c)?;
    rows.reverse();
    let receipt_kind = get(params, "doc")
        .filter(|d| ["receipt", "sales_receipt"].contains(d))
        .map(|d| if d == "receipt" { "RCP" } else { "SR" });
    let receipt_targets = if let Some(kind) = receipt_kind {
        app.open_mut()?
            .book
            .read(|tx| -> Result<std::collections::HashSet<String>, FbError> {
                let mut st =
                    tx.prepare("SELECT id FROM sales_receipt WHERE company_id=?1 AND kind=?2")?;
                Ok(st
                    .query_map(params![c.to_db(), kind], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .map(|id| format!("receipt:{id}"))
                    .collect())
            })?
    } else {
        Default::default()
    };
    if valid {
        rows.retain(|e| {
            let day = e["when"].as_str().unwrap().get(..10).unwrap();
            let code = e["action"].as_str().unwrap();
            let action = get(params, "action").unwrap_or("");
            let doc = get(params, "doc").unwrap_or("");
            let user = get(params, "user").unwrap_or("");
            let prefix = LABELS["DOCUMENT_TYPES"]
                .as_array()
                .unwrap()
                .iter()
                .find(|d| d[0] == doc)
                .and_then(|d| d[2].as_str());
            start.as_deref().is_none_or(|s| day >= s)
                && end.as_deref().is_none_or(|s| day <= s)
                && (action.is_empty()
                    || if action == "voids" {
                        code.ends_with(".voided")
                            || code.ends_with(".reversed")
                            || code.ends_with(".unapplied")
                    } else {
                        code == action
                    })
                && prefix.is_none_or(|p| e["target"].as_str().unwrap().starts_with(p))
                && (receipt_kind.is_none()
                    || receipt_targets.contains(e["target"].as_str().unwrap()))
                && (user.is_empty() || e["actor_id"] == user)
        });
    }
    for row in &mut rows {
        row["action"] = row["action_label"].clone();
    }
    if valid && get(params, "format") == Some("csv") {
        return Ok((history_csv(&rows), 200));
    }
    let page = Page::of(&rows, 25, get(params, "page"));
    put(&mut vm, "rows", &page.object_list);
    put(&mut vm, "page", page);
    put(
        &mut vm,
        "filter_form",
        forms::bind(&decorate(fields, &errors), &nonfield(&errors, "__all__")),
    );
    let query: Vec<_> = params
        .iter()
        .filter(|(k, _)| k != "page" && k != "format")
        .cloned()
        .collect();
    let mut csv = query.clone();
    csv.push(("format".into(), "csv".into()));
    put(&mut vm, "query_string", urlencode(&query));
    put(&mut vm, "csv_query", urlencode(&csv));
    put(
        &mut vm,
        "time_zone",
        std::env::var("FB_TIME_ZONE").unwrap_or_else(|_| "America/New_York".into()),
    );
    put(
        &mut vm,
        "filtered",
        ["start", "end", "action", "doc", "user"]
            .iter()
            .any(|k| get(params, k).is_some_and(|s| !s.is_empty())),
    );
    Ok((draw(app, "ledger/history.html", &vm)?, 200))
}

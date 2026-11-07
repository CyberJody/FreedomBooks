use super::*;
pub fn register_accounts(book: &mut Book, actor: UserId, c: CompanyId) -> FbResult<Vec<Value>> {
    book.read(|q|{services::access(q,actor,c)?;let mut st=q.prepare("SELECT id FROM banking_register_account WHERE company_id=?1 ORDER BY code COLLATE fb_text,id")?;let keys=st.query_map([c.to_db()],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;keys.iter().map(|k|{let a=account(q,c,k)?;Ok(json!({"id":k,"code":a.code,"name":a.name,"kind":a.kind.as_str(),"active":a.active}))}).collect()})
}
pub fn build(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    aid: &str,
    filters: &Value,
    page: i64,
    per_page: usize,
) -> FbResult<Value> {
    book.read(|q| {
        services::access(q, actor, c)?;
        build_query(q, c, aid, filters, page, per_page)
    })
}
pub(super) fn build_query(
    q: &(impl Query + ?Sized),
    c: CompanyId,
    aid: &str,
    f: &Value,
    page: i64,
    per_page: usize,
) -> FbResult<Value> {
    build_selected(q, c, aid, f, page, per_page, None)
}
pub(super) fn build_selected(
    q: &(impl Query + ?Sized),
    c: CompanyId,
    aid: &str,
    f: &Value,
    page: i64,
    per_page: usize,
    selected: Option<&BTreeSet<String>>,
) -> FbResult<Value> {
    let normalized = normalize_filters(f, chrono::Local::now().date_naive())?;
    build_normalized(q, c, aid, &normalized, page, per_page, selected)
}
fn build_normalized(
    q: &(impl Query + ?Sized),
    c: CompanyId,
    aid: &str,
    f: &Value,
    page: i64,
    per_page: usize,
    selected: Option<&BTreeSet<String>>,
) -> FbResult<Value> {
    let a = account(q, c, aid)?;
    if !is_register_account(q, c, aid)? {
        return Err(FbError::validation(
            "Only bank and credit card accounts have a bank register.",
        ));
    }
    if per_page == 0 || (selected.is_none() && per_page > 2000) {
        return Err(FbError::validation("Invalid page size."));
    }
    let start = f["start"].as_str().map(str::to_owned);
    let end = f["end"].as_str().map(str::to_owned);
    let state = s(f, "state")?;
    let payee = s(f, "payee")?;
    let wanted_number = s(f, "number")?;
    let amount = if f["amount"].is_null() {
        None
    } else {
        Some(cents(&f["amount"])?)
    };
    let sources = source_metadata(q, c)?;
    let mut st=q.prepare("SELECT l.id,l.account_id,l.journal_id,l.debit,l.credit,j.date,coalesce(s.state,'U'),coalesce(s.revision,0),l.description,l.name,l.position,j.memo,j.source_id,j.reversal_of_id,z.id,i.number,j.entry_number,j.number,o.entry_number,o.number FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id LEFT JOIN banking_banklinestatus s ON s.journal_line_id=l.id LEFT JOIN documents_documentidentity i ON i.id=j.source_id LEFT JOIN ledger_journal o ON o.id=j.reversal_of_id LEFT JOIN ledger_journal z ON z.reversal_of_id=j.id WHERE l.account_id=?1 AND j.company_id=?2 AND j.status='POSTED' ORDER BY j.date,j.id,l.position,l.id")?;
    let raw=st.query_map(params![aid,c.to_db()],|r|{
        let description:String=r.get(8)?;let jm:String=r.get(11)?;
        let memo=if description.is_empty(){jm}else{description.clone()};
        Ok(json!({"id":r.get::<_,String>(0)?,"account_id":r.get::<_,String>(1)?,"journal_id":r.get::<_,String>(2)?,"debit":cash(r.get(3)?),"credit":cash(r.get(4)?),"date":r.get::<_,String>(5)?,"state":r.get::<_,String>(6)?,"revision":r.get::<_,i64>(7)?,"description":description,"name":r.get::<_,String>(9)?,"position":r.get::<_,i64>(10)?,"memo":memo,"source_id":r.get::<_,Option<String>>(12)?,"reversal_of_id":r.get::<_,Option<String>>(13)?,"voided_by_id":r.get::<_,Option<String>>(14)?,"document_number":r.get::<_,Option<String>>(15)?,"entry_number":r.get::<_,String>(16)?,"journal_number":r.get::<_,i64>(17)?,"original_entry_number":r.get::<_,Option<String>>(18)?,"original_journal_number":r.get::<_,Option<i64>>(19)?}))
    })?.collect::<Result<Vec<_>,_>>()?;
    let mut st=q.prepare("SELECT l.journal_id,l.id,a.code,a.name,l.debit,l.credit,l.description FROM ledger_journalline l JOIN ledger_account a ON a.id=l.account_id JOIN ledger_journal j ON j.id=l.journal_id WHERE j.company_id=?1 AND j.status='POSTED' AND EXISTS(SELECT 1 FROM ledger_journalline b WHERE b.journal_id=j.id AND b.account_id=?2) ORDER BY j.id,l.position,l.id")?;
    let mut splits: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for row in st.query_map(params![c.to_db(),aid],|r|Ok((r.get::<_,String>(0)?,json!({"id":r.get::<_,String>(1)?,"code":r.get::<_,String>(2)?,"name":r.get::<_,String>(3)?,"debit":cash(r.get(4)?),"credit":cash(r.get(5)?),"memo":r.get::<_,String>(6)?}))))? {let(j,row)=row?;splits.entry(j).or_default().push(row);}
    let (mut run, mut opening, mut cleared, mut od, mut oc) = (0, 0, 0, 0, 0);
    let mut rows = vec![];
    let sign = if a.section_balance() == crate::model::Side::Debit {
        1
    } else {
        -1
    };
    for mut row in raw {
        if end
            .as_ref()
            .is_some_and(|d| row["date"].as_str().is_some_and(|rd| rd > d.as_str()))
        {
            continue;
        }
        let debit = cents(&row["debit"])?;
        let credit = cents(&row["credit"])?;
        let net = add(debit, -credit)?;
        run = add(run, net)?;
        if row["state"] == "U" {
            od = add(od, debit)?;
            oc = add(oc, credit)?;
        } else {
            cleared = add(cleared, net)?;
        }
        if start
            .as_ref()
            .is_some_and(|d| row["date"].as_str().is_some_and(|rd| rd < d.as_str()))
        {
            opening = run;
            continue;
        }
        let key = s(&row, "id")?;
        if selected.is_some_and(|ids| !ids.contains(key)) {
            continue;
        }
        if !(state.is_empty() || state == "open" && row["state"] != "R" || row["state"] == state) {
            continue;
        }
        if amount.is_some_and(|n| n != debit && n != credit) {
            continue;
        }
        let reversal = !row["reversal_of_id"].is_null();
        let source = row["source_id"].as_str().and_then(|id| sources.get(id));
        let (label, number, party, search_names, search_numbers) = if row["source_id"].is_string() {
            let doc = s(&row, "document_number")?.to_owned();
            let (party, label, number, search_names) = match source {
                Some(src) => {
                    let names = src["parties"]
                        .as_array()
                        .ok_or_else(|| FbError::validation("Invalid source parties."))?
                        .clone();
                    if names.iter().any(|n| !n.is_string()) {
                        return Err(FbError::validation("Invalid source party."));
                    }
                    (
                        s(src, "party")?.to_owned(),
                        s(src, "label")?.to_owned(),
                        match src.get("ref") {
                            Some(v) => v
                                .as_str()
                                .ok_or_else(|| FbError::validation("Invalid source reference."))?
                                .to_owned(),
                            None => doc.clone(),
                        },
                        names,
                    )
                }
                None => (String::new(), "Document".into(), doc.clone(), vec![]),
            };
            let mut numbers = vec![json!(doc)];
            if let Some(extra) = source.and_then(|s| s["search_number"].as_str()) {
                numbers.push(json!(extra));
            }
            (label, number, party, search_names, numbers)
        } else {
            let display = format!(
                "J{:06}",
                row["journal_number"]
                    .as_i64()
                    .ok_or_else(|| FbError::validation("Invalid journal number."))?
            );
            let entry = s(&row, "entry_number")?;
            let number = if reversal || entry.is_empty() {
                display
            } else {
                entry.into()
            };
            let label = if reversal {
                let entry = s(&row, "original_entry_number")?;
                let original = if entry.is_empty() {
                    format!(
                        "J{:06}",
                        row["original_journal_number"]
                            .as_i64()
                            .ok_or_else(|| FbError::validation("Invalid original journal."))?
                    )
                } else {
                    entry.into()
                };
                format!("General journal {original}")
            } else {
                "General journal".into()
            };
            let party = s(&row, "name")?.to_owned();
            (
                label,
                number.clone(),
                party.clone(),
                vec![json!(party)],
                vec![json!(entry)],
            )
        };
        if !payee.is_empty()
            && !search_names
                .iter()
                .filter_map(Value::as_str)
                .any(|s| fb_db::collation::upper(s).contains(&fb_db::collation::upper(payee)))
        {
            continue;
        }
        if !wanted_number.is_empty()
            && !search_numbers
                .iter()
                .filter_map(Value::as_str)
                .any(|n| fb_db::collation::upper(n) == fb_db::collation::upper(wanted_number))
        {
            continue;
        }
        let others = splits
            .get(s(&row, "journal_id")?)
            .into_iter()
            .flatten()
            .filter(|v| v["id"] != row["id"])
            .map(|v| {
                let mut v = v.clone();
                v.as_object_mut().map(|m| m.remove("id"));
                v
            })
            .collect::<Vec<_>>();
        row["other_count"] = json!(others.len());
        row["more_splits"] = json!(others.len().saturating_sub(20));
        row["other"] = json!(if others.is_empty() {
            String::new()
        } else if others.len() == 1 {
            format!("{} · {}", s(&others[0], "code")?, s(&others[0], "name")?)
        } else {
            "-split-".into()
        });
        row["splits"] = json!(if others.len() > 1 {
            others.into_iter().take(20).collect::<Vec<_>>()
        } else {
            vec![]
        });
        row["left"] = if credit == 0 {
            Value::Null
        } else {
            cash(credit)
        };
        row["right"] = if debit == 0 { Value::Null } else { cash(debit) };
        row["balance"] = cash(signed(run, sign)?);
        row["party"] = json!(party);
        row["number"] = json!(number);
        row["type"] = json!(format!(
            "{}{label}",
            if reversal { "Reversal · " } else { "" }
        ));
        row["reversal"] = json!(reversal);
        row["source"] = source.cloned().unwrap_or(Value::Null);
        rows.push(row);
    }
    let total = rows.len();
    let last = total.div_ceil(per_page).max(1);
    let page = usize::try_from(page)
        .ok()
        .filter(|n| *n >= 1 && *n <= last)
        .unwrap_or(last);
    let rows = rows
        .into_iter()
        .skip((page - 1) * per_page)
        .take(per_page)
        .collect::<Vec<_>>();
    Ok(
        json!({"account_id":aid,"rows":rows,"page":page,"total":total,"opening":cash(signed(opening,sign)?),"closing":cash(signed(run,sign)?),"totals":{"cleared":cash(signed(cleared,sign)?),"ending":cash(signed(run,sign)?),"left_open":cash(oc),"right_open":cash(od)},"labels":if sign==1{json!({"card":false,"left":"Payment","right":"Deposit","left_open":"Uncleared payments","right_open":"Uncleared deposits"})}else{json!({"card":true,"left":"Charge","right":"Payment","left_open":"Uncleared charges","right_open":"Uncleared payments"})}}),
    )
}

/// Bulk provenance reads keep register query count independent of its row count.
pub fn source_metadata(q: &(impl Query + ?Sized), c: CompanyId) -> FbResult<BTreeMap<String, Value>> {
    let mut out = BTreeMap::new();
    for (sql, party, label) in [
        (
            "SELECT identity_id,id,data FROM sales_receipt WHERE company_id=?1",
            "customer_snapshot",
            "Receipt",
        ),
        (
            "SELECT i.identity_id,i.id,r.data FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.number=i.revision WHERE i.company_id=?1",
            "customer_snapshot",
            "Invoice",
        ),
        (
            "SELECT i.identity_id,i.id,r.vendor_snapshot FROM payables_bill i JOIN payables_billrevision r ON r.bill_id=i.id AND r.number=i.revision WHERE i.company_id=?1",
            "",
            "Bill",
        ),
        (
            "SELECT identity_id,id,data FROM credits_creditmemo WHERE company_id=?1",
            "customer_snapshot",
            "Credit memo",
        ),
    ] {
        let mut st = q.prepare(sql)?;
        for row in st.query_map([c.to_db()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (identity, id, raw) = row?;
            let data: Value = fb_db::parse_unique_json_as(&raw)
                .map_err(|_| FbError::validation("Invalid source evidence."))?;
            let snapshot = if party.is_empty() {
                &data
            } else {
                &data[party]
            };
            let name = s(snapshot, "display_name")?;
            let label = if label == "Receipt" && data["kind"] == "SR" {
                "Sales receipt"
            } else if label == "Receipt" && data["kind"] == "CDP" {
                "Customer deposit"
            } else {
                label
            };
            let mut metadata = json!({"identity_id":identity,"document_id":id,"label":label,"party":name,"parties":[name]});
            if label == "Credit memo" { metadata["parties"] = json!([]); }
            if label == "Customer deposit" {
                let reference = s(&data, "reference")?;
                metadata["search_number"] = json!(reference);
                if !reference.is_empty() { metadata["ref"] = json!(reference); }
            }
            out.insert(identity, metadata);
        }
    }
    let mut st=q.prepare("SELECT d.identity_id,d.id,count(l.id),json_group_array(json_extract(r.data,'$.customer_snapshot.display_name')) FROM banking_deposit d LEFT JOIN banking_depositline l ON l.deposit_id=d.id LEFT JOIN sales_receipt r ON r.id=l.receipt_id WHERE d.company_id=?1 GROUP BY d.id ORDER BY d.id")?;
    for row in st.query_map([c.to_db()], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, String>(3)?,
        ))
    })? {
        let (identity, id, n, raw) = row?;
        let names: Value = fb_db::parse_unique_json_as(&raw)
            .map_err(|_| FbError::validation("Invalid deposit names."))?;
        let name = if n == 1 {
            names[0]
                .as_str()
                .ok_or_else(|| FbError::validation("Invalid deposit party."))?
                .to_owned()
        } else {
            format!("{n} receipts")
        };
        out.insert(identity.clone(),json!({"identity_id":identity,"document_id":id,"label":"Deposit","party":name,"parties":names}));
    }
    let mut st=q.prepare("SELECT identity_id,id,vendor_snapshot,method,reference FROM payables_billpayment WHERE company_id=?1")?;
    for row in st.query_map([c.to_db()], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    })? {
        let (identity, id, raw, method, reference) = row?;
        let party: Value = fb_db::parse_unique_json_as(&raw)
            .map_err(|_| FbError::validation("Invalid payment party."))?;
        let name = s(&party, "display_name")?;
        let numbered = method == "CHECK"
            && (1..=20).contains(&reference.len())
            && reference.bytes().all(|b| b.is_ascii_digit());
        let mut value = json!({"identity_id":identity,"document_id":id,"label":"Bill payment","party":name,"parties":[name]});
        if numbered {
            value["ref"] = json!(reference);
        }
        if method == "CHECK" {
            value["search_number"] = json!(reference);
        }
        out.insert(identity, value);
    }
    // Deposit applications have display provenance, but no party-search provider in Python.
    let mut st=q.prepare("SELECT a.identity_id,a.id,r.data,di.number,ii.number FROM prepayments_depositapplication a JOIN sales_receipt r ON r.id=a.deposit_id JOIN documents_documentidentity di ON di.id=r.identity_id JOIN sales_invoice i ON i.id=a.invoice_id JOIN documents_documentidentity ii ON ii.id=i.identity_id WHERE a.company_id=?1")?;
    for row in st.query_map([c.to_db()], |r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?)))? {
        let (identity,id,raw,deposit,invoice)=row?;
        let receipt=fb_db::parse_unique_json(&raw).map_err(|_| FbError::validation("Invalid deposit evidence."))?;
        let party=format!("{} ({deposit} to {invoice})",s(&receipt["customer_snapshot"],"display_name")?);
        out.insert(identity.clone(),json!({"identity_id":identity,"document_id":id,"label":"Deposit application","party":party,"parties":[]}));
    }
    // Frozen module provenance, in bulk for the whole register snapshot.
    for (sql, label, parent_kind) in [
        ("SELECT r.identity_id,r.id,r.data,i.number FROM credits_creditrefund r JOIN credits_creditmemo m ON m.id=r.credit_memo_id JOIN documents_documentidentity i ON i.id=m.identity_id WHERE r.company_id=?1", "Refund", "credit memo"),
        ("SELECT r.identity_id,r.id,r.data,i.number FROM prepayments_depositrefund r JOIN sales_receipt m ON m.id=r.deposit_id JOIN documents_documentidentity i ON i.id=m.identity_id WHERE r.company_id=?1", "Deposit refund", "customer deposit"),
    ] {
        let mut st=q.prepare(sql)?;
        for row in st.query_map([c.to_db()], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))? {
            let (identity,id,raw,parent)=row?;
            let data=fb_db::parse_unique_json(&raw).map_err(|_| FbError::validation("Invalid refund evidence."))?;
            let name=s(&data["customer_snapshot"],"display_name")?;
            let number=s(&data,"check_number")?;
            let suffix=if number.is_empty(){String::new()}else{format!(", check {number}")};
            let party=format!("to {name} ({parent_kind} {parent}{suffix})");
            let mut metadata=json!({"identity_id":identity,"document_id":id,"label":label,"party":party,"parties":[name],"search_number":number});
            if !number.is_empty(){metadata["ref"]=json!(number);}
            out.insert(identity,metadata);
        }
    }
    let mut st=q.prepare("SELECT i.identity_id,i.id,r.check_number,r.print_later,r.payee_name FROM checks_check i JOIN checks_checkrevision r ON r.document_id=i.id AND r.number=i.revision WHERE i.company_id=?1")?;
    for row in st.query_map([c.to_db()], |r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,bool>(3)?,r.get::<_,String>(4)?)))? {
        let (identity,id,number,later,name)=row?;
        let display=if number.is_empty(){if later{"To print"}else{"No number"}}else{&number};
        out.insert(identity.clone(),json!({"identity_id":identity,"document_id":id,"label":"Check","party":name,"parties":[name],"ref":display,"search_number":number}));
    }
    let mut st=q.prepare("SELECT identity_id,id,from_snapshot,to_snapshot FROM transfers_transfer WHERE company_id=?1")?;
    for row in st.query_map([c.to_db()], |r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))? {
        let (identity,id,from,to)=row?;
        let from=fb_db::parse_unique_json(&from).map_err(|_| FbError::validation("Invalid transfer evidence."))?;
        let to=fb_db::parse_unique_json(&to).map_err(|_| FbError::validation("Invalid transfer evidence."))?;
        let party=format!("from {} · {} to {} · {}",s(&from,"code")?,s(&from,"name")?,s(&to,"code")?,s(&to,"name")?);
        out.insert(identity.clone(),json!({"identity_id":identity,"document_id":id,"label":"Transfer","party":party,"parties":[from["code"],from["name"],to["code"],to["name"]]}));
    }
    Ok(out)
}

/// Register form entry point with an explicit clock, for reproducible callers.
#[allow(clippy::too_many_arguments)]
pub fn build_on(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    aid: &str,
    filters: &Value,
    page: i64,
    per_page: usize,
    today: &str,
) -> FbResult<Value> {
    let today = py::model_date(today).map_err(FbError::validation)?;
    let filters = normalize_filters(filters, today)?;
    book.read(|q| {
        services::access(q, actor, c)?;
        build_normalized(q, c, aid, &filters, page, per_page, None)
    })
}
fn normalize_filters(f: &Value, today: chrono::NaiveDate) -> FbResult<Value> {
    use chrono::{Datelike, Duration, Months, NaiveDate};
    let invalid = || FbError::validation("Invalid register filters.");
    if !f.is_object() {
        return Err(invalid());
    }
    let empty = |v: &Value| {
        v.is_null()
            || v.as_str() == Some("")
            || v.as_array().is_some_and(Vec::is_empty)
            || v.as_object().is_some_and(serde_json::Map::is_empty)
    };
    let optional = |key: &str, max: usize| -> FbResult<String> {
        match f.get(key) {
            None => Ok(String::new()),
            Some(v) if empty(v) => Ok(String::new()),
            Some(v) => {
                let raw = match v {
                    Value::String(s) => s.clone(),
                    Value::Bool(true) => "True".into(),
                    Value::Bool(false) => "False".into(),
                    _ => v.to_string(),
                };
                // Django CharField's ProhibitNullCharactersValidator applies after stripping.
                if raw.contains('\0') {
                    return Err(invalid());
                }
                text(&json!(raw), max, false).map_err(|_| invalid())
            }
        }
    };
    // ChoiceField does not strip input; CharField and DateField do.
    let choice = |key: &str| -> FbResult<String> {
        match f.get(key) {
            None => Ok(String::new()),
            Some(v) if empty(v) => Ok(String::new()),
            Some(Value::String(s)) => Ok(s.clone()),
            Some(v) => Ok(v.to_string()),
        }
    };
    let preset = choice("preset")?;
    if ![
        "",
        "custom",
        "this_month",
        "last_month",
        "this_quarter",
        "this_year",
        "year_to_date",
    ]
    .contains(&preset.as_str())
    {
        return Err(invalid());
    }
    let state = choice("state")?;
    if !["", "U", "C", "R", "open"].contains(&state.as_str()) {
        return Err(invalid());
    }
    if !["", "print", "csv"].contains(&choice("output")?.as_str()) {
        return Err(invalid());
    }
    let parse_date = |key: &str| -> FbResult<Option<String>> {
        let s = optional(key, 100)?;
        if s.is_empty() {
            Ok(None)
        } else {
            statement_date(&json!(s)).map(Some).map_err(|_| invalid())
        }
    };
    let mut start = parse_date("start")?;
    let mut end = parse_date("end")?;
    if !preset.is_empty() && preset != "custom" {
        let date = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).ok_or_else(invalid);
        let month = date(today.year(), today.month(), 1)?;
        let year = date(today.year(), 1, 1)?;
        let month_end = |d: NaiveDate| {
            d.checked_add_months(Months::new(1))
                .and_then(|d| d.checked_sub_signed(Duration::days(1)))
                .ok_or_else(invalid)
        };
        let (a, b) = match preset.as_str() {
            "this_month" => (month, month_end(month)?),
            "last_month" => {
                let b = month
                    .checked_sub_signed(Duration::days(1))
                    .ok_or_else(invalid)?;
                (date(b.year(), b.month(), 1)?, b)
            }
            "this_quarter" => {
                let a = date(today.year(), 3 * ((today.month() - 1) / 3) + 1, 1)?;
                let b = a
                    .checked_add_months(Months::new(3))
                    .and_then(|d| d.checked_sub_signed(Duration::days(1)))
                    .ok_or_else(invalid)?;
                (a, b)
            }
            "this_year" => (year, date(today.year(), 12, 31)?),
            "year_to_date" => (year, today),
            _ => return Err(invalid()),
        };
        start = Some(a.to_string());
        end = Some(b.to_string());
    } else if start.as_ref().zip(end.as_ref()).is_some_and(|(a, b)| a > b) {
        return Err(invalid());
    }
    let raw = optional("amount", 20)?.replace(',', "");
    let raw = raw.trim_start_matches('$');
    let amount = if raw.is_empty() {
        None
    } else {
        {
            let n = cents(&json!(raw)).map_err(|_| invalid())?;
            if n <= 0 {
                return Err(invalid());
            }
            Some(n)
        }
    };
    Ok(
        json!({"preset":"custom","start":start,"end":end,"amount":amount.map(cash),"state":state,"payee":optional("payee",120)?,"number":optional("number",30)?}),
    )
}

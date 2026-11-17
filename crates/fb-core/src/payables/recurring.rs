//! Anchored calendar schedules. Each occurrence, bill, posting and audit is one Writer transaction.
use super::*;
use chrono::{Datelike, Duration, Months, NaiveDate};
use sha2::{Digest, Sha256};

/// Hard limits, enforced here for every caller (docs/recurring-bills.md). A first bill date further
/// back than this, or a backlog larger than this, is a typing mistake, a wrong system clock or a
/// crafted request. Nothing beyond them is created or skipped without a person choosing again.
pub const MAX_START_MONTHS_BACK: u32 = 24;
pub const MAX_OCCURRENCES_PER_RUN: usize = 36;
/// How far a resume decision counts missed dates. Only the newest `MAX_OCCURRENCES_PER_RUN` can be
/// created; older ones can only be skipped, at most `MAX_OCCURRENCES_PER_RUN` per decision.
pub const BACKLOG_LOOK_AHEAD: usize = 2 * MAX_OCCURRENCES_PER_RUN;

fn calendar(raw: &str) -> FbResult<NaiveDate> {
    let iso = date(&json!(raw))?;
    NaiveDate::parse_from_str(&iso, "%Y-%m-%d").map_err(|_| err("Enter a valid date."))
}
/// Index zero is the start date. Always clamp against the original anchor, never the prior date.
pub fn occurrence(start: &str, frequency: &str, index: i64) -> FbResult<Option<String>> {
    calendar(start)?;
    if index < 0 {
        return Err(err("Invalid occurrence index."));
    }
    if !["MONTHLY", "YEARLY"].contains(&frequency) {
        return Err(err("Choose monthly or yearly."));
    }
    Ok(fb_db::dates::recurring_occurrence(start, frequency, index))
}
fn writable(q: &(impl Query + ?Sized), actor: UserId, co: CompanyId) -> FbResult<store::User> {
    let u = services::access(q, actor, co)?;
    let role: Option<String> = q
        .query_row(
            "SELECT role FROM desktop_user WHERE user_id=?",
            [actor.to_db()],
            |r| r.get(0),
        )
        .optional()?;
    if role.as_deref() == Some("readonly") {
        return Err(FbError::denied(
            "Read-only users cannot manage or generate recurring bills.",
        ));
    }
    Ok(u)
}
fn get(q: &(impl Query + ?Sized), co: CompanyId, tid: &str) -> FbResult<Value> {
    let mut v = rows(
        q,
        "SELECT * FROM payables_recurring WHERE company_id=? AND id=?",
        &[&co.to_db(), &tid],
    )?
    .pop()
    .ok_or_else(|| FbError::missing("Recurring bill"))?;
    v["data"] =
        fb_db::parse_unique_json(s(&v, "data")).map_err(|_| err("Invalid recurring bill data."))?;
    Ok(v)
}
fn next(t: &Value) -> FbResult<Option<String>> {
    let n = stored_i64(t, "next_index")?;
    if t["occurrence_limit"]
        .as_i64()
        .is_some_and(|limit| n >= limit)
    {
        return Ok(None);
    }
    Ok(occurrence(s(t, "start_date"), s(t, "frequency"), n)?
        .filter(|d| t["end_date"].as_str().is_none_or(|end| d.as_str() <= end)))
}
/// The oldest pending dates through `through`, at most `limit`, and whether more remain. The same
/// preview is compared inside the write transaction, so stale confirmations cannot act.
fn pending_dates(t: &Value, through: &str, limit: usize) -> FbResult<(Vec<String>, bool)> {
    let through = calendar(through)?.to_string();
    let mut probe = t.clone();
    let mut dates = vec![];
    while let Some(d) = next(&probe)?.filter(|d| d <= &through) {
        if dates.len() == limit {
            return Ok((dates, true));
        }
        dates.push(d);
        probe["next_index"] = json!(stored_i64(&probe, "next_index")? + 1);
    }
    Ok((dates, false))
}
fn confirmation_preview(t: &Value, dates: Vec<String>, more: bool, source_warning: Value) -> Value {
    let auto_post = t["auto_post"] == 1 || t["auto_post"] == true;
    let binding = json!({"identity":t["id"].as_str().or(t["request_key"].as_str()),"revision":t["revision"].as_i64().or(t["expected_revision"].as_i64()),"next_index":t["next_index"],"frequency":t["frequency"],"start_date":t["start_date"],"end_date":t["end_date"],"occurrence_limit":t["occurrence_limit"],"source_date":t["source_date"],"auto_post":auto_post,"dates":dates,"more":more,"source_warning":source_warning});
    let token = format!("{:x}", Sha256::digest(binding.to_string().as_bytes()));
    let display_dates = if dates.len() > 12 {
        dates[..6]
            .iter()
            .chain(&dates[dates.len() - 6..])
            .collect::<Vec<_>>()
    } else {
        dates.iter().collect()
    };
    json!({"token":token,"count":dates.len(),"first_date":dates.first(),"last_date":dates.last(),"display_dates":display_dates,"omitted_count":dates.len().saturating_sub(12),"dates":dates,"more":more,"auto_post":auto_post,"source_warning":source_warning})
}
/// The resume decision. With `MAX_OCCURRENCES_PER_RUN` or fewer missed dates it lists them all for
/// skip or create. With more, `more` is true and it lists only the excess that must be skipped
/// first: the oldest `min(MAX_OCCURRENCES_PER_RUN, missed - MAX_OCCURRENCES_PER_RUN)` dates, so the
/// newest `MAX_OCCURRENCES_PER_RUN` always reach the normal choice. `total` is null when more than
/// `BACKLOG_LOOK_AHEAD` dates were missed.
pub fn due_confirmation(t: &Value, through: &str) -> FbResult<Value> {
    let (mut dates, beyond) = pending_dates(t, through, BACKLOG_LOOK_AHEAD)?;
    let total = dates.len();
    let more = beyond || total > MAX_OCCURRENCES_PER_RUN;
    if more {
        dates.truncate(if beyond {
            MAX_OCCURRENCES_PER_RUN
        } else {
            total - MAX_OCCURRENCES_PER_RUN
        });
    }
    let mut preview = confirmation_preview(t, dates, more, Value::Null);
    preview["total"] = if beyond { Value::Null } else { json!(total) };
    preview["limit"] = json!(MAX_OCCURRENCES_PER_RUN);
    preview["look_ahead"] = json!(BACKLOG_LOOK_AHEAD);
    Ok(preview)
}
/// An active template with more dates due than one run creates: nothing is generated until a person
/// pauses and resumes it.
pub fn stalled(t: &Value, through: &str) -> FbResult<bool> {
    Ok(t["active"] == 1
        && t["deleted"] != 1
        && pending_dates(t, through, MAX_OCCURRENCES_PER_RUN)?.1)
}
fn require_confirmation(preview: &Value, confirmed: &Value) -> FbResult<()> {
    if (preview["count"] != 0 || !preview["source_warning"].is_null())
        && &preview["token"] != confirmed
    {
        return Err(err(
            "Review and confirm the pending bill dates before continuing.",
        ));
    }
    Ok(())
}
fn normalized_schedule(p: &Value) -> FbResult<Value> {
    let mut probe = p.clone();
    let start = date(&p["start_date"])?;
    occurrence(&start, s(p, "frequency"), 0)?;
    let end = if p["end_date"].is_null() {
        None
    } else {
        Some(date(&p["end_date"])?)
    };
    if end.as_ref().is_some_and(|end| end < &start) {
        return Err(err("End date cannot precede the start date."));
    }
    let limit = if p["occurrence_limit"].is_null() {
        None
    } else {
        Some(
            p["occurrence_limit"]
                .as_i64()
                .filter(|limit| (1..=120000).contains(limit))
                .ok_or_else(|| err("Enter an occurrence count from 1 to 120000."))?,
        )
    };
    probe["start_date"] = json!(start);
    probe["end_date"] = json!(end);
    probe["occurrence_limit"] = json!(limit);
    Ok(probe)
}
/// `old_start` is the stored first bill date when editing; a new or changed date must be recent.
fn save_preview(t: &Value, old_start: Option<&str>, through: &str) -> FbResult<Value> {
    let start = s(t, "start_date");
    if old_start != Some(start)
        && calendar(through)?
            .checked_sub_months(Months::new(MAX_START_MONTHS_BACK))
            .is_some_and(|earliest| start < earliest.to_string().as_str())
    {
        return Err(err(&format!(
            "The first bill date cannot be more than {MAX_START_MONTHS_BACK} months before today. Enter older bills as ordinary bills."
        )));
    }
    let mut warning = Value::Null;
    if let Some(source) = t["source_date"].as_str() {
        if s(t, "start_date") <= source {
            return Err(err(
                "The first recurring bill must be after the source bill date.",
            ));
        }
        if stored_i64(t, "next_index")? == 0
            && occurrence(source, s(t, "frequency"), 1)?
                .is_none_or(|first| s(t, "start_date") < first.as_str())
        {
            warning = json!({"source_date":source,"first_date":t["start_date"]});
        }
    }
    let (dates, more) = if t["active"] == true {
        pending_dates(t, through, MAX_OCCURRENCES_PER_RUN)?
    } else {
        (vec![], false)
    };
    if more {
        return Err(err(&format!(
            "This template has more than {MAX_OCCURRENCES_PER_RUN} missed bill dates. Pause it, then use Resume before editing: the oldest dates beyond the newest {MAX_OCCURRENCES_PER_RUN} must be skipped, and the rest can be skipped or created."
        )));
    }
    Ok(confirmation_preview(t, dates, false, warning))
}
/// Read-only save preview. Saving repeats this calculation after checking the revision.
pub fn save_confirmation(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    p: &Value,
    through: &str,
) -> FbResult<Value> {
    book.read(|q| {
        writable(q, actor, co)?;
        let mut t = normalized_schedule(p)?;
        let old = p["id"].as_str().map(|tid| get(q, co, tid)).transpose()?;
        t["next_index"] = if let Some(old) = &old {
            old["next_index"].clone()
        } else {
            json!(0)
        };
        t["source_date"] = if let Some(old) = &old {
            old["source_date"].clone()
        } else if let Some(bid) = id(&p["source_bill_id"], true)? {
            bill(q, co, &bid)?["current"]["date"].clone()
        } else {
            Value::Null
        };
        save_preview(&t, old.as_ref().map(|old| s(old, "start_date")), through)
    })
}
/// Includes deleted templates so historical bill links remain useful.
pub fn detail(book: &mut Book, actor: UserId, co: CompanyId, tid: &str) -> FbResult<Value> {
    book.read(|q| {
        services::access(q, actor, co)?;
        let mut t = get(q, co, tid)?;
        t["next_date"] = json!(next(&t)?);
        t["occurrences"] = json!(rows(q, "SELECT o.occurrence_date,o.occurrence_index,o.bill_id,o.skipped,b.state,i.number FROM payables_recurring_occurrence o LEFT JOIN payables_bill b ON b.id=o.bill_id LEFT JOIN documents_documentidentity i ON i.id=b.identity_id WHERE o.company_id=? AND o.template_id=? ORDER BY o.occurrence_index DESC", &[&co.to_db(), &tid])?);
        let mut upcoming = vec![];
        for n in stored_i64(&t, "next_index")?..stored_i64(&t, "next_index")? + 12 {
            let mut probe = t.clone(); probe["next_index"] = json!(n);
            let Some(d) = next(&probe)? else { break };
            upcoming.push(d);
        }
        t["upcoming"] = json!(upcoming);
        Ok(t)
    })
}
pub fn list(book: &mut Book, actor: UserId, co: CompanyId) -> FbResult<Vec<Value>> {
    book.read(|q| {
        services::access(q, actor, co)?;
        let ids = rows(
            q,
            "SELECT id FROM payables_recurring WHERE company_id=? AND deleted=0 ORDER BY name,id",
            &[&co.to_db()],
        )?;
        ids.into_iter()
            .map(|id| {
                let mut t = get(q, co, s(&id, "id"))?;
                t["next_date"] = json!(next(&t)?);
                Ok(t)
            })
            .collect()
    })
}
/// Copy editable values only: supplier references and correction links belong to the original bill.
/// The suggested first date follows the source bill by whole months and is never older than `save` accepts.
pub fn from_bill(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    bid: &str,
    through: &str,
) -> FbResult<Value> {
    let b = get_bill(book, actor, co, bid)?;
    if b["state"] == "VOID" {
        return Err(err("A voided bill cannot become a recurring template."));
    }
    let r = &b["current"];
    let source = calendar(s(r, "date"))?;
    let terms = (calendar(s(r, "due_date"))? - source).num_days();
    let mut start = occurrence(s(r, "date"), "MONTHLY", 1)?;
    if let Some(earliest) =
        calendar(through)?.checked_sub_months(Months::new(MAX_START_MONTHS_BACK))
    {
        let months = i64::from(earliest.year() - source.year()) * 12 + i64::from(earliest.month())
            - i64::from(source.month());
        let earliest = earliest.to_string();
        for index in [months, months + 1] {
            if start.as_ref().is_some_and(|d| *d < earliest) && index > 1 {
                start = occurrence(s(r, "date"), "MONTHLY", index)?;
            }
        }
    }
    Ok(
        json!({"source_bill_id":bid,"source_date":r["date"],"start_date":start,"vendor_id":r["vendor_id"],"memo":r["memo"],"terms_days":terms,"lines":r["lines"].as_array().ok_or_else(||err("Invalid bill lines."))?.iter().map(|l|json!({"item_id":l["item_id"],"expense_account_id":l["expense_account_id"],"description":l["description"],"quantity":l["quantity"],"rate":l["rate"]})).collect::<Vec<_>>()}),
    )
}
pub fn save(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    p: &Value,
    through: &str,
) -> FbResult<Value> {
    calendar(through)?;
    write(book, |tx| {
        let u = writable(tx, actor, co)?;
        let tid = match id(&p["id"], true)? {
            Some(id) => id,
            None => text(&p["request_key"], 36, true)?,
        };
        id(&json!(tid), false)?;
        let old = rows(
            tx,
            "SELECT id FROM payables_recurring WHERE company_id=? AND id=?",
            &[&co.to_db(), &tid],
        )?
        .pop()
        .map(|_| get(tx, co, &tid))
        .transpose()?;
        let expected = p["expected_revision"]
            .as_i64()
            .ok_or_else(|| err("Reopen this template before saving."))?;
        documents::check_revision(
            old.as_ref()
                .map_or(0, |v| v["revision"].as_i64().unwrap_or(-1)),
            Some(expected),
        )?;
        if old.as_ref().is_some_and(|v| v["deleted"] == 1) {
            return Err(err("This template was deleted."));
        }
        let (source_bill_id, source_date) = if let Some(old) = &old {
            (
                old["source_bill_id"].as_str().map(str::to_owned),
                old["source_date"].as_str().map(str::to_owned),
            )
        } else if let Some(bid) = id(&p["source_bill_id"], true)? {
            let source = bill(tx, co, &bid)?;
            if source["state"] == "VOID" {
                return Err(err("A voided bill cannot become a recurring template."));
            }
            (Some(bid), Some(s(&source["current"], "date").to_owned()))
        } else {
            (None, None)
        };
        let name = text(&p["name"], 120, true)?;
        let mut probe = normalized_schedule(p)?;
        let start = s(&probe, "start_date").to_owned();
        let frequency = text(&p["frequency"], 10, true)?;
        let end = probe["end_date"].as_str().map(str::to_owned);
        let limit = probe["occurrence_limit"].as_i64();
        let n = old
            .as_ref()
            .map_or(0, |v| v["next_index"].as_i64().unwrap_or(0));
        if n > 0
            && old
                .as_ref()
                .is_some_and(|v| v["start_date"] != start || v["frequency"] != frequency)
        {
            return Err(err(
                "After periods are recorded, keep the start date and frequency. Create a new template to change the schedule.",
            ));
        }
        if limit.is_some_and(|limit| limit < n) {
            return Err(err(
                "Count cannot be less than the periods already recorded.",
            ));
        }
        let active = flag(p, "active")?;
        let auto = flag(p, "auto_post")?;
        if old
            .as_ref()
            .is_some_and(|old| old["active"] != i64::from(active))
        {
            return Err(err(
                "Use Pause or Resume on the template to change its active state.",
            ));
        }
        probe["next_index"] = json!(n);
        probe["source_date"] = json!(source_date);
        let preview = save_preview(&probe, old.as_ref().map(|old| s(old, "start_date")), through)?;
        require_confirmation(&preview, &p["confirmation"])?;
        let vendor_id = id(&p["vendor_id"], false)?.unwrap();
        if directory::get(tx, co, Model::Vendor, &vendor_id)?["active"] != true {
            return Err(err("Choose an active vendor from this company."));
        }
        let memo = field_text(p, "memo", 2000)?;
        let terms = p["terms_days"]
            .as_i64()
            .filter(|n| (0..=36500).contains(n))
            .ok_or_else(|| err("Enter terms from 0 to 36500 days."))?;
        let input = p["lines"]
            .as_array()
            .filter(|ls| (1..=50).contains(&ls.len()))
            .ok_or_else(|| err("Enter between 1 and 50 lines."))?;
        let mut lines = vec![];
        for l in input {
            let item = id(&l["item_id"], true)?;
            if let Some(id) = &item
                && directory::get(tx, co, Model::Item, id)?["active"] != true
            {
                return Err(err("Choose an active item from this company."));
            }
            let expense = id(&l["expense_account_id"], true)?;
            if let Some(id) = &expense {
                let a = account(tx, co, id)?;
                if let Some(e) = documents::line_account_error(tx, co, Some(&a), None, false)? {
                    return Err(err(&e));
                }
            }
            if auto && expense.is_none() {
                return Err(err(
                    "Automatic posting requires an expense account on every line.",
                ));
            }
            lines.push(json!({"item_id":item,"expense_account_id":expense,"description":text(&l["description"],1000,true)?,"quantity":calculations::quantity(&l["quantity"])?.to_string(),"rate":calculations::unit_rate(&l["rate"])?.to_string()}));
        }
        let total = calculations::totals(
            &lines
                .iter()
                .map(|v| {
                    let mut v = v.clone();
                    v["taxable"] = json!(false);
                    v
                })
                .collect::<Vec<_>>(),
            "NONE",
            None,
            &json!(""),
            None,
            false,
        )?;
        if auto && cents(&total["total"])? == 0 {
            return Err(err("A zero-total template cannot post automatically."));
        }
        let data = json!({"vendor_id":vendor_id,"memo":memo,"lines":lines});
        if old.is_some() {
            tx.execute("UPDATE payables_recurring SET revision=revision+1,name=?,frequency=?,start_date=?,end_date=?,occurrence_limit=?,terms_days=?,active=?,auto_post=?,data=? WHERE company_id=? AND id=?",params![name,frequency,start,end,limit,terms,active,auto,data.to_string(),co.to_db(),tid])?;
        } else {
            tx.execute("INSERT INTO payables_recurring(id,company_id,name,frequency,start_date,end_date,occurrence_limit,terms_days,active,auto_post,data,revision,next_index,deleted,source_bill_id,source_date) VALUES(?,?,?,?,?,?,?,?,?,?,?,1,0,0,?,?)",params![tid,co.to_db(),name,frequency,start,end,limit,terms,active,auto,data.to_string(),source_bill_id,source_date])?;
        }
        let after = get(tx, co, &tid)?;
        services::audit(
            tx,
            &u,
            co,
            if old.is_some() {
                "recurring_bill.updated"
            } else {
                "recurring_bill.created"
            },
            format!("recurring_bill:{tid}"),
            json!({"before":old,"after":after}),
        )?;
        Ok(after)
    })
}
/// Resume choices and skipped evidence commit atomically before any generator can run.
pub fn control(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    tid: &str,
    p: &Value,
    through: &str,
) -> FbResult<()> {
    calendar(through)?;
    write(book, |tx| {
        let u = writable(tx, actor, co)?;
        let t = get(tx, co, tid)?;
        documents::check_revision(stored_i64(&t, "revision")?, p["expected_revision"].as_i64())?;
        if t["deleted"] == 1 {
            return Err(err("This template was deleted."));
        }
        let action = s(p, "action");
        let mut skipped = vec![];
        let mut more = false;
        let (active, deleted) = match action {
            "pause" if t["active"] == 1 => (false, false),
            "resume" | "resume_create" | "resume_skip" if t["active"] == 0 => {
                let preview = due_confirmation(&t, through)?;
                more = preview["more"] == true;
                if !preview["dates"].as_array().unwrap().is_empty() {
                    if action == "resume" {
                        return Err(err("Choose whether to skip or create the missed bills."));
                    }
                    if more && action == "resume_create" {
                        return Err(err(&format!(
                            "More than {MAX_OCCURRENCES_PER_RUN} bill dates were missed. Skip the oldest {} first; the newest {MAX_OCCURRENCES_PER_RUN} can then be skipped or created.",
                            preview["count"]
                        )));
                    }
                    require_confirmation(&preview, &p["confirmation"])?;
                }
                if action == "resume_skip" {
                    for (offset, dt) in preview["dates"].as_array().unwrap().iter().enumerate() {
                        let n = stored_i64(&t, "next_index")? + offset as i64;
                        tx.execute("INSERT INTO payables_recurring_occurrence(company_id,template_id,occurrence_index,occurrence_date,bill_id,skipped) VALUES(?,?,?,?,NULL,1)",params![co.to_db(),tid,n,dt.as_str().unwrap()])?;
                        services::audit(
                            tx,
                            &u,
                            co,
                            "recurring_bill.skipped",
                            format!("recurring_bill:{tid}"),
                            json!({"template_id":tid,"template_revision":t["revision"],"occurrence_index":n,"occurrence_date":dt,"reason":"Skipped on resume","automatic":false}),
                        )?;
                        tx.execute("UPDATE payables_recurring SET next_index=next_index+1 WHERE company_id=? AND id=?",params![co.to_db(),tid])?;
                        skipped.push(dt.clone());
                    }
                }
                // Stays paused while further missed dates still need a decision.
                (!more, false)
            }
            "delete" => {
                if p["confirm_delete"] != true {
                    return Err(err("Confirm deletion of this recurring template."));
                }
                (false, true)
            }
            _ => return Err(err("Invalid recurring bill action.")),
        };
        tx.execute("UPDATE payables_recurring SET revision=revision+1,active=?,deleted=? WHERE company_id=? AND id=?",params![active,deleted,co.to_db(),tid])?;
        services::audit(
            tx,
            &u,
            co,
            &format!(
                "recurring_bill.{}",
                if more {
                    "skip_missed"
                } else if action.starts_with("resume") {
                    "resume"
                } else {
                    action
                }
            ),
            format!("recurring_bill:{tid}"),
            json!({"before":{"active":t["active"],"deleted":t["deleted"],"revision":t["revision"],"next_index":t["next_index"]},"after":{"active":active,"deleted":deleted,"revision":stored_i64(&t,"revision")?+1,"next_index":stored_i64(&t,"next_index")?+skipped.len() as i64},"choice":action,"skipped_dates":skipped,"automatic":false}),
        )
    })
}
fn generate_one(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    tid: &str,
    through: &str,
) -> FbResult<Option<Value>> {
    write(book, |tx| {
        let u = writable(tx, actor, co)?;
        let t = get(tx, co, tid)?;
        if t["active"] != 1 || t["deleted"] == 1 {
            return Ok(None);
        }
        let Some(dt) = next(&t)?.filter(|d| d.as_str() <= through) else {
            return Ok(None);
        };
        let n = stored_i64(&t, "next_index")?;
        let due = calendar(&dt)?
            .checked_add_signed(Duration::days(stored_i64(&t, "terms_days")?))
            .filter(|d| d.year() <= 9999)
            .ok_or_else(|| err("Due date exceeds the supported calendar."))?;
        let mut p = t["data"].clone();
        p["date"] = json!(dt);
        p["due_date"] = json!(due.to_string());
        p["request_key"] = json!(
            uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_OID,
                format!("recurring-bill:{tid}:{n}:{dt}").as_bytes()
            )
            .to_string()
        );
        p["reference"] = json!("");
        let mut b = save_bill_tx(tx, actor, co, &p)?;
        let mut posting_error = None;
        if t["auto_post"] == 1 {
            match tx.atomic(|| {
                let stored = bill(tx, co, s(&b, "id"))?;
                let review = posting(tx, actor, co, &stored)?.2;
                post_bill_tx(tx, actor, co, s(&b, "id"), Some(1), &review)
            }) {
                Ok(posted) => b = posted,
                Err(e @ FbError::Validation(_)) => posting_error = Some(e.message()),
                Err(e) => return Err(e),
            }
        }
        tx.execute("INSERT INTO payables_recurring_occurrence(company_id,template_id,occurrence_index,occurrence_date,bill_id) VALUES(?,?,?,?,?)",params![co.to_db(),tid,n,dt,s(&b,"id")])?;
        services::audit(
            tx,
            &u,
            co,
            "recurring_bill.generated",
            format!("bill:{}", s(&b, "id")),
            json!({"template_id":tid,"template_revision":t["revision"],"occurrence_index":n,"occurrence_date":dt,"automatic":true,"auto_post":t["auto_post"]==1,"number":b["number"],"posting_error":posting_error}),
        )?;
        tx.execute(
            "UPDATE payables_recurring SET next_index=next_index+1 WHERE company_id=? AND id=?",
            params![co.to_db(), tid],
        )?;
        b["posting_error"] = json!(posting_error);
        Ok(Some(b))
    })
}
/// Creates at most `MAX_OCCURRENCES_PER_RUN` bills per template, and none for a template with a larger
/// backlog: that needs a person's Pause/Resume decision. Failure leaves that occurrence pending,
/// rolls back its numbering/ledger/audit, and does not stop other templates.
pub fn generate_due(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    through: &str,
) -> FbResult<Value> {
    let through = calendar(through)?.to_string();
    book.read(|q| writable(q, actor, co))?;
    let templates = list(book, actor, co)?;
    let mut created = vec![];
    let mut errors = vec![];
    let mut warnings = vec![];
    for mut t in templates {
        if t["active"] != 1 {
            continue;
        }
        let (dates, more) = pending_dates(&t, &through, MAX_OCCURRENCES_PER_RUN)?;
        if dates.is_empty() {
            continue;
        }
        if more {
            errors.push(json!({"template_id":t["id"],"name":t["name"],"message":format!("More than {MAX_OCCURRENCES_PER_RUN} bill dates are overdue, so none were created. Open the template, choose Pause, then Resume. The oldest dates beyond the newest {MAX_OCCURRENCES_PER_RUN} must be skipped; the newest {MAX_OCCURRENCES_PER_RUN} can be skipped or created.")}));
            continue;
        }
        for _ in 0..MAX_OCCURRENCES_PER_RUN {
            match generate_one(book, actor, co, s(&t, "id"), &through) {
                Ok(Some(b)) => {
                    if let Some(message) = b["posting_error"].as_str() {
                        warnings.push(json!({"template_id":t["id"],"name":t["name"],"message":format!("Bill {} was saved as a draft because automatic posting failed: {}",s(&b,"number"),message)}));
                    }
                    created.push(b);
                    t = book.read(|q| get(q, co, s(&t, "id")))?;
                    if t["active"] != 1 || next(&t)?.is_none_or(|d| d > through) {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    errors.push(
                        json!({"template_id":t["id"],"name":t["name"],"message":e.message()}),
                    );
                    break;
                }
            }
        }
    }
    Ok(json!({"created":created,"errors":errors,"warnings":warnings}))
}

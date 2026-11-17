//! Invoice-specific Stripe evidence. Provider calls belong to fb-net; observations
//! never alter a balance. Only an opaque, fresh receipt review can post live money.
use crate::{CompanyId, FbError, FbResult, Money, UserId, py, sales, services};
use fb_db::{Book, Query, rusqlite::params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

fn bad() -> FbError {
    FbError::validation("Stripe evidence changed or is invalid. Refresh and open a fresh review.")
}
fn text<'a>(v: &'a Value, k: &str) -> FbResult<&'a str> {
    v[k].as_str().ok_or_else(bad)
}
fn n(v: &Value, k: &str) -> FbResult<i64> {
    v[k].as_i64().ok_or_else(bad)
}
fn provider(s: &str, prefix: &str) -> bool {
    s.starts_with(prefix)
        && s.len() > prefix.len()
        && s.len() <= 256
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
fn mode(s: &str) -> FbResult<()> {
    if !["TEST", "LIVE"].contains(&s) {
        Err(bad())
    } else {
        Ok(())
    }
}
const CONNECTION: &str =
    "id,company_id,account_key,mode,key_last4,status,revision,created_at,updated_at,updated_by_id";
const LINK: &str = "id,company_id,connection_id,invoice_id,invoice_revision_id,created_by_id,description,amount,currency,state,provider_key,payment_url,cursor,created_at,checked_at,revision";
const PAYMENT: &str = "id,company_id,connection_id,link_id,account_key,mode,session_key,intent_key,amount,currency,received,refunded,disputed,provider_state,provider_created_epoch,observed_at,receipt_id,revision";
fn rows(
    q: &(impl Query + ?Sized),
    table: &str,
    columns: &str,
    filter: &str,
    args: impl fb_db::rusqlite::Params,
) -> FbResult<Vec<Value>> {
    // All SQL identifiers/filter fragments are private constants, never caller data.
    let pairs = columns
        .split(',')
        .map(|c| format!("'{c}',{c}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut st = q.prepare(&format!(
        "SELECT json_object({pairs}) FROM {table} WHERE {filter}"
    ))?;
    let raw = st
        .query_map(args, |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    raw.iter()
        .map(|v| fb_db::parse_unique_json_as(v).map_err(|_| bad()))
        .collect()
}
pub fn connection(q: &(impl Query + ?Sized), c: CompanyId, id: &str) -> FbResult<Value> {
    rows(
        q,
        "invoice_stripe_connection",
        CONNECTION,
        "company_id=?1 AND id=?2",
        params![c.to_db(), id],
    )?
    .pop()
    .ok_or_else(bad)
}
pub fn current_connection(
    q: &(impl Query + ?Sized),
    c: CompanyId,
    m: &str,
) -> FbResult<Option<Value>> {
    mode(m)?;
    Ok(rows(q,"invoice_stripe_connection",CONNECTION,"company_id=?1 AND mode=?2 ORDER BY status='CONNECTED' DESC,updated_at DESC,id DESC LIMIT 1",params![c.to_db(),m])?.pop())
}
pub fn links(q: &(impl Query + ?Sized), actor: UserId, c: CompanyId) -> FbResult<Vec<Value>> {
    services::access(q, actor, c)?;
    rows(
        q,
        "invoice_stripe_link",
        LINK,
        "company_id=?1 ORDER BY created_at,id",
        [c.to_db()],
    )
}
pub fn link(q: &(impl Query + ?Sized), c: CompanyId, id: &str) -> FbResult<Value> {
    rows(
        q,
        "invoice_stripe_link",
        LINK,
        "company_id=?1 AND id=?2",
        params![c.to_db(), id],
    )?
    .pop()
    .ok_or_else(bad)
}
pub fn payments(q: &(impl Query + ?Sized), actor: UserId, c: CompanyId) -> FbResult<Vec<Value>> {
    services::access(q, actor, c)?;
    rows(
        q,
        "invoice_stripe_payment",
        PAYMENT,
        "company_id=?1 ORDER BY provider_created_epoch DESC,id",
        [c.to_db()],
    )
}
pub fn payment(q: &(impl Query + ?Sized), c: CompanyId, id: &str) -> FbResult<Value> {
    rows(
        q,
        "invoice_stripe_payment",
        PAYMENT,
        "company_id=?1 AND id=?2",
        params![c.to_db(), id],
    )?
    .pop()
    .ok_or_else(bad)
}
pub fn check_connection_revision(
    q: &(impl Query + ?Sized),
    c: CompanyId,
    m: &str,
    expected: i64,
) -> FbResult<()> {
    if current_connection(q, c, m)?
        .as_ref()
        .map_or(Ok(0), |v| n(v, "revision"))?
        != expected
    {
        return Err(bad());
    }
    Ok(())
}
pub fn connect(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    m: &str,
    expected: i64,
    account: &str,
    last4: &str,
) -> FbResult<Value> {
    mode(m)?;
    if !provider(account, "acct_")
        || last4.len() != 4
        || !last4
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(bad());
    }
    book.write(|tx|{
  let user=services::access(tx,actor,c)?;check_connection_revision(tx,c,m,expected)?;
  if current_connection(tx,c,m)?.is_some_and(|v|v["status"]=="CONNECTED"&&v["account_key"]!=account){return Err(FbError::refused("Disconnect the current Stripe account before changing accounts."))}
  let old=rows(tx,"invoice_stripe_connection",CONNECTION,"company_id=?1 AND mode=?2 AND account_key=?3",params![c.to_db(),m,account])?.pop();
  let now=py::now_utc();let id=old.as_ref().map(|v|text(v,"id").map(str::to_owned)).transpose()?.unwrap_or_else(||fb_ids::new_v7().to_string());
  if old.is_some(){tx.execute("UPDATE invoice_stripe_connection SET revision=revision+1,key_last4=?2,status='CONNECTED',updated_at=?3,updated_by_id=?4 WHERE id=?1",params![id,last4,now,actor.to_db()])?;}
  else{tx.execute("INSERT INTO invoice_stripe_connection(id,company_id,account_key,mode,key_last4,status,revision,created_at,updated_at,updated_by_id) VALUES(?1,?2,?3,?4,?5,'CONNECTED',1,?6,?6,?7)",params![id,c.to_db(),account,m,last4,now,actor.to_db()])?;}
  services::audit(tx,&user,c,"stripe.connected",format!("stripe:{id}"),json!({"account":account,"mode":m,"last4":last4}))?;connection(tx,c,&id)
 })
}
pub fn disconnect(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    id: &str,
    expected: i64,
) -> FbResult<()> {
    book.write(|tx|{let user=services::access(tx,actor,c)?;let old=connection(tx,c,id)?;if n(&old,"revision")?!=expected{return Err(bad())}
 tx.execute("UPDATE invoice_stripe_connection SET revision=revision+1,status='DISCONNECTED',updated_at=?2,updated_by_id=?3 WHERE id=?1",params![id,py::now_utc(),actor.to_db()])?;
 services::audit(tx,&user,c,"stripe.disconnected",format!("stripe:{id}"),json!({"account":old["account_key"],"mode":old["mode"]}))})
}
fn invoice(
    q: &(impl Query + ?Sized),
    actor: UserId,
    c: CompanyId,
    id: &str,
) -> FbResult<(Value, i64)> {
    let invoice = sales::invoice(q, c, id)?;
    let balances = sales::invoice_balances_query(q, actor, c)?;
    let balance = balances
        .iter()
        .find(|v| v["invoice_id"] == id)
        .ok_or_else(bad)?;
    if invoice["current"]["state"] != "OPEN" {
        return Err(FbError::refused(
            "Post the current invoice before creating a Stripe link.",
        ));
    }
    let amount =
        Money::from_decimal_exact(crate::calculations::unit_rate(&balance["outstanding"])?)
            .ok_or_else(bad)?
            .to_db()
            .ok_or_else(bad)?;
    Ok((invoice, amount))
}
pub fn prepare_link(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    invoice_id: &str,
) -> FbResult<Value> {
    book.write(|tx|{
  let user=services::access(tx,actor,c)?;let prefs=crate::invoice_preferences::get(tx,c)?.settings;
  if !prefs.stripe_enabled{return Err(FbError::refused("Enable Stripe in invoice settings first."))}
  let conn=current_connection(tx,c,&prefs.stripe_mode)?.filter(|v|v["status"]=="CONNECTED").ok_or_else(||FbError::refused("Connect this company's Stripe account first."))?;
  let (inv,amount)=invoice(tx,actor,c,invoice_id)?;let company=crate::store::company(tx,c)?.ok_or_else(bad)?;
  if company.currency!="USD"||!(50..=99_999_999).contains(&amount){return Err(FbError::refused("Stripe invoice links currently support USD balances from $0.50 through $999,999.99."))}
  let existing=rows(tx,"invoice_stripe_link",LINK,"company_id=?1 AND connection_id=?2 AND invoice_id=?3 AND state IN ('CREATING','UNKNOWN','READY','REACTIVATED') ORDER BY state='REACTIVATED',created_at",params![c.to_db(),text(&conn,"id")?,invoice_id])?.pop();
  if let Some(old)=existing{if old["state"]=="REACTIVATED"{return Err(FbError::refused("Close the reactivated Stripe link before creating another request."))}
  if old["invoice_revision_id"]!=inv["current"]["id"]||n(&old,"amount")?!=amount{return Err(FbError::refused("Deactivate the previous Stripe link before preparing another."))}return Ok(old)}
  let pending:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM invoice_stripe_payment p JOIN invoice_stripe_link l ON l.id=p.link_id WHERE l.invoice_id=?1 AND p.mode='LIVE' AND p.intent_key!='' AND p.receipt_id IS NULL AND p.provider_state IN ('PENDING','READY'))",[invoice_id],|r|r.get(0))?;
  if pending{return Err(FbError::refused("Review the existing Stripe payment before requesting another payment."))}
  let number:String=tx.query_row("SELECT number FROM documents_displayidentity WHERE id=?1",[text(&inv,"identity_id")?],|r|r.get(0))?;
  let description=format!("{} — Invoice {number}",company.name);let description=description.chars().take(500).collect::<String>();
  let id=fb_ids::new_v7().to_string();
  tx.execute("INSERT INTO invoice_stripe_link(id,company_id,connection_id,invoice_id,invoice_revision_id,created_by_id,description,amount,currency,state,created_at,revision) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'USD','CREATING',?9,1)",params![id,c.to_db(),text(&conn,"id")?,invoice_id,inv["current"]["id"].as_str(),actor.to_db(),description,amount,py::now_utc()])?;
  services::audit(tx,&user,c,"stripe.link_prepared",format!("stripe-link:{id}"),json!({"invoice":invoice_id,"revision":inv["current"]["id"],"amount":amount,"mode":conn["mode"]}))?;link(tx,c,&id)
 })
}
/// Network helper input is frozen from a durable request, including its idempotency key.
pub fn request(v: &Value) -> FbResult<Value> {
    Ok(
        json!({"id":text(v,"id")?,"company":text(v,"company_id")?,"invoice":text(v,"invoice_id")?,"revision":text(v,"invoice_revision_id")?,"amount":n(v,"amount")?,"currency":text(v,"currency")?,"description":text(v,"description")?,"provider_id":text(v,"provider_key")?}),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteLink {
    pub id: String,
    pub url: String,
    pub active: bool,
}
pub enum LinkResult {
    Created(RemoteLink),
    Refused,
    Unknown,
}
pub fn finish_link(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    id: &str,
    expected: i64,
    result: LinkResult,
) -> FbResult<Value> {
    book.write(|tx|{
 let user=services::access(tx,actor,c)?;let old=link(tx,c,id)?;if n(&old,"revision")?!=expected||!matches!(text(&old,"state")?,"CREATING"|"UNKNOWN"){return Err(bad())}
 let(state,provider,url)=match result{LinkResult::Created(r)=>{if !provider(&r.id,"plink_")||!r.url.starts_with("https://buy.stripe.com/")||r.url.len()>2048||r.url.bytes().any(|b|b<=32||b>=127)||r.url.contains(['?','#','\\']){return Err(bad())}("READY",r.id,r.url)},LinkResult::Refused=>("REFUSED",String::new(),String::new()),LinkResult::Unknown=>("UNKNOWN",String::new(),String::new())};
 tx.execute("UPDATE invoice_stripe_link SET revision=revision+1,state=?2,provider_key=?3,payment_url=?4 WHERE id=?1",params![id,state,provider,url])?;
 services::audit(tx,&user,c,"stripe.link_result",format!("stripe-link:{id}"),json!({"state":state,"provider":provider}))?;link(tx,c,id)
 })
}
/// Explicit operator recovery after a full provider lookup and dashboard check.
/// A quiet interval outlasts the bounded helper call; never an automatic retry.
pub fn resolve_uncreated(book: &mut Book, actor: UserId, c: CompanyId, id: &str) -> FbResult<()> {
    let old = book.read(|q| {
        services::access(q, actor, c)?;
        link(q, c, id)
    })?;
    let at = chrono::DateTime::parse_from_rfc3339(text(&old, "created_at")?).map_err(|_| bad())?;
    if chrono::Utc::now().signed_duration_since(at).num_seconds() < 900 {
        return Err(FbError::refused(
            "Wait 15 minutes after the interrupted request, then check Stripe before resolving it.",
        ));
    }
    finish_link(
        book,
        actor,
        c,
        id,
        n(&old, "revision")?,
        LinkResult::Refused,
    )?;
    Ok(())
}
/// Provider reactivation makes the invoice payable again: restore balance guards.
pub fn mark_reactivated(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    id: &str,
    expected: i64,
) -> FbResult<()> {
    book.write(|tx|{let user=services::access(tx,actor,c)?;let old=link(tx,c,id)?;if n(&old,"revision")?!=expected||!matches!(text(&old,"state")?,"READY"|"INACTIVE"|"REACTIVATED"){return Err(bad())}
 tx.execute("UPDATE invoice_stripe_link SET revision=revision+1,state='REACTIVATED',checked_at=?2 WHERE id=?1",params![id,py::now_utc()])?;
 services::audit(tx,&user,c,"stripe.link_reactivated",format!("stripe-link:{id}"),json!({"provider":old["provider_key"]}))})
}
/// Called only after the helper has disabled the link and expired every open
/// checkout session. A provider's active=false field alone is not this proof.
pub fn mark_deactivated(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    id: &str,
    expected: i64,
) -> FbResult<()> {
    book.write(|tx|{let user=services::access(tx,actor,c)?;let old=link(tx,c,id)?;if n(&old,"revision")?!=expected||!matches!(text(&old,"state")?,"READY"|"INACTIVE"|"REACTIVATED"){return Err(bad())}
 tx.execute("UPDATE invoice_stripe_link SET revision=revision+1,state='INACTIVE',checked_at=?2 WHERE id=?1",params![id,py::now_utc()])?;
 services::audit(tx,&user,c,"stripe.link_deactivated",format!("stripe-link:{id}"),json!({"provider":old["provider_key"]}))})
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub session: String,
    pub intent: String,
    pub amount: i64,
    pub received: i64,
    pub refunded: i64,
    pub disputed: bool,
    pub currency: String,
    pub status: String,
    pub created: i64,
}
pub fn stage_page(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    id: &str,
    expected: i64,
    observations: &[Observation],
    next: &str,
) -> FbResult<usize> {
    if observations.len() > 100 || (!next.is_empty() && !provider(next, "cs_")) {
        return Err(bad());
    }
    book.write(|tx|tx.atomic(||{
  let user=services::access(tx,actor,c)?;let link=link(tx,c,id)?;if n(&link,"revision")?!=expected||!matches!(text(&link,"state")?,"READY"|"INACTIVE"|"REACTIVATED"){return Err(bad())}
  let conn=connection(tx,c,text(&link,"connection_id")?)?;let mut changed=0;
  for o in observations{
   if !provider(&o.session,"cs_")||(!o.intent.is_empty()&&!provider(&o.intent,"pi_")){return Err(bad())}
   if o.status=="READY"&&(o.amount!=n(&link,"amount")?||o.currency!=text(&link,"currency")?||o.received!=o.amount||o.refunded!=0||o.disputed||o.intent.is_empty()){return Err(bad())}
   let old=rows(tx,"invoice_stripe_payment",PAYMENT,"account_key=?1 AND mode=?2 AND session_key=?3",params![text(&conn,"account_key")?,text(&conn,"mode")?,o.session])?.pop();
   if let Some(old)=old{
    if old["link_id"]!=link["id"]||n(&old,"amount")?!=o.amount||old["currency"]!=o.currency||n(&old,"provider_created_epoch")?!=o.created{return Err(bad())}
    if old["intent_key"]==o.intent&&n(&old,"received")?==o.received&&n(&old,"refunded")?==o.refunded&&n(&old,"disputed")?==i64::from(o.disputed)&&old["provider_state"]==o.status{tx.execute("UPDATE invoice_stripe_payment SET revision=revision+1,observed_at=?2 WHERE id=?1",params![text(&old,"id")?,py::now_utc()])?;continue}
    tx.execute("UPDATE invoice_stripe_payment SET revision=revision+1,intent_key=?2,received=?3,refunded=?4,disputed=?5,provider_state=?6,observed_at=?7 WHERE id=?1",params![text(&old,"id")?,o.intent,o.received,o.refunded,o.disputed,o.status,py::now_utc()])?;
    services::audit(tx,&user,c,"stripe.payment_changed",format!("stripe-payment:{}",text(&old,"id")?),json!({"before":old,"after":o}))?;
   }else{
    let pid=fb_ids::new_v7().to_string();
    tx.execute("INSERT INTO invoice_stripe_payment(id,company_id,connection_id,link_id,account_key,mode,session_key,intent_key,amount,currency,received,refunded,disputed,provider_state,provider_created_epoch,observed_at,revision) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,1)",params![pid,c.to_db(),text(&conn,"id")?,id,text(&conn,"account_key")?,text(&conn,"mode")?,o.session,o.intent,o.amount,o.currency,o.received,o.refunded,o.disputed,o.status,o.created,py::now_utc()])?;
    services::audit(tx,&user,c,"stripe.payment_observed",format!("stripe-payment:{pid}"),json!({"invoice":link["invoice_id"],"mode":conn["mode"],"observation":o}))?;
   }changed+=1;
  }
  tx.execute("UPDATE invoice_stripe_link SET revision=revision+1,cursor=?2,checked_at=?3 WHERE id=?1",params![id,next,py::now_utc()])?;services::audit(tx,&user,c,"stripe.refreshed",format!("stripe-link:{id}"),json!({"changed":changed,"more":!next.is_empty()}))?;Ok(changed)
 }))
}
fn receipt_input(
    q: &(impl Query + ?Sized),
    actor: UserId,
    c: CompanyId,
    payment_id: &str,
    date: &str,
) -> FbResult<(Value, Value)> {
    let p = payment(q, c, payment_id)?;
    let l = link(q, c, text(&p, "link_id")?)?;
    if p["mode"] != "LIVE"
        || p["provider_state"] != "READY"
        || l["state"] != "INACTIVE"
        || p["receipt_id"].is_string()
    {
        return Err(FbError::refused(
            "Only an unposted, successful live payment with a deactivated link can be recorded.",
        ));
    }
    let checked =
        chrono::DateTime::parse_from_rfc3339(text(&p, "observed_at")?).map_err(|_| bad())?;
    let age = chrono::Utc::now()
        .signed_duration_since(checked)
        .num_seconds();
    if !(0..=300).contains(&age) || l["cursor"] != "" {
        return Err(FbError::refused(
            "Refresh this Stripe payment before reviewing or posting it.",
        ));
    }
    let (inv, due) = invoice(q, actor, c, text(&l, "invoice_id")?)?;
    if inv["current"]["id"] != l["invoice_revision_id"] || due < n(&p, "amount")? {
        return Err(FbError::refused(
            "The invoice or balance changed. This Stripe payment needs separate review.",
        ));
    }
    let amount = Money::from_db(n(&p, "amount")?).to_string();
    Ok((
        p.clone(),
        json!({"request_key":p["id"],"customer_id":inv["current"]["customer_id"],"date":date,"amount":amount,"method":"OTHER","reference":p["intent_key"],"memo":"Stripe gross payment; fees and payouts are recorded separately.","destination":"UNDEPOSITED","bank_id":null,"allocations":[{"invoice_id":l["invoice_id"],"amount":amount}]}),
    ))
}
fn evidence(p: &Value) -> Value {
    let mut v = p.clone();
    let m = v.as_object_mut().expect("payment");
    m.remove("revision");
    m.remove("observed_at");
    v
}
pub struct PaymentReview {
    actor: UserId,
    company: CompanyId,
    payment: String,
    evidence: Value,
    input: Value,
    receipt: sales::Review,
    pub display: Value,
}
pub fn review_payment(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    id: &str,
    date: &str,
) -> FbResult<PaymentReview> {
    let (p, input) = book.read(|q| {
        services::access(q, actor, c)?;
        receipt_input(q, actor, c, id, date)
    })?;
    let (cap, vm) = sales::review_receipt_form(book, actor, c, &input, false)?;
    Ok(PaymentReview {
        actor,
        company: c,
        payment: id.into(),
        evidence: evidence(&p),
        input,
        receipt: cap,
        display: json!({"payment":p,"receipt":vm}),
    })
}
pub fn post_payment(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    review: &PaymentReview,
) -> FbResult<Value> {
    if actor != review.actor || c != review.company {
        return Err(bad());
    }
    book.write(|tx| {
        tx.atomic(|| {
            let user = services::access(tx, actor, c)?;
            let (p, input) =
                receipt_input(tx, actor, c, &review.payment, text(&review.input, "date")?)?;
            if evidence(&p) != review.evidence || input != review.input {
                return Err(bad());
            }
            let receipt =
                sales::record_receipt_tx(tx, actor, c, &input, Some(&review.receipt), false)?;
            tx.execute(
                "UPDATE invoice_stripe_payment SET revision=revision+1,receipt_id=?2 WHERE id=?1",
                params![review.payment, text(&receipt, "id")?],
            )?;
            services::audit(
                tx,
                &user,
                c,
                "stripe.payment_posted",
                format!("stripe-payment:{}", review.payment),
                json!({"receipt":receipt["id"],"intent":p["intent_key"],"amount":p["amount"]}),
            )?;
            Ok(receipt)
        })
    })
}
/// Only a live, current request can be put into customer-facing documents.
pub fn payment_url(
    q: &(impl Query + ?Sized),
    actor: UserId,
    c: CompanyId,
    invoice_id: &str,
) -> FbResult<Option<String>> {
    let prefs = crate::invoice_preferences::get(q, c)?.settings;
    if !prefs.stripe_enabled || prefs.stripe_mode != "LIVE" {
        return Ok(None);
    }
    let (inv, due) = match invoice(q, actor, c, invoice_id) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    for row in rows(
        q,
        "invoice_stripe_link",
        LINK,
        "company_id=?1 AND invoice_id=?2 AND state='READY' ORDER BY created_at DESC",
        params![c.to_db(), invoice_id],
    )? {
        let pending:bool=q.query_row("SELECT EXISTS(SELECT 1 FROM invoice_stripe_payment WHERE link_id=?1 AND provider_state IN ('READY','PENDING'))",[text(&row,"id")?],|r|r.get(0))?;
        if pending {
            continue;
        }
        let conn = connection(q, c, text(&row, "connection_id")?)?;
        if conn["mode"] == "LIVE"
            && conn["status"] == "CONNECTED"
            && row["invoice_revision_id"] == inv["current"]["id"]
            && n(&row, "amount")? == due
        {
            return Ok(Some(text(&row, "payment_url")?.into()));
        }
    }
    Ok(None)
}

//! Ordered per-company suggestion rules (`BankFeedRule`, `save_rule`, `move_rule`, `set_rule_active`).
use super::*;

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub id: String,
    pub company_id: String,
    pub position: i64,
    pub name: String,
    pub match_text: String,
    pub sign: String,
    pub amount_min: Option<i64>,
    pub amount_max: Option<i64>,
    pub account_id: String,
    pub payee_name: String,
    pub active: bool,
}
const RULE_COLS: &str = "id,company_id,position,name,match_text,sign,amount_min,amount_max,account_id,payee_name,active";
fn rule_row(r: &Row<'_>) -> fb_db::rusqlite::Result<Rule> {
    Ok(Rule {
        id: r.get(0)?,
        company_id: r.get(1)?,
        position: r.get(2)?,
        name: r.get(3)?,
        match_text: r.get(4)?,
        sign: r.get(5)?,
        amount_min: r.get(6)?,
        amount_max: r.get(7)?,
        account_id: r.get(8)?,
        payee_name: r.get(9)?,
        active: r.get(10)?,
    })
}
fn rules_where(q: &(impl Query + ?Sized), co: CompanyId, extra: &str) -> FbResult<Vec<Rule>> {
    let mut st = q.prepare(&format!("SELECT {RULE_COLS} FROM bankfeeds_rule WHERE company_id=?1 {extra} ORDER BY position,id"))?;
    Ok(st.query_map([co.to_db()], rule_row)?.collect::<Result<Vec<_>, _>>()?)
}
pub(crate) fn active_rules_in(q: &(impl Query + ?Sized), co: CompanyId) -> FbResult<Vec<Rule>> {
    rules_where(q, co, "AND active=1")
}
/// `active_rules`: position order.
pub fn active_rules(book: &mut Book, actor: UserId, co: CompanyId) -> FbResult<Vec<Rule>> {
    book.read(|q| {
        services::access(q, actor, co)?;
        active_rules_in(q, co)
    })
}
/// Every rule of the company in model order (`position`, `pk`).
pub fn all_rules(book: &mut Book, actor: UserId, co: CompanyId) -> FbResult<Vec<Rule>> {
    book.read(|q| {
        services::access(q, actor, co)?;
        rules_where(q, co, "")
    })
}

/// `rule_for`: first active rule (position order) whose text / sign / amount range fit the item.
/// Python lowercases with 3.13's `str.lower()` (Unicode 15.1, pinned in `text::py_lower`), and compares
/// `abs(amount)` exactly (i128, so `i64::MIN` cannot wrap).
pub fn rule_for<'r>(rules: &'r [Rule], item: &Item) -> Option<&'r Rule> {
    let text = text::py_lower(&item.description);
    let amount = i128::from(item.amount).abs();
    rules.iter().find(|rule| {
        rule.active
            && text.contains(&text::py_lower(&rule.match_text))
            && !(rule.sign == "IN" && !item.money_in())
            && !(rule.sign == "OUT" && item.money_in())
            && rule.amount_min.is_none_or(|m| amount >= i128::from(m))
            && rule.amount_max.is_none_or(|m| amount <= i128::from(m))
    })
}

/// `rule_accounts`: active postable accounts that are not control accounts.
pub fn rule_accounts(book: &mut Book, actor: UserId, co: CompanyId) -> FbResult<Vec<crate::model::Account>> {
    book.read(|q| {
        services::access(q, actor, co)?;
        let controls = control_accounts(q, co)?;
        Ok(store::accounts(q, co)?.into_iter().filter(|a| a.active && a.postable && !controls.contains(&a.id.to_db())).collect())
    })
}

/// `_money_or_none`.
fn money_or_none(v: &Value, label: &str) -> FbResult<Option<i64>> {
    if v.is_null() || v.as_str() == Some("") {
        return Ok(None);
    }
    let amount = crate::calculations::unit_rate(v)?;
    if amount.is_sign_negative() && !amount.is_zero() {
        return Err(err(format!("{label} cannot be negative.")));
    }
    i64::try_from(amount.mantissa()).map(Some).map_err(|_| err("Document amount exceeds 99999999999999.99."))
}
fn opt_money(n: Option<i64>) -> Value {
    n.map(|n| json!(money(n))).unwrap_or(Value::Null)
}

/// `save_rule`: create (rule_id null) or edit a rule. `values` keys follow the Python keyword arguments.
pub fn save_rule(book: &mut Book, actor: UserId, co: CompanyId, values: &Value, secrets: &SecretSet) -> FbResult<Rule> {
    command(book, secrets, |w| {
        let tx = w.tx;
        let user = services::access(tx, actor, co)?;
        let get = |k: &str| values.get(k).cloned().unwrap_or(Value::Null);
        let previous = values["rule_id"].as_str().map(|key| {
            tx.query_row(&format!("SELECT {RULE_COLS} FROM bankfeeds_rule WHERE company_id=?1 AND id=?2"),params![co.to_db(),key],rule_row)
                .optional()
        }).transpose()?.flatten();
        let name = py::input_text(&get("name"), 120, true, previous.as_ref().map(|r|r.name.as_str()))?;
        let match_text = py::input_text(&get("match_text"), 200, true, previous.as_ref().map(|r|r.match_text.as_str()))?;
        let payee_name = py::input_text(values.get("payee_name").unwrap_or(&json!("")), 120, false, previous.as_ref().map(|r|r.payee_name.as_str()))?;
        if match_text.is_empty() {
            return Err(err("Enter the text the bank description must contain."));
        }
        let sign = values.get("sign").cloned().unwrap_or(json!("ANY"));
        let sign = sign.as_str().filter(|s| ["ANY", "IN", "OUT"].contains(s)).ok_or_else(|| err("Choose money in, money out, or either."))?.to_owned();
        let amount_min = money_or_none(&get("amount_min"), "Minimum amount")?;
        let amount_max = money_or_none(&get("amount_max"), "Maximum amount")?;
        if let (Some(lo), Some(hi)) = (amount_min, amount_max)
            && lo > hi
        {
            return Err(err("The minimum amount cannot exceed the maximum."));
        }
        let account_key = id(&get("account_id"), false)?.unwrap_or_default();
        let account = AccountId::parse(&account_key)
            .map(|a| store::account(tx, a))
            .transpose()?
            .flatten()
            .filter(|a| a.company_id == co)
            .ok_or_else(|| err("Choose an account from this company."))?;
        if let Some(role) = documents::control_role(tx, co, account.id)? {
            return Err(err(documents::control_message(&account, &role) + " A rule cannot categorize to it."));
        }
        if !account.active || !account.postable {
            return Err(err("Choose an active posting account."));
        }
        let active = truthy(values.get("active").unwrap_or(&json!(true)));
        let now = py::now_utc();
        let rule_id = get("rule_id");
        let (key, action) = if rule_id.is_null() {
            let count: i64 = tx.query_row("SELECT count(*) FROM bankfeeds_rule WHERE company_id=?1", [co.to_db()], |r| r.get(0))?;
            if count >= MAX_RULES {
                return Err(err(format!("A company has at most {MAX_RULES} rules.")));
            }
            let top: Option<i64> = tx.query_row("SELECT max(position) FROM bankfeeds_rule WHERE company_id=?1", [co.to_db()], |r| r.get(0))?;
            let position = top.filter(|p| *p != 0).unwrap_or(0).checked_add(1).ok_or_else(|| err("Too many rules."))?;
            let key = fb_ids::new_v7().to_string();
            w.execute(
                "INSERT INTO bankfeeds_rule(id,company_id,position,name,match_text,sign,amount_min,amount_max,account_id,payee_name,active,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?12)",
                params![key, co.to_db(), position, name, match_text, sign, amount_min, amount_max, account.id.to_db(), payee_name, active, now],
            )?;
            (key, "bankfeed.rule_created")
        } else {
            let key = rule_id.as_str().map(str::to_owned).unwrap_or_default();
            let found: Option<String> =
                tx.query_row("SELECT id FROM bankfeeds_rule WHERE company_id=?1 AND id=?2", params![co.to_db(), key], |r| r.get(0)).optional()?;
            let key = found.ok_or_else(|| err("Choose a rule of this company."))?;
            w.execute(
                "UPDATE bankfeeds_rule SET name=?2,match_text=?3,sign=?4,amount_min=?5,amount_max=?6,account_id=?7,payee_name=?8,active=?9,updated_at=?10 WHERE id=?1",
                params![key, name, match_text, sign, amount_min, amount_max, account.id.to_db(), payee_name, active, now],
            )?;
            (key, "bankfeed.rule_edited")
        };
        w.audit(
            &user,
            co,
            action,
            format!("bankfeedrule:{key}"),
            json!({"name":name,"match_text":match_text,"sign":sign,"amount_min":opt_money(amount_min),"amount_max":opt_money(amount_max),"account_id":account.id.to_db(),"payee_name":payee_name,"active":active}),
        )?;
        Ok(tx.query_row(&format!("SELECT {RULE_COLS} FROM bankfeeds_rule WHERE id=?1"), [&key], rule_row)?)
    })
}

/// `move_rule`: swap with the neighbour above ('up') or below ('down'); ends are a no-op.
pub fn move_rule(book: &mut Book, actor: UserId, co: CompanyId, rule_id: &str, direction: &Value, secrets: &SecretSet) -> FbResult<Rule> {
    command(book, secrets, |w| {
        let tx = w.tx;
        let user = services::access(tx, actor, co)?;
        let mut rules = rules_where(tx, co, "")?;
        let index = rules.iter().position(|r| r.id == rule_id).ok_or_else(|| err("Choose a rule of this company."))?;
        let direction = direction.as_str().filter(|d| ["up", "down"].contains(d)).ok_or_else(|| err("Move a rule up or down."))?;
        let other = if direction == "up" { index.checked_sub(1) } else { Some(index + 1) };
        let Some(other) = other.filter(|o| *o < rules.len()) else { return Ok(rules.swap_remove(index)) };
        for (position, rule) in rules.iter_mut().enumerate() {
            rule.position = position as i64 + 1;
        }
        let (a, b) = (rules[index].position, rules[other].position);
        rules[index].position = b;
        rules[other].position = a;
        for rule in &rules {
            w.execute("UPDATE bankfeeds_rule SET position=?2 WHERE id=?1", params![rule.id, rule.position])?;
        }
        w.audit(&user, co, "bankfeed.rule_moved", format!("bankfeedrule:{rule_id}"), json!({"direction":direction,"position":rules[index].position}))?;
        Ok(rules.swap_remove(index))
    })
}

/// `set_rule_active`.
pub fn set_rule_active(book: &mut Book, actor: UserId, co: CompanyId, rule_id: &str, active: &Value, secrets: &SecretSet) -> FbResult<Rule> {
    command(book, secrets, |w| {
        let tx = w.tx;
        let user = services::access(tx, actor, co)?;
        let rule = tx
            .query_row(&format!("SELECT {RULE_COLS} FROM bankfeeds_rule WHERE company_id=?1 AND id=?2"), params![co.to_db(), rule_id], rule_row)
            .optional()?
            .ok_or_else(|| err("Choose a rule of this company."))?;
        let active = truthy(active);
        if rule.active == active {
            return Ok(rule);
        }
        w.execute("UPDATE bankfeeds_rule SET active=?2,updated_at=?3 WHERE id=?1", params![rule_id, active, py::now_utc()])?;
        w.audit(&user, co, "bankfeed.rule_edited", format!("bankfeedrule:{rule_id}"), json!({"active":active}))?;
        Ok(Rule { active, ..rule })
    })
}

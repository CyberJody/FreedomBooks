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
    pub policy: Option<qbo::Policy>,
    pub feed_account_id: Option<String>,
    feed_scope_active: bool,
}
impl Rule {
    pub fn edit_snapshot(&self) -> String {
        services::snapshot_token(&json!([self.id, self.name, self.match_text, self.sign,
            self.amount_min, self.amount_max, self.account_id, self.payee_name, self.active, self.feed_account_id]))
    }
}
const RULE_COLS: &str = "id,company_id,position,name,match_text,sign,amount_min,amount_max,account_id,payee_name,active,(SELECT data FROM bankfeeds_rule_policy WHERE rule_id=bankfeeds_rule.id),feed_account_id,(feed_account_id IS NULL OR EXISTS(SELECT 1 FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE f.id=bankfeeds_rule.feed_account_id AND f.active=1 AND c.active=1))";
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
        policy: r.get::<_,Option<String>>(11)?.map(|s|fb_db::parse_unique_json(&s).and_then(serde_json::from_value).map_err(|e|fb_db::rusqlite::Error::FromSqlConversionFailure(11,fb_db::rusqlite::types::Type::Text,Box::new(e)))).transpose()?,
        feed_account_id: r.get(12)?,
        feed_scope_active: r.get(13)?,
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
        if let Some(policy)=&rule.policy { return rule.active && policy.matches(item); }
        rule.active
            && rule.feed_scope_active
            && rule.feed_account_id.as_ref().is_none_or(|id| *id == item.feed_account_id)
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
        if previous.as_ref().is_some_and(|r|r.policy.is_some()) {return Err(err("Imported conditions are retained. Use the rules list to test or disable this rule; import a revised source rule to change its conditions."));}
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
        let previous_scope = previous.as_ref().and_then(|r| r.feed_account_id.as_ref());
        let feed_scope = match values.get("feed_account_id") {
            Some(v) => id(v, true)?,
            None => previous_scope.cloned(),
        };
        if let Some(scope) = &feed_scope
            && Some(scope) != previous_scope
            && !tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id
                  WHERE f.id=?1 AND c.company_id=?2 AND f.active=1 AND c.active=1)",
                params![scope, co.to_db()], |r| r.get::<_, bool>(0),
            )?
        {
            return Err(err("Choose an active feed account of this company."));
        }
        let active = truthy(values.get("active").unwrap_or(&json!(true)));
        if let (Some(current), Some(expected)) = (&previous, values["expected_snapshot"].as_str()) {
            if current.name == name && current.match_text == match_text && current.sign == sign
                && current.amount_min == amount_min && current.amount_max == amount_max
                && current.account_id == account_key && current.payee_name == payee_name
                && current.active == active && current.feed_account_id == feed_scope {
                return Ok(current.clone());
            }
            if current.edit_snapshot() != expected {
                return Err(err("This rule changed since you opened the form. Reload it before saving."));
            }
        }
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
                "INSERT INTO bankfeeds_rule(id,company_id,position,name,match_text,sign,amount_min,amount_max,account_id,payee_name,active,created_at,updated_at,feed_account_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?12,?13)",
                params![key, co.to_db(), position, name, match_text, sign, amount_min, amount_max, account.id.to_db(), payee_name, active, now, feed_scope],
            )?;
            (key, "bankfeed.rule_created")
        } else {
            let key = rule_id.as_str().map(str::to_owned).unwrap_or_default();
            let found: Option<String> =
                tx.query_row("SELECT id FROM bankfeeds_rule WHERE company_id=?1 AND id=?2", params![co.to_db(), key], |r| r.get(0)).optional()?;
            let key = found.ok_or_else(|| err("Choose a rule of this company."))?;
            w.execute(
                "UPDATE bankfeeds_rule SET name=?2,match_text=?3,sign=?4,amount_min=?5,amount_max=?6,account_id=?7,payee_name=?8,active=?9,updated_at=?10,feed_account_id=?11 WHERE id=?1",
                params![key, name, match_text, sign, amount_min, amount_max, account.id.to_db(), payee_name, active, now, feed_scope],
            )?;
            (key, "bankfeed.rule_edited")
        };
        let mut detail = json!({"name":name,"match_text":match_text,"sign":sign,"amount_min":opt_money(amount_min),"amount_max":opt_money(amount_max),"account_id":account.id.to_db(),"payee_name":payee_name,"active":active});
        if feed_scope.is_some() || previous_scope.is_some() {
            detail["feed_account_id"] = json!(feed_scope);
        }
        w.audit(&user, co, action, format!("bankfeedrule:{key}"), detail)?;
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
        if active && let Some(p)=&rule.policy {qbo::validate_policy(tx,co,p,&rule.account_id)?;}
        if rule.active == active {
            return Ok(rule);
        }
        w.execute("UPDATE bankfeeds_rule SET active=?2,updated_at=?3 WHERE id=?1", params![rule_id, active, py::now_utc()])?;
        w.audit(&user, co, "bankfeed.rule_edited", format!("bankfeedrule:{rule_id}"), json!({"active":active}))?;
        Ok(Rule { active, ..rule })
    })
}

/// Turn off automatic posting without changing matching, category or active state.
/// The database permits only this one-way policy edit. The original import remains audited.
pub fn turn_off_imported_auto_post(book: &mut Book, actor: UserId, co: CompanyId, rule_id: Option<&str>, secrets: &SecretSet) -> FbResult<usize> {
    command(book, secrets, |w| {
        let user = services::access(w.tx, actor, co)?;
        let rules = rules_where(w.tx, co, "")?;
        if let Some(id) = rule_id
            && !rules.iter().any(|r| r.id == id && r.policy.is_some()) {
                return Err(err("Choose an imported rule of this company."));
        }
        let mut changed = 0;
        for rule in rules.into_iter().filter(|r| rule_id.is_none_or(|id| r.id == id)) {
            let Some(_) = rule.policy.filter(|p| p.auto_post) else { continue };
            w.execute("UPDATE bankfeeds_rule_policy SET data=json_set(data,'$.auto_post',json('false')) WHERE rule_id=?1 AND company_id=?2", params![rule.id, co.to_db()])?;
            w.execute("UPDATE bankfeeds_rule SET updated_at=?2 WHERE id=?1", params![rule.id, py::now_utc()])?;
            w.execute("DELETE FROM bankfeeds_auto_queue WHERE rule_id=?1", params![rule.id])?;
            w.audit(&user, co, "bankfeed.rule_auto_post_disabled", format!("bankfeedrule:{}", rule.id), json!({"auto_post":false,"active":rule.active,"suggestions_retained":true}))?;
            changed += 1;
        }
        Ok(changed)
    })
}

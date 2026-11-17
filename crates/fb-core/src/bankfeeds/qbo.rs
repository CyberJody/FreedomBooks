//! Strict, bounded reader for QBO's OLE/BIFF8 string-cell rules export. Never evaluates
//! formulas, loads macros, follows external references, or allocates from sheet dimensions.
//! Unrecognized QBO codes are diagnostics, not guesses. Only the screenshot-verified
//! condition 1/2/3/10 and action 0/5/8 meanings are translated.
use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Cursor, Read};

pub const MAX_FILE: usize = 4 * 1024 * 1024;
const MAX_CELL: usize = 16_384;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub code: i64,
    pub value: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub conditions: Vec<Condition>,
    pub all: bool,
    pub direction: String,
    /// ALL is an explicit selection, never an import default. Otherwise a feed account id.
    pub scope: String,
    pub auto_post: bool,
    pub kind: String,
    pub payee_id: String,
    pub payee_table: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct ImportRow {
    pub row: usize,
    pub name: String,
    pub conditions: Vec<Condition>,
    pub all: bool,
    pub direction: String,
    pub category: String,
    pub payee: String,
    pub auto_post: bool,
    pub diagnostics: Vec<String>,
    pub source_key: String,
}
#[derive(Clone, Debug)]
pub struct Import {
    pub hash: String,
    pub rows: Vec<ImportRow>,
}
#[derive(Clone, Debug)]
pub struct Selection {
    pub row: usize,
    /// Explicit opt-in; source flags alone never enable automatic posting.
    pub preserve_auto_post: bool,
    pub account_id: String,
    pub scope: String,
    pub kind: String,
    pub payee_id: String,
    pub payee_table: String,
}
fn bad() -> FbError {
    err(
        "This is not a supported QuickBooks BIFF8 rules export. Choose the original .xls export (no formulas or macros).",
    )
}
fn u16_at(b: &[u8], at: usize) -> FbResult<u16> {
    Ok(u16::from_le_bytes(
        b.get(at..at + 2)
            .ok_or_else(bad)?
            .try_into()
            .map_err(|_| bad())?,
    ))
}
fn u32_at(b: &[u8], at: usize) -> FbResult<u32> {
    Ok(u32::from_le_bytes(
        b.get(at..at + 4)
            .ok_or_else(bad)?
            .try_into()
            .map_err(|_| bad())?,
    ))
}
fn digest(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}

// SST continuation boundaries are meaningful: a continuation inside a character
// array starts with a fresh encoding flag. Headers do not consume such a flag.
struct Strings<'a> {
    chunks: Vec<&'a [u8]>,
    chunk: usize,
    pos: usize,
}
impl Strings<'_> {
    fn byte(&mut self) -> FbResult<u8> {
        while self.chunk < self.chunks.len() && self.pos == self.chunks[self.chunk].len() {
            self.chunk += 1;
            self.pos = 0;
        }
        let v = *self
            .chunks
            .get(self.chunk)
            .and_then(|b| b.get(self.pos))
            .ok_or_else(bad)?;
        self.pos += 1;
        Ok(v)
    }
    fn short(&mut self) -> FbResult<u16> {
        Ok(u16::from_le_bytes([self.byte()?, self.byte()?]))
    }
    fn string(&mut self) -> FbResult<String> {
        let count = self.short()? as usize;
        if count > MAX_CELL {
            return Err(bad());
        }
        let mut flag = self.byte()?;
        if flag > 1 {
            return Err(bad());
        } // no rich text / phonetic extensions
        let mut units = Vec::with_capacity(count);
        for _ in 0..count {
            if self.pos == self.chunks[self.chunk].len() {
                self.chunk += 1;
                self.pos = 0;
                flag = self.byte()?;
                if flag > 1 {
                    return Err(bad());
                }
            }
            let lo = self.byte()?;
            let v = if flag == 1 {
                u16::from_le_bytes([lo, self.byte()?])
            } else {
                lo as u16
            };
            units.push(v);
        }
        let s = String::from_utf16(&units).map_err(|_| bad())?;
        if s.len() > MAX_CELL {
            return Err(bad());
        }
        Ok(s)
    }
}
pub fn parse_xls(bytes: &[u8]) -> FbResult<Import> {
    if bytes.len() > MAX_FILE || bytes.len() < 512 {
        return Err(bad());
    }
    // QBO exports fit V3's header DIFAT. Reject extended DIFAT chains before
    // cfb can expand attacker-controlled FAT references into allocations.
    if &bytes[..8] != b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1"
        || u16_at(bytes, 26)? != 3
        || u16_at(bytes, 28)? != 0xfffe
        || u16_at(bytes, 30)? != 9
        || u16_at(bytes, 32)? != 6
        || !bytes.len().is_multiple_of(512)
    {
        return Err(bad());
    }
    let sectors = (bytes.len() / 512 - 1) as u32;
    let fats = u32_at(bytes, 44)?;
    if fats == 0
        || fats > 64
        || fats > sectors
        || u32_at(bytes, 40)? != 0
        || u32_at(bytes, 72)? != 0
        || u32_at(bytes, 68)? != 0xfffffffe
        || u32_at(bytes, 64)? > sectors
    {
        return Err(bad());
    }
    let mut unique = std::collections::HashSet::new();
    for index in 0..109 {
        let sector = u32_at(bytes, 76 + index * 4)?;
        if index < fats as usize {
            if sector >= sectors || !unique.insert(sector) {
                return Err(bad());
            }
        } else if sector != 0xffffffff {
            return Err(bad());
        }
    }
    let mut compound = cfb::CompoundFile::open_strict(Cursor::new(bytes)).map_err(|_| bad())?;
    let entries = compound.walk().take(3).collect::<Vec<_>>();
    if entries.len() != 2
        || entries
            .iter()
            .any(|e| e.is_stream() && e.path() != std::path::Path::new("/Workbook"))
    {
        return Err(bad());
    }
    let mut b = Vec::new();
    compound
        .open_stream("/Workbook")
        .map_err(|_| bad())?
        .take(MAX_FILE as u64 + 1)
        .read_to_end(&mut b)
        .map_err(|_| bad())?;
    if b.len() > MAX_FILE {
        return Err(bad());
    }
    let mut records = Vec::new();
    let mut at = 0;
    while at + 4 <= b.len() {
        let code = u16_at(&b, at)?;
        let len = u16_at(&b, at + 2)? as usize;
        at += 4;
        if code == 0 && len == 0 {
            break;
        }
        if len > 8224 || records.len() > 20_000 {
            return Err(bad());
        }
        let data = b.get(at..at + len).ok_or_else(bad)?;
        records.push((code, data));
        at += len;
    }
    if records
        .first()
        .is_none_or(|(t, b)| *t != 0x809 || u16_at(b, 0).ok() != Some(0x600))
    {
        return Err(bad());
    }
    let sheets = records
        .iter()
        .filter(|(t, _)| *t == 0x85)
        .collect::<Vec<_>>();
    if sheets.len() != 1 || sheets[0].1.get(5) != Some(&0) {
        return Err(bad());
    }
    if records
        .iter()
        .any(|(t, _)| matches!(t, 0x06 | 0x2f | 0x204 | 0x203 | 0x27e | 0xbd | 0xbe | 0x205))
    {
        return Err(bad());
    }
    let sst = records
        .iter()
        .position(|(t, _)| *t == 0xfc)
        .ok_or_else(bad)?;
    let count = u32_at(records[sst].1, 4)? as usize;
    if count > 603 {
        return Err(bad());
    }
    let mut chunks = vec![records[sst].1.get(8..).ok_or_else(bad)?];
    chunks.extend(
        records[sst + 1..]
            .iter()
            .take_while(|(t, _)| *t == 0x3c)
            .map(|(_, b)| *b),
    );
    let mut reader = Strings {
        chunks,
        chunk: 0,
        pos: 0,
    };
    let strings = (0..count)
        .map(|_| reader.string())
        .collect::<FbResult<Vec<_>>>()?;
    let mut cells = vec![vec![String::new(); 3]; 201];
    let mut seen = std::collections::HashSet::new();
    let mut last = 0;
    for (t, b) in records {
        if t == 0x200 && (u32_at(b, 4)? > 201 || u16_at(b, 10)? > 3) {
            return Err(bad());
        }
        if t == 0xfd {
            let r = u16_at(b, 0)? as usize;
            let c = u16_at(b, 2)? as usize;
            if r > 200 || c > 2 || !seen.insert((r, c)) {
                return Err(bad());
            }
            cells[r][c] = strings.get(u32_at(b, 6)? as usize).ok_or_else(bad)?.clone();
            last = last.max(r);
        }
    }
    if cells[0] != ["Rule Name", "Rule Conditions", "Rule Outputs"] || last == 0 {
        return Err(bad());
    }
    let rows = cells[1..=last]
        .iter()
        .enumerate()
        .map(|(i, c)| parse_row(i + 2, &c[0], &c[1], &c[2]))
        .collect();
    Ok(Import {
        hash: digest(bytes),
        rows,
    })
}
fn json_cell(s: &str) -> FbResult<Value> {
    if s.len() > MAX_CELL {
        return Err(bad());
    }
    let v = fb_db::parse_unique_json(s).map_err(|_| bad())?;
    fn bounded(v: &Value, depth: usize) -> bool {
        depth <= 6
            && match v {
                Value::Array(a) => a.len() <= 20 && a.iter().all(|v| bounded(v, depth + 1)),
                Value::Object(o) => o.len() <= 10 && o.values().all(|v| bounded(v, depth + 1)),
                _ => true,
            }
    }
    if !bounded(&v, 0) {
        return Err(bad());
    }
    Ok(v)
}
pub fn parse_row(row: usize, name: &str, conditions: &str, outputs: &str) -> ImportRow {
    let mut r = ImportRow {
        row,
        name: name.into(),
        conditions: vec![],
        all: true,
        direction: String::new(),
        category: String::new(),
        payee: String::new(),
        auto_post: false,
        diagnostics: vec![],
        source_key: digest(format!("{name}\n{conditions}\n{outputs}").as_bytes()),
    };
    if name.is_empty() || name.chars().count() > 120 {
        r.diagnostics
            .push("Rule name is missing or too long.".into());
    }
    let parsed = (|| -> FbResult<()> {
        let c = json_cell(conditions)?;
        let o = json_cell(outputs)?;
        if c.as_object()
            .is_none_or(|m| m.keys().any(|k| k != "ruleConditions" && k != "isAndRule"))
            || o.as_object()
                .is_none_or(|m| m.keys().any(|k| k != "ruleActions"))
        {
            return Err(bad());
        }
        r.all = c["isAndRule"].as_bool().ok_or_else(bad)?;
        for condition in c["ruleConditions"].as_array().ok_or_else(bad)? {
            if condition.as_object().is_none_or(|m| m.len() != 2) {
                return Err(bad());
            }
            let code = condition["ruleType"].as_i64().ok_or_else(bad)?;
            let value = condition["value"].as_str().ok_or_else(bad)?.to_owned();
            if code == 10 {
                if !r.direction.is_empty() {
                    return Err(bad());
                }
                r.direction = match value.as_str() {
                    "-1" => "OUT",
                    "1" => "IN",
                    _ => return Err(bad()),
                }
                .into();
            } else {
                if ![1, 2, 3].contains(&code) {
                    r.diagnostics.push(format!(
                        "Condition code {code} is not verified; this rule cannot be activated."
                    ));
                }
                if code == 1 && (value.is_empty() || value.chars().count() > 200) {
                    return Err(bad());
                }
                if [2, 3].contains(&code) {
                    signed_cents(&value)?;
                }
                r.conditions.push(Condition { code, value });
            }
        }
        let mut actions = std::collections::HashSet::new();
        for a in o["ruleActions"].as_array().ok_or_else(bad)? {
            if a.as_object().is_none_or(|m| m.len() != 2) {
                return Err(bad());
            }
            let code = a["actionType"].as_i64().ok_or_else(bad)?;
            if !actions.insert(code) {
                return Err(bad());
            }
            match code {
                0=>r.category=a["value"].as_str().ok_or_else(bad)?.to_owned(),
                5=>r.payee=a["value"].as_str().ok_or_else(bad)?.to_owned(),
                8=>r.auto_post=a["value"].as_bool().ok_or_else(bad)?,
                _=>r.diagnostics.push(format!("Action code {code} is not verified (including empty values); this rule cannot be activated.")),
            }
        }
        if r.conditions.is_empty()
            || r.direction.is_empty()
            || r.category.is_empty()
            || r.payee.chars().count() > 120
        {
            return Err(bad());
        }
        Ok(())
    })();
    if parsed.is_err() {
        r.diagnostics.push("Missing, incomplete or unsupported conditions/outputs. Correct the source rule and export it again.".into());
    }
    r
}
fn signed_cents(s: &str) -> FbResult<i64> {
    // unit_rate rejects fractional cents and overflow; do not round source amounts.
    let s = s.trim();
    let (negative, abs) = s.strip_prefix('-').map(|s| (true, s)).unwrap_or((false, s));
    let n = crate::calculations::unit_rate(&json!(abs))?;
    let value = i64::try_from(n.mantissa()).map_err(|_| bad())?;
    Ok(if negative { -value } else { value })
}
impl Policy {
    pub fn matches(&self, item: &Item) -> bool {
        if self.scope != "ALL" && self.scope != item.feed_account_id {
            return false;
        }
        if (self.direction == "IN" && item.amount <= 0)
            || (self.direction == "OUT" && item.amount >= 0)
        {
            return false;
        }
        let test = |c: &Condition| match c.code {
            1 => text::py_lower(&item.description).contains(&text::py_lower(&c.value)),
            2 => signed_cents(&c.value).is_ok_and(|v| item.amount == v),
            3 => signed_cents(&c.value).is_ok_and(|v| item.amount > v),
            _ => false,
        };
        !self.conditions.is_empty()
            && if self.all {
                self.conditions.iter().all(test)
            } else {
                self.conditions.iter().any(test)
            }
    }
}

pub(crate) fn validate_policy(
    q: &(impl Query + ?Sized),
    co: CompanyId,
    p: &Policy,
    category: &str,
) -> FbResult<String> {
    if !["IN", "OUT"].contains(&p.direction.as_str())
        || p.conditions.is_empty()
        || p.conditions.len() > 20
    {
        return Err(bad());
    }
    for c in &p.conditions {
        match c.code {
            1 if !c.value.is_empty() && c.value.chars().count() <= 200 => (),
            2 | 3 => {
                signed_cents(&c.value)?;
            }
            _ => return Err(bad()),
        }
    }
    if p.scope != "ALL" {
        let f=feed(q,&p.scope)?.filter(|f|f.company_id==co.to_db()&&f.active&&f.ledger_account_id.is_some()).ok_or_else(||err("Choose an active mapped bank feed of this company, or explicitly choose all feeds."))?;
        if f.ledger_account_id.as_deref() == Some(category) {
            return Err(err("A rule cannot categorize to its own feed account."));
        }
    }
    let a = AccountId::parse(category)
        .map(|id| store::account(q, id))
        .transpose()?
        .flatten()
        .filter(|a| a.company_id == co && a.active && a.postable)
        .ok_or_else(|| err("Map the category to an active posting account of this company."))?;
    if documents::control_role(q, co, a.id)?.is_some() {
        return Err(err("A rule cannot post to a control account."));
    }
    let eligible = match p.kind.as_str() {
        "EXPENSE" => p.direction == "OUT" && a.kind.as_str() == "EXPENSE",
        "INCOME" => p.direction == "IN" && a.kind.as_str() == "INCOME",
        "TRANSFER" => ["BANK", "CREDIT_CARD"].contains(&a.detail_type.as_str()),
        _ => false,
    };
    if !eligible {
        return Err(err(
            "Choose a supported transaction type and category: expense for money out, income for money in, or a transfer between eligible bank/card accounts. Other categories need manual review.",
        ));
    }
    if p.payee_id.is_empty() && p.payee_table.is_empty() {
        return Ok(String::new());
    }
    if !["directory_vendor", "directory_customer"].contains(&p.payee_table.as_str()) {
        return Err(err(
            "Map the payee to a customer or vendor of this company.",
        ));
    }
    q.query_row(
        &format!(
            "SELECT display_name FROM {} WHERE id=?1 AND company_id=?2 AND active=1",
            p.payee_table
        ),
        params![p.payee_id, co.to_db()],
        |r| r.get(0),
    )
    .optional()?
    .ok_or_else(|| err("The mapped payee is missing or inactive."))
}
/// Imports selected, fully resolved rows atomically. No downloaded transaction is
/// touched. Same source row content cannot be imported twice, even in another file.
pub fn import_rules(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    source: &Import,
    selections: &[Selection],
    description_mapping_confirmed: bool,
    secrets: &SecretSet,
) -> FbResult<Value> {
    if !description_mapping_confirmed {
        return Err(err(
            "Confirm how QuickBooks Description conditions map to the downloaded description.",
        ));
    }
    if selections.is_empty()
        || selections.len() > 200
        || source.rows.len() > 200
        || source.hash.len() != 64
    {
        return Err(err("Select between 1 and 200 valid rules."));
    }
    command(book, secrets, |w| {
        let q = w.tx;
        let user = services::access(q, actor, co)?;
        let count: i64 = q.query_row(
            "SELECT count(*) FROM bankfeeds_rule WHERE company_id=?1",
            [co.to_db()],
            |r| r.get(0),
        )?;
        if count + selections.len() as i64 > MAX_RULES {
            return Err(err("This import would exceed the 200-rule limit."));
        }
        let top: i64 = q.query_row(
            "SELECT coalesce(max(position),0) FROM bankfeeds_rule WHERE company_id=?1",
            [co.to_db()],
            |r| r.get(0),
        )?;
        let mut ordered = selections.to_vec();
        ordered.sort_by_key(|s| s.row);
        let mut seen = std::collections::HashSet::new();
        let mut ids = vec![];
        for (offset, s) in ordered.iter().enumerate() {
            if !seen.insert(s.row) {
                return Err(err("A rule was selected more than once."));
            }
            let r = source
                .rows
                .iter()
                .find(|r| r.row == s.row)
                .filter(|r| r.diagnostics.is_empty())
                .ok_or_else(|| {
                    err(format!(
                        "Source row {} has unresolved or unsupported data.",
                        s.row
                    ))
                })?;
            let duplicate:bool=q.query_row("SELECT EXISTS(SELECT 1 FROM bankfeeds_rule_policy WHERE company_id=?1 AND source_key=?2) OR EXISTS(SELECT 1 FROM bankfeeds_rule WHERE company_id=?1 AND active=1 AND fb_lower(name)=fb_lower(?3))",params![co.to_db(),r.source_key,r.name],|r|r.get(0))?;
            if duplicate {
                return Err(err(format!(
                    "Row {}: this source rule or rule name already exists. Deselect it to import the remaining rules.",
                    r.row
                )));
            }
            if !r.payee.is_empty() && s.payee_id.is_empty() {
                return Err(err(format!(
                    "Row {}: map the source payee before importing.",
                    r.row
                )));
            }
            let policy = Policy {
                conditions: r.conditions.clone(),
                all: r.all,
                direction: r.direction.clone(),
                scope: s.scope.clone(),
                auto_post: r.auto_post && s.preserve_auto_post,
                kind: s.kind.clone(),
                payee_id: s.payee_id.clone(),
                payee_table: s.payee_table.clone(),
            };
            let payee = validate_policy(q, co, &policy, &s.account_id)
                .map_err(|e| err(format!("Row {}: {e}", r.row)))?;
            let id = fb_ids::new_v7().to_string();
            let now = py::now_utc();
            w.execute("INSERT INTO bankfeeds_rule(id,company_id,position,name,match_text,sign,amount_min,amount_max,account_id,payee_name,active,created_at,updated_at) VALUES(?1,?2,?3,?4,'Imported conditions',?5,NULL,NULL,?6,?7,1,?8,?8)",params![id,co.to_db(),top+offset as i64+1,r.name,r.direction,s.account_id,payee,now])?;
            w.execute("INSERT INTO bankfeeds_rule_policy(rule_id,company_id,source_key,source_hash,source_row,data) VALUES(?1,?2,?3,?4,?5,?6)",params![id,co.to_db(),r.source_key,source.hash,r.row as i64,serde_json::to_string(&policy).map_err(|_|bad())?])?;
            w.audit(&user,co,"bankfeed.rule_imported",format!("bankfeedrule:{id}"),json!({"source_hash":source.hash,"source_row":r.row,"source_key":r.source_key,"source_auto_post":r.auto_post,"policy":policy,"account_id":s.account_id,"payee_name":payee,"description_mapping":"downloaded_description"}))?;
            ids.push(id);
        }
        Ok(
            json!({"imported":ids.len(),"ids":ids,"auto_post":ordered.iter().filter(|s|s.preserve_auto_post&&source.rows.iter().any(|r|r.row==s.row&&r.auto_post)).count()}),
        )
    })
}

#[cfg(test)]
mod tests;

//! Period activity from frozen sales evidence and the same tax-account recognition as What you owe.
use super::{Basis, Books, Figures, cents, err, parse, s};
use crate::{CompanyId, FbResult, Money};
use chrono::NaiveDate;
use fb_db::{Query, rusqlite::params};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use crate::reports::apportionment::monotonic_share;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Rate {
    name: String,
    agency_name: String,
    percent: Option<Decimal>,
    agency_id: Option<String>,
    id: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct GroupKey {
    name: String,
    jurisdiction: String,
    percent: Option<Decimal>,
    id: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum EntryKey {
    Group(GroupKey),
    Single(Rate),
}

#[derive(Default)]
struct EntryTotals {
    taxable: Money,
    exempt: Money,
    collected: Money,
    components: BTreeMap<Rate, Money>,
}

#[derive(Default)]
struct Period {
    before: Money,
    during: Money,
}

impl Period {
    fn add(&mut self, before: bool, amount: Money) {
        if before {
            self.before += amount;
        } else {
            self.during += amount;
        }
    }
}

struct Sale {
    total: Money,
    taxable: Money,
    exempt: Money,
    group: Option<GroupKey>,
    rates: Vec<(Rate, Money)>,
    gross: Period,
    tax: Period,
}

fn stored_money(data: &Value) -> FbResult<Money> {
    Ok(Money::from_db(cents(data)?))
}

fn sale(raw: &str, sign: i128) -> FbResult<Sale> {
    let data: Value =
        fb_db::parse_unique_json_as(raw).map_err(|_| err("Invalid stored sales document."))?;
    let subtotal = stored_money(&data["subtotal"])?;
    let no_tax = data["tax_mode"] == "NONE";
    let taxable = if no_tax {
        Money::ZERO
    } else {
        stored_money(&data["taxable"])?
    };
    let percent = |v: &Value| {
        v.as_str()
            .and_then(|value| Decimal::from_str_exact(value).ok())
            .map(|d| d.normalize())
    };
    let mut rates = Vec::new();
    for m in data["tax_snapshot"]["members"]
        .as_array()
        .into_iter()
        .flatten()
    {
        rates.push((
            Rate {
                name: s(m, "name").into(),
                agency_name: s(m, "agency_name").into(),
                percent: percent(&m["rate"]),
                agency_id: m["agency_id"].as_str().map(str::to_owned),
                id: s(m, "id").into(),
            },
            stored_money(&m["tax"])?,
        ));
    }
    let group = if data["tax_snapshot"]["kind"] == "GROUP" && rates.len() > 1 {
        let mut jurisdictions = BTreeSet::new();
        let mut total_rate = Some(Decimal::ZERO);
        for (rate, _) in &rates {
            if !rate.agency_name.is_empty() {
                jurisdictions.insert(rate.agency_name.clone());
            }
            total_rate = match (total_rate, rate.percent) {
                (Some(sum), Some(p)) => Some((sum + p).normalize()),
                _ => None,
            };
        }
        let name = s(&data["tax_snapshot"], "name");
        Some(GroupKey {
            name: if name.is_empty() {
                "Tax group".into()
            } else {
                name.into()
            },
            jurisdiction: jurisdictions.into_iter().collect::<Vec<_>>().join(", "),
            percent: total_rate,
            id: s(&data["tax_snapshot"], "id").into(),
        })
    } else {
        None
    };
    if rates.is_empty() {
        let label = s(&data, "tax_label");
        let name = if no_tax {
            "No tax selected"
        } else if label.is_empty() {
            "Manual rate"
        } else {
            label
        };
        rates.push((
            Rate {
                name: name.into(),
                agency_name: String::new(),
                percent: if no_tax {
                    None
                } else {
                    percent(&data["tax_rate"])
                },
                agency_id: None,
                id: String::new(),
            },
            stored_money(&data["tax"])?,
        ));
    }
    Ok(Sale {
        total: Money::from_cents(sign * stored_money(&data["total"])?.cents()),
        taxable,
        exempt: subtotal - taxable,
        group,
        rates,
        gross: Period::default(),
        tax: Period::default(),
    })
}

pub(super) struct Sales {
    docs: HashMap<String, Sale>,
    start: Option<NaiveDate>,
}

impl Sales {
    pub(super) fn load(
        q: &(impl Query + ?Sized),
        co: CompanyId,
        basis: Basis,
        start: Option<NaiveDate>,
        end: NaiveDate,
    ) -> FbResult<Self> {
        let mut stmt = q.prepare(
            "WITH sales(identity_id,data,journal_id,sign,immediate) AS (
               SELECT i.identity_id,r.data,r.journal_id,1,0
               FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN'
               WHERE i.company_id=?1
               UNION ALL
               SELECT r.identity_id,s.data,r.journal_id,1,1
               FROM sales_receipt r JOIN sales_salesreceipt s ON s.receipt_id=r.id
               WHERE r.company_id=?1
               UNION ALL
               SELECT identity_id,data,journal_id,-1,0 FROM credits_creditmemo WHERE company_id=?1
             )
             SELECT s.identity_id,s.data,s.sign,s.immediate,j.date,z.date
             FROM sales s JOIN ledger_journal j ON j.id=s.journal_id AND j.company_id=?1
             LEFT JOIN ledger_journal z ON z.reversal_of_id=j.id AND z.company_id=?1 AND z.status='POSTED' AND z.date<=?2
             WHERE j.status='POSTED' AND j.date<=?2",
        )?;
        let rows = stmt.query_map(params![co.to_db(), end.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })?;
        let mut docs = HashMap::new();
        let before = |d: NaiveDate| start.is_some_and(|first| d < first);
        for row in rows {
            let (identity, raw, sign, immediate, date, reversal) = row?;
            let mut sale = sale(&raw, sign.into())?;
            if basis == Basis::Accrual || immediate {
                sale.gross.add(before(parse(&date)?), sale.total);
                if let Some(date) = reversal {
                    sale.gross.add(before(parse(&date)?), -sale.total);
                }
            }
            docs.insert(identity, sale);
        }
        Ok(Self { docs, start })
    }

    pub(super) fn recognise_gross(&mut self, books: &Books) {
        for event in &books.events {
            if let Some(sale) = self.docs.get_mut(&event.doc) {
                let amount = if sale.total < Money::ZERO {
                    -event.amount
                } else {
                    event.amount
                };
                sale.gross
                    .add(self.start.is_some_and(|s| event.date < s), amount);
            }
        }
    }

    pub(super) fn add_tax(&mut self, identity: Option<&str>, before: bool, amount: Money) -> bool {
        if let Some(sale) = identity.and_then(|id| self.docs.get_mut(id)) {
            sale.tax.add(before, amount);
            true
        } else {
            false
        }
    }

    pub(super) fn finish(
        self,
        agencies: &mut BTreeMap<String, Figures>,
        unlinked: &mut Figures,
        collected: Money,
    ) -> FbResult<(Vec<Value>, Value)> {
        let mut entries: BTreeMap<EntryKey, EntryTotals> = BTreeMap::new();
        let mut total_taxable = Money::ZERO;
        let mut total_exempt = Money::ZERO;
        let mut accounted_tax = Money::ZERO;
        for sale in self.docs.into_values() {
            // Use monotonic cumulative apportionment at both period boundaries so
            // positive receipts never reduce any agency's recognized tax and
            // reversals release the exact same cents.
            let tax_before = tax_shares(&sale.rates, sale.tax.before);
            let tax_through = tax_shares(&sale.rates, sale.tax.before + sale.tax.during);
            let sales_before = sales_shares(&sale, sale.gross.before - sale.tax.before);
            let sales_through = sales_shares(
                &sale,
                sale.gross.before + sale.gross.during - sale.tax.before - sale.tax.during,
            );
            let taxable = sales_through[0] - sales_before[0];
            let exempt = sales_through[1] - sales_before[1];
            let mut period_tax_sum = Money::ZERO;
            let mut component_taxes = Vec::with_capacity(sale.rates.len());
            for (index, (rate, _)) in sale.rates.into_iter().enumerate() {
                let opening = tax_before[index];
                let period = tax_through[index] - opening;
                let agency = match &rate.agency_id {
                    Some(id) => agencies.entry(id.clone()).or_default(),
                    None => &mut *unlinked,
                };
                agency.add(true, opening, Money::ZERO);
                agency.add(false, period, Money::ZERO);
                period_tax_sum += period;
                component_taxes.push((rate, period));
            }
            if taxable.is_zero() && exempt.is_zero() && period_tax_sum.is_zero() {
                continue;
            }
            total_taxable += taxable;
            total_exempt += exempt;
            accounted_tax += period_tax_sum;
            if let Some(group) = sale.group {
                let entry = entries.entry(EntryKey::Group(group)).or_default();
                entry.taxable += taxable;
                entry.exempt += exempt;
                entry.collected += period_tax_sum;
                for (rate, period) in component_taxes {
                    *entry.components.entry(rate).or_default() += period;
                }
            } else if let Some((rate, period)) = component_taxes.into_iter().next() {
                let entry = entries.entry(EntryKey::Single(rate)).or_default();
                entry.taxable += taxable;
                entry.exempt += exempt;
                entry.collected += period;
            }
        }
        let other = collected - accounted_tax;
        let mut rows = Vec::new();
        for (key, entry) in entries {
            match key {
                EntryKey::Single(rate) => {
                    rows.push(json!({
                        "name": rate.name,
                        "jurisdiction": rate.agency_name,
                        "rate": rate.percent.map(|p| p.to_string()),
                        "taxable": entry.taxable.to_string(),
                        "exempt": entry.exempt.to_string(),
                        "collected": entry.collected.to_string(),
                        "is_component": false,
                    }));
                }
                EntryKey::Group(group) => {
                    rows.push(json!({
                        "name": group.name,
                        "jurisdiction": group.jurisdiction,
                        "rate": group.percent.map(|p| p.to_string()),
                        "taxable": entry.taxable.to_string(),
                        "exempt": entry.exempt.to_string(),
                        "collected": entry.collected.to_string(),
                        "is_component": false,
                    }));
                    for (rate, tax) in entry.components {
                        rows.push(json!({
                            "name": rate.name,
                            "jurisdiction": rate.agency_name,
                            "rate": rate.percent.map(|p| p.to_string()),
                            "taxable": Value::Null,
                            "exempt": Value::Null,
                            "collected": tax.to_string(),
                            "is_component": true,
                        }));
                    }
                }
            }
        }
        if !other.is_zero() {
            rows.push(json!({
                "name": "Other tax activity",
                "jurisdiction": "",
                "rate": Value::Null,
                "taxable": Value::Null,
                "exempt": Value::Null,
                "collected": other.to_string(),
                "is_component": false,
            }));
        }
        let totals = json!({
            "taxable": total_taxable.to_string(),
            "exempt": total_exempt.to_string(),
            "collected": collected.to_string(),
        });
        Ok((rows, totals))
    }
}

fn tax_shares(rates: &[(Rate, Money)], amount: Money) -> Vec<Money> {
    if rates.len() == 1 {
        return vec![amount];
    }
    let weights: Vec<i128> = rates.iter().map(|(_, w)| w.cents().abs()).collect();
    monotonic_share(&weights, amount)
}

fn sales_shares(sale: &Sale, amount: Money) -> [Money; 2] {
    let weights = [sale.taxable.cents().abs(), sale.exempt.cents().abs()];
    let parts = monotonic_share(&weights, amount);
    [parts[0], parts[1]]
}

#[cfg(test)]
mod tests {
    use super::monotonic_share;
    use crate::Money;

    #[test]
    fn monotonic_share_never_decreases_any_agency_and_matches_exact_weights() {
        for weights in [
            vec![1_i128, 3, 3],
            vec![1, 1, 1, 7],
            vec![400, 250],
            vec![1, 2, 5, 13, 21],
        ] {
            let total: i128 = weights.iter().sum();
            let mut prev = vec![0_i128; weights.len()];
            for k in 0..=(2 * total) {
                let alloc = monotonic_share(&weights, Money::from_cents(k));
                let cents: Vec<i128> = alloc.iter().map(|m| m.cents()).collect();
                assert_eq!(cents.iter().sum::<i128>(), k);
                for (i, (&cur, &old)) in cents.iter().zip(&prev).enumerate() {
                    assert!(
                        cur >= old,
                        "weights={weights:?} k={k} idx={i}: {cur} < {old}"
                    );
                }
                if k == total {
                    assert_eq!(cents, weights);
                }
                let neg: Vec<i128> = monotonic_share(&weights, Money::from_cents(-k))
                    .iter()
                    .map(|m| m.cents())
                    .collect();
                assert_eq!(neg, cents.iter().map(|c| -*c).collect::<Vec<_>>());
                prev = cents;
            }
        }
    }
}

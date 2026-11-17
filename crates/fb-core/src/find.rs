//! Find: one search across a company's documents, names, accounts and items, for the title-bar search
//! field. Read-only (a `Book::read` transaction, `query_only`), company-scoped, parameterized and bounded:
//! a minimum query length, a cap on its length and a row limit per kind. Documents are driven from the
//! company's document identities (`UNIQUE(company_id, kind, ordinal)`), so each kind reads only this
//! company's rows of that kind; journal entries use `ledger_journal(company_id, date)`.
//!
//! The caller is authorized for the company here (`services::access`); the app layer also checks the
//! viewer's role for the search page and for every record a result links to.
use crate::{Book, CompanyId, FbResult, Money, UserId, services};
use fb_db::Query;
use serde::Serialize;

/// The last run of digits of a number column, without leading zeros (`INV-001042` → `1042`).
macro_rules! trailing_digits {
    ($col:literal) => {
        concat!("ltrim(substr(", $col, ",length(rtrim(", $col, ",'0123456789'))+1),'0')")
    };
}

macro_rules! receipt_source {
    ($kind:literal) => {
        concat!(
            "SELECT s.id AS id, d.number AS number, '' AS alt, s.date AS date,
            coalesce(p.display_name, json_extract(s.data,'$.customer_snapshot.display_name'), '') AS name, s.amount AS amount,
            CASE WHEN EXISTS(SELECT 1 FROM sales_receiptreversal z WHERE z.receipt_id=s.id) THEN 'VOID' ELSE 'RECORDED' END AS status,
            coalesce(json_extract(s.data,'$.memo'),'') AS memo,
            trim(coalesce(json_extract(s.data,'$.reference'),'') || ' ' || coalesce((SELECT c.order_reference FROM prepayments_customerdeposit c WHERE c.receipt_id=s.id),'')) AS reference,
            '' AS extra, 0 AS amount_hit, d.ordinal AS ordinal
            FROM documents_documentidentity d JOIN sales_receipt s ON s.identity_id=d.id AND s.kind=d.kind
            LEFT JOIN directory_customer p ON p.id=s.customer_id AND p.company_id=d.company_id
            WHERE d.company_id=?1 AND d.kind='",
            $kind,
            "'"
        )
    };
}

macro_rules! party_query {
    ($table:literal) => {
        concat!(
            "SELECT id, '', '', '', display_name, NULL, CASE WHEN active THEN 'ACTIVE' ELSE 'INACTIVE' END,
              trim(email || '  ' || phone), '',
              (fb_lower(display_name)=?2) AS exact
            FROM ", $table, " WHERE company_id=?1 AND (
              (?5 AND (instr(fb_lower(display_name),?2)>0 OR instr(fb_lower(business_name),?2)>0 OR instr(fb_lower(contact_name),?2)>0
                OR instr(fb_lower(email),?2)>0))
              OR (?8 IS NOT NULL AND instr(replace(replace(replace(replace(replace(replace(phone,'-',''),' ',''),'(',''),')',''),'.',''),'+',''),?8)>0))
            ORDER BY exact DESC, instr(fb_lower(display_name),?2)=1 DESC, active DESC, fb_lower(display_name) LIMIT ?6"
        )
    };
}

/// The shortest text query. A shorter query that is all digits still finds documents by number.
pub const MIN_CHARS: usize = 2;
/// The longest query accepted (the lists' own filter boxes allow 120 characters as well).
pub const MAX_CHARS: usize = 120;

/// What a result is. The order here is the order the groups are shown in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Invoice,
    Estimate,
    SalesReceipt,
    CreditMemo,
    Payment,
    CustomerDeposit,
    Bill,
    BillPayment,
    Check,
    Deposit,
    Transfer,
    TaxPayment,
    JournalEntry,
    Customer,
    Vendor,
    Account,
    Item,
}

impl Kind {
    pub const ALL: [Kind; 17] = [
        Kind::Invoice, Kind::Estimate, Kind::SalesReceipt, Kind::CreditMemo, Kind::Payment, Kind::CustomerDeposit,
        Kind::Bill, Kind::BillPayment, Kind::Check, Kind::Deposit, Kind::Transfer, Kind::TaxPayment, Kind::JournalEntry,
        Kind::Customer, Kind::Vendor, Kind::Account, Kind::Item,
    ];
    /// Documents carry a number, a date and an amount; the others are names.
    pub fn is_document(self) -> bool {
        !matches!(self, Kind::Customer | Kind::Vendor | Kind::Account | Kind::Item)
    }
}

/// A parsed query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Terms {
    /// The query as typed, trimmed.
    pub text: String,
    /// Lowercased with the same case mapping as the book's `fb_lower`.
    lower: String,
    /// All digits: the number with leading zeros removed (`1042` finds `INV-001042`).
    digits: Option<String>,
    /// An exact amount in cents (`84`, `84.00`, `$1,234.50`).
    amount: Option<i64>,
    /// Text matching (names, memos, partial numbers) needs at least `MIN_CHARS` characters.
    contains: bool,
    /// The digits of the query when it has three or more, for phone numbers typed any way.
    phone: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermsError {
    Empty,
    TooShort,
    TooLong,
}

impl Terms {
    pub fn parse(raw: &str) -> Result<Terms, TermsError> {
        let text = raw.trim().to_owned();
        let chars = text.chars().count();
        if chars == 0 {
            return Err(TermsError::Empty);
        }
        if chars > MAX_CHARS {
            return Err(TermsError::TooLong);
        }
        let all_digits = text.bytes().all(|b| b.is_ascii_digit());
        if chars < MIN_CHARS && !all_digits {
            return Err(TermsError::TooShort);
        }
        let digits = all_digits.then(|| text.trim_start_matches('0').to_owned()).filter(|d| !d.is_empty());
        let phone_digits: String = text.chars().filter(char::is_ascii_digit).collect();
        let phone_like = text.chars().all(|c| c.is_ascii_digit() || " -().+".contains(c));
        Ok(Terms {
            lower: fb_db::collation::lower(&text),
            amount: amount(&text),
            contains: chars >= MIN_CHARS,
            phone: (phone_like && phone_digits.len() >= 3).then_some(phone_digits),
            digits,
            text,
        })
    }
    /// True when the query is all digits (it may be a document number, a check number or an amount).
    fn digits_only(&self) -> bool {
        self.text.bytes().all(|b| b.is_ascii_digit())
    }
}

/// `84`, `84.5`, `84.50`, `1,234.50`, `$84.00`: an exact positive amount in cents.
fn amount(text: &str) -> Option<i64> {
    let s = text.trim().trim_start_matches('$').trim();
    let (whole, cents) = match s.split_once('.') {
        Some((w, c)) if (1..=2).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_digit()) => (w, c),
        Some(_) => return None,
        None => (s, ""),
    };
    // Thousands separators only in their proper places.
    let groups: Vec<&str> = whole.split(',').collect();
    if groups.iter().any(|g| g.is_empty() || !g.bytes().all(|b| b.is_ascii_digit()))
        || (groups.len() > 1 && (groups[0].len() > 3 || groups[1..].iter().any(|g| g.len() != 3)))
    {
        return None;
    }
    let digits: String = groups.concat();
    if digits.len() > 14 {
        return None;
    }
    let whole: i64 = digits.parse().ok()?;
    let cents: i64 = if cents.is_empty() { 0 } else { format!("{cents:0<2}").parse().ok()? };
    let total = whole.checked_mul(100)?.checked_add(cents)?;
    (total > 0 && total <= 9_999_999_999_999_999).then_some(total)
}

/// One result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub kind: Kind,
    /// The record's id (the detail page's argument).
    pub id: String,
    /// The document number, account code, or empty.
    pub number: String,
    /// A second number: the check number, the vendor's bill reference, a journal's entry number.
    pub alt: String,
    pub date: String,
    /// Customer, vendor or payee; the bank account of a deposit; the accounts of a transfer; or the
    /// record's own name.
    pub name: String,
    pub amount: Option<Money>,
    /// A status code: DRAFT, OPEN, PARTIAL, PAID, VOID, RECORDED, ACCEPTED, DECLINED, CANCELLED,
    /// CONVERTED, POSTED, REVERSED, ACTIVE or INACTIVE.
    pub status: String,
    /// The memo or reference, or contact details for a name.
    pub detail: String,
    /// The query is this document's number (or check number, reference or entry number).
    pub exact: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub kind: Kind,
    pub hits: Vec<Hit>,
    /// More rows matched than `per_kind`.
    pub more: bool,
}

/// The columns every document source yields: id, number, alt, date, name, amount, status, memo,
/// reference, extra text, amount_hit, ordinal. `?1` is the company, `?4` the amount.
fn document_source(kind: Kind) -> Option<&'static str> {
    Some(match kind {
        Kind::Invoice => "SELECT inv.id AS id, coalesce(h.source_number, d.number) AS number, '' AS alt, r.date AS date,
            coalesce(p.display_name, json_extract(r.data,'$.customer_snapshot.display_name'), '') AS name, r.total AS amount, r.state AS status,
            coalesce(json_extract(r.data,'$.memo'),'') AS memo, '' AS reference, '' AS extra, 0 AS amount_hit, d.ordinal AS ordinal
            FROM documents_documentidentity d JOIN sales_invoice inv ON inv.identity_id=d.id
            JOIN sales_invoicerevision r ON r.invoice_id=inv.id AND r.number=inv.revision
            LEFT JOIN sales_importinvoice h ON h.invoice_id=inv.id AND h.company_id=d.company_id
            LEFT JOIN directory_customer p ON p.id=r.customer_id AND p.company_id=d.company_id
            WHERE d.company_id=?1 AND d.kind='INV'",
        Kind::Estimate => "SELECT e.id AS id, d.number AS number, '' AS alt, r.date AS date,
            coalesce(p.display_name, json_extract(r.data,'$.customer_snapshot.display_name'), '') AS name, r.total AS amount,
            CASE WHEN EXISTS(SELECT 1 FROM sales_estimateconversion v WHERE v.estimate_id=e.id) THEN 'CONVERTED' ELSE r.state END AS status,
            coalesce(json_extract(r.data,'$.memo'),'') AS memo, '' AS reference, '' AS extra, 0 AS amount_hit, d.ordinal AS ordinal
            FROM documents_documentidentity d JOIN sales_estimate e ON e.identity_id=d.id
            JOIN sales_estimaterevision r ON r.estimate_id=e.id AND r.number=e.revision
            LEFT JOIN directory_customer p ON p.id=r.customer_id AND p.company_id=d.company_id
            WHERE d.company_id=?1 AND d.kind='EST'",
        Kind::SalesReceipt | Kind::Payment | Kind::CustomerDeposit => match kind {
            Kind::SalesReceipt => receipt_source!("SR"),
            Kind::Payment => receipt_source!("RCP"),
            _ => receipt_source!("CDP"),
        },
        Kind::CreditMemo => "SELECT m.id AS id, d.number AS number, '' AS alt, m.date AS date,
            coalesce(p.display_name, json_extract(m.data,'$.customer_snapshot.display_name'), '') AS name, m.total AS amount,
            CASE WHEN EXISTS(SELECT 1 FROM credits_creditmemovoid z WHERE z.credit_memo_id=m.id) THEN 'VOID' ELSE 'RECORDED' END AS status,
            coalesce(json_extract(m.data,'$.memo'),'') AS memo, '' AS reference, '' AS extra, 0 AS amount_hit, d.ordinal AS ordinal
            FROM documents_documentidentity d JOIN credits_creditmemo m ON m.identity_id=d.id
            LEFT JOIN directory_customer p ON p.id=m.customer_id AND p.company_id=d.company_id
            WHERE d.company_id=?1 AND d.kind='CM'",
        Kind::Bill => "SELECT b.id AS id, d.number AS number, r.reference AS alt, r.date AS date,
            coalesce(v.display_name, json_extract(r.vendor_snapshot,'$.display_name'), '') AS name, r.total AS amount, b.state AS status,
            r.memo AS memo, r.reference AS reference, '' AS extra, 0 AS amount_hit, d.ordinal AS ordinal
            FROM documents_documentidentity d JOIN payables_bill b ON b.identity_id=d.id
            JOIN payables_billrevision r ON r.bill_id=b.id AND r.number=b.revision
            LEFT JOIN directory_vendor v ON v.id=r.vendor_id AND v.company_id=d.company_id
            WHERE d.company_id=?1 AND d.kind='BILL'",
        Kind::BillPayment => "SELECT x.id AS id, d.number AS number, x.reference AS alt, x.date AS date,
            coalesce(v.display_name, json_extract(x.vendor_snapshot,'$.display_name'), '') AS name, x.amount AS amount,
            CASE WHEN EXISTS(SELECT 1 FROM payables_paymentreversal z WHERE z.payment_id=x.id) THEN 'VOID' ELSE 'RECORDED' END AS status,
            x.memo AS memo, x.reference AS reference, '' AS extra, 0 AS amount_hit, d.ordinal AS ordinal
            FROM documents_documentidentity d JOIN payables_billpayment x ON x.identity_id=d.id
            LEFT JOIN directory_vendor v ON v.id=x.vendor_id AND v.company_id=d.company_id
            WHERE d.company_id=?1 AND d.kind='BPAY'",
        Kind::Check => "SELECT k.id AS id, d.number AS number, r.check_number AS alt, r.date AS date, r.payee_name AS name,
            r.total AS amount, k.state AS status, r.memo AS memo, '' AS reference, '' AS extra, 0 AS amount_hit, d.ordinal AS ordinal
            FROM documents_documentidentity d JOIN checks_check k ON k.identity_id=d.id
            JOIN checks_checkrevision r ON r.document_id=k.id AND r.number=k.revision
            WHERE d.company_id=?1 AND d.kind='CHK'",
        Kind::Deposit => "SELECT x.id AS id, d.number AS number, '' AS alt, x.date AS date, coalesce(a.name,'') AS name, x.amount AS amount,
            CASE WHEN EXISTS(SELECT 1 FROM banking_depositreversal z WHERE z.deposit_id=x.id) THEN 'VOID' ELSE 'RECORDED' END AS status,
            coalesce(json_extract(x.data,'$.memo'),'') AS memo, coalesce(json_extract(x.data,'$.reference'),'') AS reference,
            '' AS extra, 0 AS amount_hit, d.ordinal AS ordinal
            FROM documents_documentidentity d JOIN banking_deposit x ON x.identity_id=d.id
            LEFT JOIN ledger_account a ON a.id=x.bank_id AND a.company_id=d.company_id
            WHERE d.company_id=?1 AND d.kind='DEP'",
        Kind::Transfer => "SELECT t.id AS id, d.number AS number, '' AS alt, t.date AS date,
            coalesce(fa.name,'') || ' → ' || coalesce(ta.name,'') AS name, t.amount AS amount,
            CASE WHEN EXISTS(SELECT 1 FROM transfers_transfervoid z WHERE z.transfer_id=t.id) THEN 'VOID' ELSE 'RECORDED' END AS status,
            t.memo AS memo, '' AS reference, '' AS extra, 0 AS amount_hit, d.ordinal AS ordinal
            FROM documents_documentidentity d JOIN transfers_transfer t ON t.identity_id=d.id
            LEFT JOIN ledger_account fa ON fa.id=t.from_account_id AND fa.company_id=d.company_id
            LEFT JOIN ledger_account ta ON ta.id=t.to_account_id AND ta.company_id=d.company_id
            WHERE d.company_id=?1 AND d.kind='TRF'",
        Kind::TaxPayment => "SELECT t.id AS id, d.number AS number, '' AS alt, t.date AS date, t.agency_name AS name,
            t.tax_amount+t.penalty_amount AS amount,
            CASE WHEN EXISTS(SELECT 1 FROM salestax_paymentvoid z WHERE z.payment_id=t.id) THEN 'VOID' ELSE 'RECORDED' END AS status,
            t.memo AS memo, t.reference AS reference, '' AS extra, 0 AS amount_hit, d.ordinal AS ordinal
            FROM documents_documentidentity d JOIN salestax_payment t ON t.identity_id=d.id
            WHERE d.company_id=?1 AND d.kind='STP' AND t.company_id=d.company_id",
        // Journal entries made by hand (and their reversals); document journals are found through
        // their documents. A line's description or name is memo text; any line's amount matches.
        Kind::JournalEntry => "SELECT j.id AS id, printf('J%06d', j.number) AS number, j.entry_number AS alt, j.date AS date, '' AS name,
            NULL AS amount,
            CASE WHEN EXISTS(SELECT 1 FROM ledger_journal z WHERE z.reversal_of_id=j.id) THEN 'REVERSED' ELSE j.status END AS status,
            j.memo AS memo, '' AS reference,
            coalesce((SELECT group_concat(l.description || ' ' || l.name, ' ') FROM ledger_journalline l WHERE l.journal_id=j.id),'') AS extra,
            (?4 IS NOT NULL AND EXISTS(SELECT 1 FROM ledger_journalline l WHERE l.journal_id=j.id AND (l.debit=?4 OR l.credit=?4))) AS amount_hit,
            j.number AS ordinal
            FROM ledger_journal j WHERE j.company_id=?1 AND j.source_id IS NULL",
        _ => return None,
    })
}



/// Wraps a document source with the match and the order. Parameters: ?1 company, ?2 lowercased text,
/// ?3 the number's digits (or NULL), ?4 the amount in cents (or NULL), ?5 text matching allowed,
/// ?6 the row limit, ?7 the query is all digits (partial-number matching is then left out: `10`
/// would match every number with a 1 and a 0 in it).
fn document_query(source: &str) -> String {
    format!(
        "SELECT id, number, alt, date, name, amount, status, memo, reference, exact FROM (
           SELECT *, (fb_lower(number)=?2 OR (alt<>'' AND fb_lower(alt)=?2)
              OR (?3 IS NOT NULL AND ({num}=?3 OR (alt<>'' AND {alt}=?3)))) AS exact
           FROM ({source})
         ) WHERE exact
            OR (?5 AND (instr(fb_lower(name),?2)>0 OR instr(fb_lower(memo),?2)>0 OR instr(fb_lower(reference),?2)>0
                OR instr(fb_lower(extra),?2)>0 OR (NOT ?7 AND (instr(fb_lower(number),?2)>0 OR instr(fb_lower(alt),?2)>0))))
            OR (?4 IS NOT NULL AND (amount=?4 OR amount_hit))
         ORDER BY exact DESC, date DESC, ordinal DESC LIMIT ?6",
        num = trailing_digits!("number"),
        alt = trailing_digits!("alt"),
    )
}

/// Names: customers and vendors by name, contact, email or phone; accounts by name or code; items by
/// name or description. Parameters as for documents, plus ?8 the query's digits for phone numbers.
fn name_query(kind: Kind) -> Option<&'static str> {
    Some(match kind {
        Kind::Customer | Kind::Vendor => match kind {
            Kind::Customer => party_query!("directory_customer"),
            _ => party_query!("directory_vendor"),
        },
        Kind::Account => "SELECT id, code, '', '', name, NULL, CASE WHEN active THEN 'ACTIVE' ELSE 'INACTIVE' END, kind, '',
              (fb_lower(code)=?2 OR fb_lower(name)=?2) AS exact
            FROM ledger_account WHERE company_id=?1 AND ?5
              AND (instr(fb_lower(name),?2)>0 OR instr(fb_lower(code),?2)=1)
            ORDER BY exact DESC, active DESC, code, name LIMIT ?6",
        Kind::Item => "SELECT id, '', '', '', code, sale_rate, CASE WHEN active THEN 'ACTIVE' ELSE 'INACTIVE' END, description, '',
              (fb_lower(code)=?2) AS exact
            FROM directory_item WHERE company_id=?1 AND ?5
              AND (instr(fb_lower(code),?2)>0 OR instr(fb_lower(description),?2)>0)
            ORDER BY exact DESC, active DESC, fb_lower(code) LIMIT ?6",
        _ => return None,
    })
}


/// Search company `c` for `terms`, at most `per_kind` results of each kind (most recent first for
/// documents; best match, then alphabetical, for names). Only kinds with results are returned.
pub fn find(book: &mut Book, actor: UserId, c: CompanyId, terms: &Terms, per_kind: usize) -> FbResult<Vec<Group>> {
    let per_kind = per_kind.clamp(1, 50);
    book.read(|q| {
        services::access(q, actor, c)?;
        let limit = i64::try_from(per_kind).unwrap_or(50) + 1;
        let mut groups = Vec::new();
        use fb_db::rusqlite::types::Value as Sql;
        let text = |v: &Option<String>| v.clone().map_or(Sql::Null, Sql::Text);
        // ?1..?8 (see `document_query` and `name_query`); each statement binds those it uses.
        let values = [
            Sql::Text(c.to_db()),
            Sql::Text(terms.lower.clone()),
            text(&terms.digits),
            terms.amount.map_or(Sql::Null, Sql::Integer),
            Sql::Integer(terms.contains.into()),
            Sql::Integer(limit),
            Sql::Integer(terms.digits_only().into()),
            text(&terms.phone),
        ];
        for kind in Kind::ALL {
            let sql = match (document_source(kind), name_query(kind)) {
                (Some(source), _) => document_query(source),
                (None, Some(sql)) => sql.to_owned(),
                (None, None) => continue,
            };
            let mut st = q.prepare(&sql)?;
            for i in 1..=st.parameter_count() {
                st.raw_bind_parameter(i, &values[i - 1])?;
            }
            let mut rows = st.raw_query();
            let mut hits = Vec::new();
            while let Some(r) = rows.next()? {
                let memo: String = r.get(7)?;
                let reference: String = r.get(8)?;
                hits.push(Hit {
                    kind,
                    id: r.get(0)?,
                    number: r.get(1)?,
                    alt: r.get(2)?,
                    date: r.get(3)?,
                    name: r.get(4)?,
                    amount: r.get::<_, Option<i64>>(5)?.map(Money::from_db),
                    status: r.get(6)?,
                    detail: first_line(if memo.trim().is_empty() { &reference } else { &memo }),
                    exact: r.get(9)?,
                });
            }
            drop(rows);
            let more = hits.len() > per_kind;
            hits.truncate(per_kind);
            for hit in &mut hits {
                settle(q, hit)?;
            }
            if !hits.is_empty() {
                groups.push(Group { kind, hits, more });
            }
        }
        Ok(groups)
    })
}

/// Payment state for open invoices and bills, and the amount of a journal entry (its debits). Sums are
/// taken in Rust with overflow checks, as the lists do.
fn settle(q: &(impl Query + ?Sized), hit: &mut Hit) -> FbResult<()> {
    let sum = |sql: &str, id: &str| -> FbResult<i64> {
        let mut st = q.prepare(sql)?;
        let values = st.query_map([id], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
        values.into_iter().try_fold(0i64, |s, v| s.checked_add(v)).ok_or_else(|| crate::FbError::validation("Amounts exceed supported limits."))
    };
    match hit.kind {
        Kind::Invoice | Kind::Bill if hit.status == "OPEN" => {
            let paid = if hit.kind == Kind::Invoice {
                crate::sales::invoice_settled(q, &hit.id)?
            } else {
                sum("SELECT amount FROM fb_payables_live_allocations WHERE bill_id=?1", &hit.id)?
            };
            let total = hit.amount.and_then(Money::to_db).unwrap_or(0);
            if paid > 0 {
                hit.status = if paid >= total { "PAID" } else { "PARTIAL" }.into();
            }
        }
        Kind::JournalEntry => {
            hit.amount = Some(Money::from_db(sum("SELECT debit FROM ledger_journalline WHERE journal_id=?1", &hit.id)?));
        }
        _ => {}
    }
    Ok(())
}

fn first_line(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let mut out: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_bounded_and_read_numbers_and_amounts() {
        assert_eq!(Terms::parse("  "), Err(TermsError::Empty));
        assert_eq!(Terms::parse("a"), Err(TermsError::TooShort));
        assert_eq!(Terms::parse(&"x".repeat(MAX_CHARS + 1)), Err(TermsError::TooLong));
        assert!(Terms::parse(&"x".repeat(MAX_CHARS)).is_ok());
        // One digit still finds document number 7, but matches no text.
        let seven = Terms::parse("7").unwrap();
        assert_eq!((seven.digits.as_deref(), seven.amount, seven.contains), (Some("7"), Some(700), false));
        let number = Terms::parse("001042").unwrap();
        assert_eq!((number.digits.as_deref(), number.amount), (Some("1042"), Some(104200)));
        assert_eq!(Terms::parse("INV-001042").unwrap().digits, None);
        for (text, cents) in [("84", Some(8400)), ("84.5", Some(8450)), ("84.50", Some(8450)), ("$1,234.50", Some(123450)),
            ("1,234", Some(123400)), ("1,23", None), ("12,3456", None), ("84.505", None), ("0", None), ("0.00", None), ("abc", None), ("-5", None)] {
            assert_eq!(amount(text), cents, "{text}");
        }
        assert_eq!(Terms::parse("(555) 010-1234").unwrap().phone.as_deref(), Some("5550101234"));
        assert_eq!(Terms::parse("Lakeside 12").unwrap().phone, None);
        assert_eq!(Terms::parse("ÉCOLE").unwrap().lower, "école");
    }
}

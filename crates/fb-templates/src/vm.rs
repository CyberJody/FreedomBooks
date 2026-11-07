//! View models: plain data each ported screen renders. Field names follow the Django template context
//! (`company.pk`, `page.paginator.num_pages`, `row.line.journal`) so the templates stay a 1:1 port.
//! Money arrives as display strings (`191.25`), dates as ISO text, ids as opaque strings.
use crate::forms::BoundForm;
use crate::nav::Nav;
use serde::Serialize;

/// What Django's context processors and the shell add to every page.
#[derive(Clone, Debug, Serialize, Default)]
pub struct Shell {
    pub company: Option<CompanyVm>,
    pub user: UserVm,
    pub nav: Nav,
    pub fb_local_preview: bool,
    pub fb_banner: String,
    /// Plugin surfaces shown (dev/test builds); release builds hide them.
    pub plugins: bool,
    pub bankfeeds: bool,
}

/// The shell banner the desktop app shows (the frozen app's `ALPHA_BANNER`).
pub const ALPHA_BANNER: &str = "Alpha: keep your real books in your current software.";

#[derive(Clone, Debug, Serialize, Default)]
pub struct CompanyVm {
    pub pk: String,
    pub name: String,
    pub currency: String,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct UserVm {
    pub username: String,
    pub is_authenticated: bool,
}

/// `django.core.paginator.Page`, as far as the templates read it.
#[derive(Clone, Debug, Serialize, Default)]
pub struct Page<T: Serialize> {
    pub object_list: Vec<T>,
    pub number: usize,
    pub paginator: Paginator,
    pub has_previous: bool,
    pub has_next: bool,
    pub previous_page_number: usize,
    pub next_page_number: usize,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct Paginator {
    pub num_pages: usize,
    pub count: usize,
}

impl<T: Serialize + Clone> Page<T> {
    /// `Paginator(rows, per_page).get_page(number)`: an invalid or out-of-range page falls back (first page
    /// for junk, last page past the end), as Django's `get_page` does. An empty list is one empty page.
    pub fn of(rows: &[T], per_page: usize, requested: Option<&str>) -> Page<T> {
        let count = rows.len();
        let num_pages = if count == 0 { 1 } else { count.div_ceil(per_page) };
        let number = match requested.and_then(crate::forms::python_integer) {
            Some(n) if n >= 1 => (n as usize).min(num_pages),
            Some(_) => num_pages,
            _ => 1,
        };
        let start = (number - 1) * per_page;
        let object_list = rows.iter().skip(start).take(per_page).cloned().collect();
        Page {
            object_list,
            number,
            paginator: Paginator { num_pages, count },
            has_previous: number > 1,
            has_next: number < num_pages,
            previous_page_number: number.saturating_sub(1).max(1),
            next_page_number: (number + 1).min(num_pages),
        }
    }
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct AccountVm {
    pub pk: String,
    pub code: String,
    pub name: String,
    /// `str(account)`: `code · name`.
    pub label: String,
    pub get_detail_type_display: String,
    pub is_contra: bool,
    pub description: String,
    /// `DEBIT` / `CREDIT`.
    pub normal_balance: String,
    pub section_balance: String,
    pub active: bool,
    pub postable: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct HomeVm {
    #[serde(flatten)]
    pub shell: Shell,
    pub banks: Vec<AccountVm>,
    pub more_banks: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct AccountsVm {
    #[serde(flatten)]
    pub shell: Shell,
    pub accounts: Vec<AccountVm>,
    pub page: Page<AccountVm>,
    pub by_code: bool,
    pub query_string: String,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct SourceVm {
    pub label: String,
    pub number: String,
    pub party: String,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct SourceRefVm {
    pub number: String,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct JournalRefVm {
    /// `str(journal)`: `J000001`.
    pub label: String,
    pub date: String,
    pub memo: String,
    pub source_id: Option<String>,
    /// `journal.source`: the document identity (its number) when the kind has no resolver.
    pub source: Option<SourceRefVm>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct LineVm {
    pub journal_id: String,
    pub journal: JournalRefVm,
    pub debit: String,
    pub credit: String,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct RegisterRowVm {
    pub line: LineVm,
    pub source: Option<SourceVm>,
    pub reversal: bool,
    pub balance: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RegisterVm {
    #[serde(flatten)]
    pub shell: Shell,
    pub account: AccountVm,
    pub start: Option<String>,
    pub end: Option<String>,
    pub opening: String,
    pub closing: String,
    pub rows: Vec<RegisterRowVm>,
    pub page: Page<RegisterRowVm>,
    pub query_string: String,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct InvoiceRowVm {
    pub invoice_id: String,
    pub number: String,
    pub customer_label: String,
    pub date: String,
    pub due_date: String,
    pub payment_state: String,
    pub total: String,
    pub currency: String,
    pub outstanding: Option<String>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct TrackerTotalsVm {
    pub open_money: String,
    pub overdue_money: String,
    pub paid_recent_money: String,
    pub draft_count: i64,
    pub currency: String,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct TrackerLinksVm {
    pub open: String,
    pub overdue: String,
    pub paid_recent: String,
    pub drafts: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct InvoiceListVm {
    #[serde(flatten)]
    pub shell: Shell,
    pub filter_form: BoundForm,
    pub page: Page<InvoiceRowVm>,
    pub query_string: String,
    pub tracker_totals: Option<TrackerTotalsVm>,
    pub tracker_links: Option<TrackerLinksVm>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct FormsetVm {
    pub management_form: String,
    pub non_form_errors: String,
    pub forms: Vec<BoundForm>,
    pub empty_form: BoundForm,
    pub total_error_count: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct InvoiceFormVm {
    #[serde(flatten)]
    pub shell: Shell,
    /// The rendered command-token input (replaces `{% csrf_token %}`).
    pub csrf_token: String,
    pub invoice: Option<serde_json::Value>,
    pub current: Option<serde_json::Value>,
    pub form: BoundForm,
    pub formset: FormsetVm,
    pub catalog: serde_json::Value,
    pub customer_tax: serde_json::Value,
}

/// First run and app lock (desktop only; Django had a login page instead).
#[derive(Clone, Debug, Serialize, Default)]
pub struct GateVm {
    #[serde(flatten)]
    pub shell: Shell,
    pub errors: Vec<String>,
    pub needs_password: bool,
    pub password_reason: String,
    pub owner_name: String,
    pub company_name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_falls_back_like_get_page() {
        let rows: Vec<i32> = (1..=60).collect();
        let p = Page::of(&rows, 25, Some("2"));
        assert_eq!((p.number, p.paginator.num_pages, p.object_list.len(), p.has_next, p.has_previous), (2, 3, 25, true, true));
        assert_eq!(Page::of(&rows, 25, Some("junk")).number, 1);
        assert_eq!(Page::of(&rows, 25, Some("99")).number, 3);
        assert_eq!(Page::of(&rows, 25, Some("0")).number, 3);
        let empty = Page::<i32>::of(&[], 25, None);
        assert_eq!((empty.number, empty.paginator.num_pages, empty.paginator.count), (1, 1, 0));
    }
}

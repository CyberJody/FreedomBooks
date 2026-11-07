"""Use the shared reminders query and correct the activity view's reversal table name."""
from pathlib import Path
p=Path('crates/fb-app/src/screens/d3/activity.rs');s=p.read_text();assert 'payables_billpaymentreversal' in s;p.write_text(s.replace('payables_billpaymentreversal','payables_paymentreversal'))
p=Path('crates/fb-app/src/screens.rs');s=p.read_text();a=s.index('fn overdue_count(');b=s.index('\nfn money(',a)
s=s[:a]+'''fn overdue_count(book: &mut Book, actor: UserId, c: CompanyId, today: chrono::NaiveDate) -> AppResult<i64> {
    let count=fb_core::reminders::overdue_count(book,actor,c,&today.to_string())?;
    i64::try_from(count).map_err(|_|AppError::Invalid("Reminder count overflow.".into()))
}
''' + s[b:];p.write_text(s)

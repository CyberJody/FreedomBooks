mod common;
use common::*;
use fb_core::{CompanyId, services, starter_chart::Choice};
use fb_db::rusqlite::params;

fn account_count(f: &mut Fx, c: CompanyId) -> i64 {
    f.book
        .read(|tx| {
            tx.query_row(
                "SELECT count(*) FROM ledger_account WHERE company_id=?1",
                [c.to_db()],
                |r| r.get(0),
            )
        })
        .unwrap()
}

#[test]
fn starter_chart_is_complete_audited_and_scoped_to_the_new_company() {
    let mut f = Fx::new("starter-chart");
    let existing = f.company("Existing");
    f.account(existing, "1000", "Keep this bank", "ASSET");
    let c = services::create_company_with_chart(
        &mut f.book,
        f.user,
        "Standard",
        "USD",
        true,
        Choice::Standard,
    )
    .unwrap();
    assert_eq!(account_count(&mut f, c.id), 34);
    assert_eq!(account_count(&mut f, existing), 1);
    f.book.read(|tx| -> Result<(), fb_core::FbError> {
        let bad: i64 = tx.query_row("SELECT count(*) FROM ledger_account WHERE company_id=?1 AND (active=0 OR postable=0)", [c.id.to_db()], |r| r.get(0))?;
        assert_eq!(bad, 0);
        let events: i64 = tx.query_row("SELECT count(*) FROM ledger_auditevent WHERE company_id=?1 AND action='account.saved' AND actor_id=?2", params![c.id.to_db(), f.user.to_db()], |r| r.get(0))?;
        assert_eq!(events, 34);
        let detail: String = tx.query_row("SELECT detail FROM ledger_auditevent WHERE company_id=?1 AND action='company.starter_chart_created'", [c.id.to_db()], |r| r.get(0))?;
        let detail: serde_json::Value = serde_json::from_str(&detail).unwrap();
        assert_eq!(detail["chart"], "standard");
        assert_eq!(detail["version"], 1);
        assert_eq!(detail["accounts"].as_array().unwrap().len(), 34);
        assert_eq!(tx.query_row("SELECT count(*) FROM ledger_journal WHERE company_id=?1", [c.id.to_db()], |r| r.get::<_, i64>(0))?, 0);
        Ok(())
    }).unwrap();
    let empty = services::create_company_with_chart(
        &mut f.book,
        f.user,
        "Empty",
        "USD",
        true,
        Choice::Empty,
    )
    .unwrap();
    assert_eq!(account_count(&mut f, empty.id), 0);
    assert_eq!(account_count(&mut f, c.id), 34);
}

#[test]
fn starter_chart_rolls_back_company_members_accounts_and_audits_on_every_late_failure() {
    for (name, trigger) in [
        (
            "account",
            "CREATE TRIGGER fail_chart BEFORE INSERT ON ledger_account WHEN NEW.code='6190' BEGIN SELECT RAISE(ABORT,'injected last account failure'); END",
        ),
        (
            "account-audit",
            "CREATE TRIGGER fail_chart BEFORE INSERT ON ledger_auditevent WHEN NEW.action='account.saved' AND json_extract(NEW.detail,'$.after.code')='6190' BEGIN SELECT RAISE(ABORT,'injected account audit failure'); END",
        ),
        (
            "chart-audit",
            "CREATE TRIGGER fail_chart BEFORE INSERT ON ledger_auditevent WHEN NEW.action='company.starter_chart_created' BEGIN SELECT RAISE(ABORT,'injected final audit failure'); END",
        ),
    ] {
        let mut f = Fx::new(name);
        f.book.connection().execute_batch(trigger).unwrap();
        let result = services::create_company_with_chart(
            &mut f.book,
            f.user,
            "Must roll back",
            "USD",
            true,
            Choice::Standard,
        );
        let expected = match name {
            "account" => "injected last account failure",
            "account-audit" => "injected account audit failure",
            "chart-audit" => "injected final audit failure",
            _ => unreachable!(),
        };
        let error = result.expect_err("injected write must fail");
        assert!(error.message().contains(expected), "{name}: {error:?}");
        f.book
            .read(|tx| -> Result<(), fb_core::FbError> {
                for table in [
                    "ledger_company",
                    "ledger_company_members",
                    "ledger_account",
                    "ledger_auditevent",
                ] {
                    // Company creation is the only write in this fixture after the operator was created.
                    let count: i64 =
                        tx.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?;
                    assert_eq!(count, 0, "{name}: left rows in {table}");
                }
                Ok(())
            })
            .unwrap();
    }
}

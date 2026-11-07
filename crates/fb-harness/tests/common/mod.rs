use serde_json::{Value, json};
pub fn dump() -> Value {
    let mut dump = json!({
        "attachments":{"rows":[],"results":{},"queries":[]},
        "assistant":{"connections":[],"readings":[],"results":{}},
        "checks":{"check":[],"checkrevision":[],"checkline":[],"transfer":[],"transfervoid":[],"numbers":{}},
        "prepayments":{"details":[],"applications":[],"refunds":[]},
        "credits":{"creditmemo":[],"creditmemoline":[],"creditmemovoid":[],"creditapplication":[],"creditapplicationline":[],"creditunapplication":[],"creditrefund":[],"creditrefundvoid":[],"balances":[]},
        "sales":{"estimates":[],"invoices":[],"receipts":[],"balances":[]},
        "banking":{"deposits":[],"deposit_lines":[],"reversals":[],"reconciliations":[],"statuses":[],"reconciliation_lines":[],"registers":{},"screens":{},"reports":{}},
        "bankfeeds":{"connections":[],"accounts":[],"transactions":[],"rules":[],"results":{}},
        "payables":{"bill":[],"billrevision":[],"billline":[],"billpayment":[],"paymentallocation":[],"paymentreversal":[],"balances":[],"tracker":[],"calendar":[]},
        "l2a": {"directory":[],"tax":[],"identities":[],"sequences":[],"snapshots":[],"results":{},"source_journals":[],
            "settings":{"c1":{"charge_tax":false,"default_item_id":null,"owed_basis":"ACCRUAL","filing_frequency":"MONTHLY","bulk_step":"","revision":0}},
            "setup":{"c1":{"receivables":null,"payables":null,"undeposited":null,"collected_tax":null,"customer_deposits":null,"revision":0,"banks":[]}}},
        "outcomes": [{"audit_counts":{"c1":1}, "step": 0, "op": "save_draft", "ok": false, "kind": "ValidationError", "message": "Enter a valid monetary amount."}],
        "companies": [{"ref": "c1", "name": "C", "currency": "USD", "first_run_open": false, "bill_reminder_days": 10, "members": ["owner"]}],
        "accounts": [
          {"ref":"a","company":"c1","code":"1","name":"Cash","kind":"ASSET","detail_type":"BANK","is_contra":false,"description":"","active":true,"postable":true},
          {"ref":"b","company":"c1","code":"2","name":"Equity","kind":"EQUITY","detail_type":"EQUITY","is_contra":false,"description":"","active":true,"postable":true}],
        "journals": [{"ref":"j1","company":"c1","date":"2026-01-01","memo":"Reversal of J000007: x","status":"POSTED","entry_number":"","adjusting":false,"revision":1,"reversal_of":"j0","request_key":null,"created_by":"owner","posted_by":"owner",
          "lines":[{"position":1,"account":"a","debit":101,"credit":0,"description":"","name":""}]}],
        "display": {"j0": "J000007", "j1": "J000008"},
        "registers": {"a": {"opening": 0, "closing": 101, "rows": [["j1", 1, 101], ["j2", 1, 202]]},"b":{"opening":0,"closing":0,"rows":[]}},
        "trial_balances": {"c1": {"2026-12-31": {"rows": [["a", 101, 0], ["b", 0, 101]], "debit": 101, "credit": 101}}},
        "audit": {"c1": [{"action":"journal.posted","actor":"owner","actor_name":"owner","target":"journal:j1","detail":{}}]}
    });
    // Empty companies still have complete payables tracker/calendar reports.
    dump["payables"]["tracker"] = json!([{
        "company_id":"c1", "filtered":[],
        "totals":{"open_money":"0.00", "overdue_money":"0.00", "paid_recent_money":"0.00",
                  "draft_count":0, "currency":"USD", "today":"2026-11-23"}
    }]);
    let days: Vec<Value> = (1..=30)
        .map(|day| json!({"date":format!("2026-11-{day:02}"), "entries":[], "total":"0.00"}))
        .collect();
    let mut cells = vec![Value::Null; 6]; // November 1, 2026 is Sunday.
    cells.extend(days.clone());
    while !cells.len().is_multiple_of(7) {
        cells.push(Value::Null);
    }
    let weeks: Vec<Value> = cells.chunks(7).map(|week| json!(week)).collect();
    dump["payables"]["calendar"] = json!([{
        "company_id":"c1", "data":{
            "entries":[], "days":days, "weeks":weeks, "agenda":[], "day_totals":{},
            "month_total":"0.00", "month_label":"November 2026",
            "weekdays":["Monday","Tuesday","Wednesday","Thursday","Friday","Saturday","Sunday"]
        }
    }]);
    let mut original = dump["journals"][0].clone();
    original["ref"] = json!("j0");
    original["reversal_of"] = Value::Null;
    original["memo"] = json!("Original");
    original["entry_number"] = json!("JE1");
    dump["journals"].as_array_mut().unwrap().push(original);
    dump
}

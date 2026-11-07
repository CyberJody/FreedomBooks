"""Extend the genuine-source proof to every document reversal owner."""
from pathlib import Path
p=Path(__file__).resolve().parents[1]/'crates/fb-core/examples/dfix3_upgrade.rs'
t=p.read_text()
t=t.replace('accounts, backups::','accounts, banking, credits, prepayments, transfers, backups::')
t=t.replace('let ap=a("200","LIABILITY");', 'let second=a("101","ASSET"); let liability=a("230","LIABILITY"); let ap=a("200","LIABILITY");')
t=t.replace('"collected_tax":tax.to_db(),"banks":[bank.to_db()]', '"collected_tax":tax.to_db(),"customer_deposits":liability.to_db(),"banks":[bank.to_db(),second.to_db()]')
needle='''        assert!(book.check().unwrap().is_empty());
        let data=json!'''
replacement='''        let mut extra=json!({});
        let mut sale=inv.clone();sale["request_key"]=json!(fb_ids::new_v7().to_string());sale["method"]=json!("CASH");sale["destination"]=json!("BANK");sale["bank_id"]=json!(bank.to_db());
        let review=sales::review_sales_receipt(&mut book,u,c,&sale).unwrap();
        extra["sale"]=sales::record_sales_receipt(&mut book,u,c,&sale,Some(&review)).unwrap()["id"].clone();
        sale["request_key"]=json!(fb_ids::new_v7().to_string());sale["destination"]=json!("UNDEPOSITED");sale["bank_id"]=Value::Null;
        let review=sales::review_sales_receipt(&mut book,u,c,&sale).unwrap();
        let funds=sales::record_sales_receipt(&mut book,u,c,&sale,Some(&review)).unwrap();
        let p=json!({"request_key":fb_ids::new_v7().to_string(),"date":"2026-10-02","bank_id":bank.to_db(),"receipt_ids":[funds["id"]],"memo":LEGACY});
        let review=banking::review_deposit(&mut book,u,c,&p).unwrap();
        extra["bank deposit"]=banking::record_deposit(&mut book,u,c,&p,&review).unwrap()["id"].clone();
        let mut credit=inv.clone();credit["request_key"]=json!(fb_ids::new_v7().to_string());
        let review=credits::review_credit_memo(&mut book,u,c,&credit).unwrap();
        let memo=credits::record_credit_memo(&mut book,u,c,&credit,Some(&review)).unwrap();extra["credit memo"]=memo["id"].clone();
        let refund=json!({"request_key":fb_ids::new_v7().to_string(),"date":"2026-10-02","bank_id":bank.to_db(),"amount":"1.00","method":"CASH","memo":LEGACY});
        let review=credits::review_refund(&mut book,u,c,memo["id"].as_str().unwrap(),&refund).unwrap();
        extra["credit refund"]=credits::record_refund(&mut book,u,c,memo["id"].as_str().unwrap(),&refund,Some(&review)).unwrap()["id"].clone();
        let p=json!({"request_key":fb_ids::new_v7().to_string(),"customer_id":customer["id"],"date":"2026-10-01","amount":"5.00","method":"CASH","destination":"BANK","bank_id":bank.to_db(),"memo":LEGACY,"order_reference":LEGACY});
        let review=prepayments::review_deposit(&mut book,u,c,&p).unwrap();
        let deposit=prepayments::record_deposit(&mut book,u,c,Some(&review),&p).unwrap();extra["customer deposit"]=deposit["id"].clone();
        let mut refund=refund.clone();refund["request_key"]=json!(fb_ids::new_v7().to_string());
        let review=prepayments::review_refund(&mut book,u,c,deposit["id"].as_str().unwrap(),&refund).unwrap();
        extra["deposit refund"]=prepayments::record_refund(&mut book,u,c,deposit["id"].as_str().unwrap(),Some(&review),&refund).unwrap()["id"].clone();
        let p=json!({"request_key":fb_ids::new_v7().to_string(),"date":"2026-10-01","amount":"1.00","from_account_id":bank.to_db(),"to_account_id":second.to_db(),"memo":LEGACY});
        let review=transfers::review_transfer(&mut book,u,c,&p).unwrap();extra["transfer"]=transfers::record_transfer(&mut book,u,c,&p,&review).unwrap()["id"].clone();
        let mut inv=inv.clone();inv["request_key"]=json!(fb_ids::new_v7().to_string());
        let posted=sales::save_invoice(&mut book,u,c,&inv).unwrap();let id=posted["id"].as_str().unwrap();
        let review=sales::review_invoice(&mut book,u,c,id,Some(1)).unwrap();sales::post_invoice(&mut book,u,c,id,Some(1),Some(&review)).unwrap();
        let p=json!({"request_key":fb_ids::new_v7().to_string(),"customer_id":customer["id"],"date":"2026-10-02","amount":"1.00","method":"CASH","destination":"BANK","bank_id":bank.to_db(),"memo":LEGACY,"reference":LEGACY,"allocations":[{"invoice_id":id,"amount":"1.00"}]});
        let review=sales::review_receipt(&mut book,u,c,&p).unwrap();extra["customer receipt"]=sales::record_receipt(&mut book,u,c,&p,Some(&review)).unwrap()["id"].clone();
        let mut bill=bill.clone();bill["request_key"]=json!(fb_ids::new_v7().to_string());bill["reference"]=json!("UPGRADE-2");
        let posted=payables::save_bill(&mut book,u,c,&bill).unwrap();let id=posted["id"].as_str().unwrap();
        let review=payables::review_bill(&mut book,u,c,id,Some(1)).unwrap();payables::post_bill(&mut book,u,c,id,Some(1),&review).unwrap();
        let p=json!({"request_key":fb_ids::new_v7().to_string(),"vendor_id":vendor["id"],"date":"2026-10-02","amount":"1.00","method":"CASH","bank_id":bank.to_db(),"memo":LEGACY,"reference":LEGACY,"allocations":[{"bill_id":id,"amount":"1.00"}]});
        let review=payables::review_payment(&mut book,u,c,&p).unwrap();extra["bill payment"]=payables::record_payment(&mut book,u,c,&p,&review).unwrap()["id"].clone();
        assert!(book.check().unwrap().is_empty());
        let data=json!'''
assert t.count(needle)==1
t=t.replace(needle,replacement).replace('"rows":rows(&book),','"rows":rows(&book),"extra":extra,')
needle='''        assert!(book.check().unwrap().is_empty());drop(book);'''
new='''        type Reverse = fn(&mut Book, UserId, CompanyId, &str, Option<i64>, &Value, &Value) -> fb_core::FbResult<Value>;
        for (name, reverse) in [
            ("credit refund",credits::void_refund as Reverse),
            ("deposit refund",prepayments::void_refund as Reverse),
            ("credit memo",credits::void_credit_memo as Reverse),
            ("customer deposit",prepayments::void_deposit as Reverse),
            ("bank deposit",banking::reverse_deposit as Reverse),
            ("sale",sales::void_sales_receipt as Reverse),
            ("customer receipt",sales::reverse_receipt as Reverse),
            ("bill payment",payables::reverse_payment as Reverse),
            ("transfer",transfers::void_transfer as Reverse),
        ] {
            reverse(&mut book,u,c,data["extra"][name].as_str().unwrap(),Some(1),&json!("2026-10-03"),&json!("Synthetic correction")).unwrap();
            println!("PASS {name} lifecycle copies legacy text");
        }
        assert!(book.check().unwrap().is_empty());drop(book);'''
assert t.count(needle)==1
p.write_text(t.replace(needle,new))

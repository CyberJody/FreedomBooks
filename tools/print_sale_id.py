"""Sales-receipt detail JSON omits its physical id; retain it for the ordered line lookup."""
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
p=ROOT/'crates/fb-app/src/screens/printing.rs';text=p.read_text()
old='''    fb_db::parse_unique_json_as(s(&row, "data")).map_err(|e| AppError::Invalid(e.to_string()))'''
new='''    let mut parsed: Value = fb_db::parse_unique_json_as(s(&row,"data")).map_err(|e| AppError::Invalid(e.to_string()))?;
    if parsed["id"].is_null() && !row["id"].is_null() { parsed["id"] = row["id"].clone(); }
    Ok(parsed)'''
assert text.count(old)==1;text=text.replace(old,new)
old='SELECT data FROM sales_salesreceipt WHERE receipt_id=?';assert text.count(old)==1;text=text.replace(old,'SELECT id,data FROM sales_salesreceipt WHERE receipt_id=?');p.write_text(text)
print('PRINT: sales receipt lines use their actual detail id')

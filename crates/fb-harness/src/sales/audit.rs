use crate::{Runner, audit::Ids};
use serde_json::{Value, json};

impl Runner<'_> {
    pub(crate) fn sales_audit_ids(&mut self, mut ids: Ids) -> Ids {
        let refs = self.l2_refs.clone();
        self.book
            .read(|r| -> fb_core::FbResult<()> {
                for (kind, table) in [("invoice", "sales_invoice"), ("receipt", "sales_receipt"), ("estimate","sales_estimate")] {
                let mut st = r.prepare(&format!("SELECT id FROM {table} ORDER BY id"))?;
                    let rows = st
                        .query_map([], |r| r.get::<_, String>(0))?
                        .collect::<Result<Vec<_>, _>>()?;
                    for (rank, id) in rows.into_iter().enumerate() {
                        let name = refs
                            .iter()
                            .filter(|(_, v)| **v == id)
                            .map(|(k, _)| k.clone())
                            .min()
                            .unwrap_or_else(|| format!("unreferenced-{kind}:{rank}"));
                        ids.entry(kind.into()).or_default().insert(id, name);
                    }
                }
                let mut conversions=r.prepare("SELECT i.identity_id,d.request_key FROM sales_estimateconversion x JOIN sales_invoice i ON i.id=x.invoice_id JOIN documents_documentidentity d ON d.id=i.identity_id")?;
                ids.insert("estimate_conversion_sources".into(),conversions.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<_,_>>()?);
                let mut st = r.prepare(
                    "SELECT id,kind FROM documents_documentidentity WHERE kind IN('SR','RCP')",
                )?;
                ids.insert(
                    "sales_account_roles".into(),
                    st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                        .collect::<Result<_, _>>()?,
                );
                Ok(())
            })
            .unwrap();
        ids.insert("journal".into(), self.journal_names());
        ids
    }
}
fn field(v: &mut Value, k: &str, kind: &str, ids: &Ids) {
    if let Some(v) = v.get_mut(k).filter(|v| !v.is_null()) {
        let raw = v
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| v.to_string());
        *v = json!(
            ids.get(kind)
                .and_then(|m| m.get(&raw))
                .cloned()
                .unwrap_or_else(|| format!("<unknown:{kind}:{raw}>"))
        );
    }
}
pub(crate) fn normalize(v: &mut Value, ids: &Ids) -> bool {
    let target = v["target"].as_str().unwrap_or("").to_owned();
    let Some((kind, raw)) = target.split_once(':') else {
        return false;
    };
    if kind == "documentidentity"
        && ids
            .get("estimate_conversion_sources")
            .and_then(|m| m.get(raw))
            .is_some_and(|key| v["detail"]["request_key"].as_str() == Some(key))
        && v["action"] == "document.identified"
    {
        v["detail"]["request_key"] = json!("generated-estimate-conversion");
    }
    if !["invoice", "receipt", "estimate"].contains(&kind) {
        // Role suffixes contain generated account IDs in the shared document audit.
        if kind == "documentidentity"
            && ids["sales_account_roles"].contains_key(raw)
            && v["action"] == "document.accounts"
            && let Some(a) = v["detail"]["accounts"].as_array_mut()
        {
            for row in a {
                if let Some((prefix, key)) = row["role"].as_str().and_then(|s| s.split_once(':'))
                    && let Some(name) = ids.get("account").and_then(|m| m.get(key))
                {
                    row["role"] = json!(format!("{prefix}:{name}"));
                }
            }
        }
        return false;
    }
    v["target"] = json!(format!(
        "{kind}:{}",
        ids.get(kind)
            .and_then(|m| m.get(raw))
            .cloned()
            .unwrap_or_else(|| format!("<unknown:{raw}>"))
    ));
    let detail = &mut v["detail"];
    field(detail, "journal_id", "journal", ids);
    field(detail, "estimate_id", "estimate", ids);
    if v["action"] == "invoice.converted" {
        v["detail"]["conversion_id"] = json!(format!(
            "conversion:{}",
            v["detail"]["estimate_id"].as_str().unwrap_or("?")
        ));
    }
    let detail = &mut v["detail"];
    if let Some(rows) = detail.get_mut("allocations").and_then(Value::as_array_mut) {
        for row in rows {
            field(row, "invoice_id", "invoice", ids);
        }
    }
    if let Some(rows) = detail.get_mut("lines").and_then(Value::as_array_mut) {
        for row in rows {
            field(row, "income_account_id", "account", ids);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_conversion_key_requires_the_exact_identity_action_and_stored_key() {
        let ids=Ids::from([
            ("estimate_conversion_sources".into(),[("generated-source".into(),"generated-key".into())].into()),
            ("sales_account_roles".into(),Default::default()),
        ]);
        let original=json!({"target":"documentidentity:generated-source","action":"document.identified","detail":{"request_key":"generated-key","memo":"generated-key"}});
        let mut actual=original.clone();normalize(&mut actual,&ids);
        assert_eq!(actual["detail"]["request_key"],"generated-estimate-conversion");
        assert_eq!(actual["detail"]["memo"],"generated-key");
        for (pointer,value) in [("/target","documentidentity:caller-source"),("/action","other.action"),("/detail/request_key","wrong-key")] {
            let mut altered=original.clone();*altered.pointer_mut(pointer).unwrap()=json!(value);
            let before=altered.clone();normalize(&mut altered,&ids);assert_eq!(altered,before);
        }
        let mut missing=original;missing["detail"].as_object_mut().unwrap().remove("request_key");
        let before=missing.clone();normalize(&mut missing,&ids);assert_eq!(missing,before);
    }
}

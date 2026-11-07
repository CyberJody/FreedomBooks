//! Attachment lifecycle, translated from attachments/{models,services}. Files are encrypted beside the book.
//! Removed rows retain their file reference for audit/backup history; no document status gates attachments.
pub mod sniff;
pub mod storage;
use crate::{Book, CompanyId, FbError, FbResult, UserId, documents, py, services, store};
use fb_db::{Query, Writer, rusqlite::params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use storage::{Store, Stored};
pub const MAX_PER_OWNER: i64 = 20;
pub const PANEL_REMOVED_ROWS: usize = 50;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Owner {
    Document(String),
    Journal(crate::JournalId),
}
impl Owner {
    pub fn key(&self) -> (&str, String) {
        match self {
            Self::Document(id) => ("document", id.clone()),
            Self::Journal(id) => ("journal", id.to_db()),
        }
    }
    fn columns(&self) -> (Option<String>, Option<String>) {
        match self {
            Self::Document(id) => (Some(id.clone()), None),
            Self::Journal(id) => (None, Some(id.to_db())),
        }
    }
}
#[derive(Clone, Debug)]
pub struct Limits {
    pub max_mb: u32,
    pub uploads_per_window: u32,
    pub company_max_mb: u32,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_mb: 10,
            uploads_per_window: 60,
            company_max_mb: 2048,
        }
    }
}
impl Limits {
    pub fn validate(&self) -> FbResult<()> {
        if !(1..=100).contains(&self.max_mb)
            || self.uploads_per_window == 0
            || self.company_max_mb == 0
        {
            return Err(FbError::refused("Invalid attachment limits."));
        }
        Ok(())
    }
}
pub struct Inspected {
    pub content_type: String,
    pub name: String,
    pub sha256: String,
}
pub fn inspect(data: &[u8], name: &str, limits: &Limits) -> FbResult<Inspected> {
    limits.validate()?;
    if data.len() as u64 > u64::from(limits.max_mb) * 1024 * 1024 {
        return Err(FbError::refused(format!(
            "The file is larger than {} MB, so it was not attached.",
            limits.max_mb
        )));
    }
    if data.is_empty() {
        return Err(FbError::refused(
            "The file is empty, so it was not attached.",
        ));
    }
    let t = sniff::sniff(data).ok_or_else(|| {
        FbError::refused(sniff::refusal(
            &data[..data.len().min(64)],
            &sniff::clean_name(name),
        ))
    })?;
    if sniff::problem(data, t).is_some() {
        return Err(FbError::refused(sniff::invalid_message(
            t,
            &sniff::clean_name(name),
        )));
    }
    Ok(Inspected {
        content_type: t.into(),
        name: sniff::display_name(name, t)?,
        sha256: storage::hex(&Sha256::digest(data)),
    })
}
pub fn document_owner(c: &(impl Query + ?Sized), co: CompanyId, id: &str) -> FbResult<Owner> {
    let d = documents::identity(c, co, id)
        .map_err(|_| FbError::denied("Not a document of this company."))?;
    if ![
        "EST", "INV", "RCP", "SR", "BILL", "BPAY", "DEP", "CHK", "TRF", "CM", "RFD", "CDP", "CDR",
    ]
    .contains(&d["kind"].as_str().unwrap_or(""))
    {
        return Err(FbError::denied("Not a document of this company."));
    }
    Ok(Owner::Document(id.into()))
}
pub fn journal_owner(
    c: &(impl Query + ?Sized),
    co: CompanyId,
    id: crate::JournalId,
) -> FbResult<Owner> {
    let j = store::journal_in(c, id, co)
        .map_err(|_| FbError::denied("Attachments go on manual journal entries only."))?;
    if !j.manual() || j.reversal_of_id.is_some() {
        return Err(FbError::denied(
            "Attachments go on manual journal entries only.",
        ));
    }
    Ok(Owner::Journal(id))
}
fn owner_check(c: &(impl Query + ?Sized), co: CompanyId, o: &Owner) -> FbResult<Owner> {
    match o {
        Owner::Document(id) => document_owner(c, co, id),
        Owner::Journal(id) => journal_owner(c, co, *id),
    }
}
fn target(c: &(impl Query + ?Sized), co: CompanyId, o: &Owner) -> FbResult<(String, String)> {
    if let Owner::Journal(id) = o {
        let j = store::journal_in(c, *id, co)?;
        return Ok((format!("journal:{id}"), j.display_number()));
    }
    let Owner::Document(id) = o else {
        return Err(FbError::refused("Invalid owner"));
    };
    let d = documents::identity(c, co, id)?;
    let (table, kind) = match d["kind"].as_str().unwrap_or("") {
        "INV" => ("sales_invoice", "invoice"),
        "RCP" | "SR" | "CDP" => ("sales_receipt", "receipt"),
        "BILL" => ("payables_bill", "bill"),
        "BPAY" => ("payables_billpayment", "billpayment"),
        "DEP" => ("banking_deposit", "deposit"),
        "CHK" => ("checks_check", "check"),
        "TRF" => ("transfers_transfer", "transfer"),
        "CM" => ("credits_creditmemo", "creditmemo"),
        "RFD" => ("credits_creditrefund", "creditrefund"),
        "CDR" => ("prepayments_depositrefund", "depositrefund"),
        "EST" => {
            return Ok((
                format!("estimate:{id}"),
                d["number"].as_str().unwrap_or("").into(),
            ));
        }
        _ => return Err(FbError::refused("Invalid document kind")),
    };
    let root: String = c.query_row(
        &format!("SELECT id FROM {table} WHERE identity_id=?"),
        [id],
        |r| r.get(0),
    )?;
    Ok((
        format!("{kind}:{root}"),
        d["number"].as_str().unwrap_or("").into(),
    ))
}
pub fn owner_of(row: &Value) -> FbResult<Owner> {
    if let Some(d) = row["document_id"].as_str() {
        Ok(Owner::Document(d.into()))
    } else {
        Ok(Owner::Journal(
            row["journal_id"]
                .as_str()
                .and_then(crate::JournalId::parse)
                .ok_or_else(|| FbError::refused("Invalid stored attachment owner"))?,
        ))
    }
}
fn rows(
    c: &(impl Query + ?Sized),
    sql: &str,
    values: &[&dyn fb_db::rusqlite::ToSql],
) -> FbResult<Vec<Value>> {
    let mut stmt = c.prepare(sql)?;
    let keys = stmt.column_names().map(str::to_owned).collect::<Vec<_>>();
    let out = stmt
        .query_map(values, |r| {
            let mut v = json!({});
            for (i, k) in keys.iter().enumerate() {
                v[k] = match r.get_ref(i)? {
                    fb_db::rusqlite::types::ValueRef::Null => Value::Null,
                    fb_db::rusqlite::types::ValueRef::Integer(n) if k == "deleted" => json!(n != 0),
                    fb_db::rusqlite::types::ValueRef::Integer(n) => json!(n),
                    fb_db::rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                    _ => Value::Null,
                };
            }
            Ok(v)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(out)
}
pub fn get(c: &(impl Query + ?Sized), co: CompanyId, id: &str) -> FbResult<Value> {
    rows(
        c,
        "SELECT * FROM attachments_attachment WHERE company_id=? AND id=?",
        &[&co.to_db(), &id],
    )?
    .pop()
    .ok_or_else(|| FbError::missing("Attachment"))
}
pub fn all(c: &(impl Query + ?Sized), co: CompanyId) -> FbResult<Vec<Value>> {
    rows(
        c,
        "SELECT * FROM attachments_attachment WHERE company_id=? ORDER BY uploaded_at,id",
        &[&co.to_db()],
    )
}
pub fn active(c: &(impl Query + ?Sized), co: CompanyId, o: &Owner) -> FbResult<Vec<Value>> {
    Ok(owner_rows(c, co, o)?
        .into_iter()
        .filter(|r| r["deleted"] == false)
        .collect())
}
fn owner_rows(c: &(impl Query + ?Sized), co: CompanyId, o: &Owner) -> FbResult<Vec<Value>> {
    owner_check(c, co, o)?;
    let (d, j) = o.columns();
    rows(
        c,
        "SELECT * FROM attachments_attachment WHERE company_id=? AND document_id IS ? AND journal_id IS ? ORDER BY uploaded_at,id",
        &[&co.to_db(), &d, &j],
    )
}
pub fn panel_rows(c: &(impl Query + ?Sized), co: CompanyId, o: &Owner) -> FbResult<Vec<Value>> {
    let mut rows = owner_rows(c, co, o)?;
    rows.sort_by_key(|r| r["deleted"] == true);
    rows.truncate(MAX_PER_OWNER as usize + PANEL_REMOVED_ROWS);
    Ok(rows)
}
pub fn removed(c: &(impl Query + ?Sized), co: CompanyId, o: &Owner) -> FbResult<Vec<Value>> {
    let mut rows = owner_rows(c, co, o)?
        .into_iter()
        .filter(|r| r["deleted"] == true)
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        (b["deleted_at"].as_str(), b["id"].as_str())
            .cmp(&(a["deleted_at"].as_str(), a["id"].as_str()))
    });
    Ok(rows)
}
fn stored(r: &Value) -> FbResult<Stored> {
    serde_json::from_value(r.clone()).map_err(|_| FbError::refused("Invalid attachment metadata"))
}
fn retry(
    c: &(impl Query + ?Sized),
    co: CompanyId,
    o: &Owner,
    key: &str,
    name: &str,
    sha: &str,
) -> FbResult<Option<Value>> {
    let Some(row) = rows(
        c,
        "SELECT * FROM attachments_attachment WHERE company_id=? AND request_key=?",
        &[&co.to_db(), &key],
    )?
    .pop() else {
        return Ok(None);
    };
    if row["deleted"] == true
        || owner_of(&row)? != *o
        || row["original_name"] != name
        || row["original_sha256"] != sha
    {
        return Err(FbError::refused(
            "This form was already used for another file. Reload the page and attach the file again.",
        ));
    }
    Ok(Some(row))
}
fn request_key(key: &str) -> FbResult<String> {
    uuid::Uuid::parse_str(key)
        .map(|u| u.to_string())
        .map_err(|_| {
            FbError::refused("This form is out of date. Reload the page and attach the file again.")
        })
}
pub fn check_limits(
    c: &(impl Query + ?Sized),
    actor: UserId,
    co: CompanyId,
    size: i64,
    l: &Limits,
) -> FbResult<()> {
    l.validate()?;
    let since = (chrono::Utc::now() - chrono::Duration::minutes(10))
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    let count:i64=c.query_row("SELECT count(*) FROM attachments_attachment WHERE company_id=? AND uploaded_by_id=? AND uploaded_at>=?",params![co.to_db(),actor.to_db(),since],|r|r.get(0))?;
    if count >= i64::from(l.uploads_per_window) {
        return Err(FbError::refused(
            "Too many files were attached in the last few minutes. Wait a few minutes and try again.",
        ));
    }
    let mut total = i128::from(size);
    for r in all(c, co)? {
        total = total
            .checked_add(i128::from(
                r["size"]
                    .as_i64()
                    .ok_or_else(|| FbError::refused("Invalid attachment size"))?,
            ))
            .ok_or_else(|| FbError::refused("Attachment size overflow"))?;
    }
    if total > i128::from(l.company_max_mb) * 1024 * 1024 {
        return Err(FbError::refused(format!(
            "This company's attachments have reached their storage limit of {} MB (removed attachments still count, because they are kept for the audit history and backups), so the file was not attached. The person who runs FreedomBooks can raise FB_ATTACHMENT_COMPANY_MAX_MB.",
            l.company_max_mb
        )));
    }
    Ok(())
}
fn cap(c: &(impl Query + ?Sized), co: CompanyId, o: &Owner) -> FbResult<()> {
    if active(c, co, o)?.len() >= MAX_PER_OWNER as usize {
        return Err(FbError::refused(
            "This document already has 20 attachments, the most it can have. Remove one before attaching another.",
        ));
    }
    Ok(())
}
fn audit(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    o: &Owner,
    row: &Value,
    action: &str,
) -> FbResult<()> {
    let user = services::access(tx, actor, co)?;
    let (target, _) = target(tx, co, o)?;
    let number = if action == "attachment.removed" {
        &row["deleted_number"]
    } else {
        &row["uploaded_number"]
    };
    let mut detail = json!({"attachment":row["id"],"name":row["original_name"],"number":number,"content_type":row["content_type"],"size":row["size"],"sha256":row["sha256"]});
    if action == "attachment.removed" {
        detail["reason"] = row["delete_reason"].clone();
    } else if row["original_sha256"] != row["sha256"] {
        detail["original_sha256"] = row["original_sha256"].clone();
    }
    services::audit(tx, &user, co, action, target, detail)?;
    Ok(())
}
/// Public add validates and writes the file before taking the writer capability. An insert failure cleans up only its new file.
#[allow(clippy::too_many_arguments)]
pub fn add(
    book: &mut Book,
    files: &Store,
    actor: UserId,
    co: CompanyId,
    o: &Owner,
    data: &[u8],
    name: &str,
    key: &str,
    l: &Limits,
) -> FbResult<(Value, bool)> {
    book.read(|c| services::access(c, actor, co))?;
    let key = request_key(key)?;
    let inspected = inspect(data, name, l)?;
    let (data, t, name) = reduce(data, &inspected)?;
    let earlier = book.read(|c| retry(c, co, o, &key, &name, &inspected.sha256))?;
    if let Some(row) = earlier {
        return Ok((row, false));
    }
    book.read(|c| {
        owner_check(c, co, o)?;
        check_limits(c, actor, co, data.len() as i64, l)?;
        cap(c, co, o)
    })?;
    let sha = storage::hex(&Sha256::digest(data.as_ref()));
    let existing = book
        .read(|c| {
            rows(
                c,
                "SELECT * FROM attachments_attachment WHERE company_id=? AND sha256=? LIMIT 1",
                &[&co.to_db(), &sha],
            )
        })?
        .pop();
    let (file, fresh) = if let Some(row) = existing {
        let file = stored(&row)?;
        files.open_read(co, &file)?;
        (file, false)
    } else {
        (files.write_atomic(co, data.as_ref(), &t)?, true)
    };
    let mut pending = Pending {
        store: files,
        name: &file.stored_name,
        armed: fresh,
    };
    let result =
        book.write(|tx| insert_tx(tx, actor, co, o, &file, &name, &inspected.sha256, &key, l));
    if result
        .as_ref()
        .is_ok_and(|(r, created)| *created && r["stored_name"] == file.stored_name)
    {
        pending.armed = false;
    }
    result
}
fn reduce<'a>(
    data: &'a [u8],
    i: &Inspected,
) -> FbResult<(std::borrow::Cow<'a, [u8]>, String, String)> {
    if !i.content_type.starts_with("image/") || data.len() <= 1536 * 1024 {
        return Ok((data.into(), i.content_type.clone(), i.name.clone()));
    }
    use image::ImageReader;
    let mut reader = ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .map_err(|_| FbError::refused("The photo could not be reduced."))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(30000);
    limits.max_image_height = Some(30000);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let photo = reader
        .decode()
        .map_err(|_| FbError::refused("The photo could not be reduced."))?
        .resize(2000, 2000, image::imageops::FilterType::Lanczos3)
        .to_rgb8();
    let mut out = vec![];
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80)
        .encode_image(&photo)
        .map_err(|_| FbError::refused("The photo could not be reduced."))?;
    let stem = i.name.rsplit_once('.').map_or(i.name.as_str(), |(s, _)| s);
    let name = sniff::display_name(&format!("{stem}.jpg"), "image/jpeg")?;
    Ok((out.into(), "image/jpeg".into(), name))
}
/// Stage a validated upload for a receipt reading, retaining its pre-reduction fingerprint.
/// The reading owns this file until adoption; an adoption refusal never removes it.
pub fn stage(
    files: &Store,
    co: CompanyId,
    data: &[u8],
    name: &str,
    limits: &Limits,
) -> FbResult<(Stored, String)> {
    let inspected = inspect(data, name, limits)?;
    let (data, content_type, name) = reduce(data, &inspected)?;
    let mut file = files.write_atomic(co, data.as_ref(), &content_type)?;
    file.original_sha256 = inspected.sha256;
    Ok((file, name))
}
/// Composable metadata adoption. The file is always re-authenticated first; the nested primitive is atomic even if its caller catches an error.
#[allow(clippy::too_many_arguments)]
pub fn adopt(
    book: &mut Book,
    files: &Store,
    actor: UserId,
    co: CompanyId,
    o: &Owner,
    file: &Stored,
    name: &str,
    key: &str,
    limits: &Limits,
) -> FbResult<(Value, bool)> {
    book.write(|tx| adopt_tx(tx, files, actor, co, o, file, name, key, limits))
}
#[allow(clippy::too_many_arguments)]
pub fn adopt_tx(
    tx: &Writer<'_>,
    files: &Store,
    actor: UserId,
    co: CompanyId,
    o: &Owner,
    file: &Stored,
    name: &str,
    key: &str,
    l: &Limits,
) -> FbResult<(Value, bool)> {
    services::access(tx, actor, co)?;
    let key = request_key(key)?;
    let data = files.open_read(co, file)?;
    let i = inspect(&data, name, l)?;
    if i.name != name || i.content_type != file.content_type || i.sha256 != file.sha256 {
        return Err(FbError::refused(
            "The receipt file no longer matches what was uploaded, so it was not attached.",
        ));
    }
    if file.original_sha256.len() != 64
        || !file
            .original_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
    {
        return Err(FbError::refused("Invalid original receipt fingerprint."));
    }
    insert_tx(tx, actor, co, o, file, name, &file.original_sha256, &key, l)
}
#[allow(clippy::too_many_arguments)]
fn insert_tx(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    o: &Owner,
    file: &Stored,
    name: &str,
    original_sha: &str,
    key: &str,
    l: &Limits,
) -> FbResult<(Value, bool)> {
    tx.atomic(||{
    let u=services::access(tx,actor,co)?;owner_check(tx,co,o)?;
    if let Some(row)=retry(tx,co,o,key,name,original_sha)?{return Ok((row,false));}
    cap(tx,co,o)?;check_limits(tx,actor,co,file.size,l)?;
    // A concurrent uploader can have committed identical bytes while this upload was on disk.
    let existing=rows(tx,"SELECT * FROM attachments_attachment WHERE company_id=? AND sha256=? LIMIT 1",&[&co.to_db(),&file.sha256])?.pop();
    let file=if let Some(row)=existing{stored(&row)?}else{file.clone()};
    let id=fb_ids::new_v7().to_string();let(d,j)=o.columns();let (_,number)=target(tx,co,o)?;
    tx.execute("INSERT INTO attachments_attachment(id,company_id,document_id,journal_id,original_name,stored_name,content_type,size,sha256,original_sha256,wrapped_key,wrap_nonce,nonce,uploaded_by_id,uploaded_by_name,uploaded_at,request_key,uploaded_number) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",params![id,co.to_db(),d,j,name,file.stored_name,file.content_type,file.size,file.sha256,original_sha,file.wrapped_key,file.wrap_nonce,file.nonce,actor.to_db(),u.username,py::now_utc(),key,number])?;
    let row=get(tx,co,&id)?;audit(tx,actor,co,o,&row,"attachment.added")?;tx.execute("INSERT INTO fb_seal VALUES('attachment_added',?)",[&id])?;Ok((row,true))
})
}
pub fn remove(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    id: &str,
    reason: &str,
) -> FbResult<(Value, Owner, bool)> {
    book.write(|tx| remove_tx(tx, actor, co, id, reason))
}
pub fn remove_tx(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    id: &str,
    reason: &str,
) -> FbResult<(Value, Owner, bool)> {
    tx.atomic(||{
    py::clean_free_text(reason)?;
    let u=services::access(tx,actor,co)?;let reason=reason.split(fb_money::py_isspace).filter(|s|!s.is_empty()).collect::<Vec<_>>().join(" ");
    if reason.is_empty(){return Err(FbError::refused("Give a reason for removing the attachment."));}
if reason.chars().count()>500{return Err(FbError::refused("The reason can be at most 500 characters."));}
    let row=get(tx,co,id)?;let o=owner_of(&row)?;owner_check(tx,co,&o)?;if row["deleted"]==true{return Ok((row,o,false));}let (_,number)=target(tx,co,&o)?;
    tx.execute("UPDATE attachments_attachment SET deleted=1,deleted_by_id=?,deleted_by_name=?,deleted_at=?,delete_reason=?,deleted_number=? WHERE id=?",params![actor.to_db(),u.username,py::now_utc(),reason,number,id])?;
    let row=get(tx,co,id)?;audit(tx,actor,co,&o,&row,"attachment.removed")?;tx.execute("INSERT INTO fb_seal VALUES('attachment_removed',?)",[id])?;Ok((row,o,true))
})
}
/// Every read verifies AEAD, plaintext hash, size, sniffed type and complete structure.
pub fn download(
    book: &mut Book,
    files: &Store,
    actor: UserId,
    co: CompanyId,
    id: &str,
) -> FbResult<Vec<u8>> {
    let row = book.read(|tx| {
        services::access(tx, actor, co)?;
        get(tx, co, id)
    })?;
    files.open_read(co, &stored(&row)?)
}

struct Pending<'a> {
    store: &'a Store,
    name: &'a str,
    armed: bool,
}
impl Drop for Pending<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.store.remove_unreferenced(self.name);
        }
    }
}

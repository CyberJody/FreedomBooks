//! Current business details for customer documents; immutable accounting snapshots stay intact.
use crate::{
    CompanyId, UserId,
    error::{FbError, FbResult},
    py, services,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use fb_db::{
    Book, Query,
    rusqlite::{OptionalExtension, params},
};
use image::{ImageFormat, ImageReader, Limits};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Cursor;

pub const LOGO_MAX_BYTES: usize = 128 * 1024;
pub const LOGO_MAX_PIXELS: u32 = 1200;
pub use fb_db::company_document_data::FIELDS;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Details {
    pub street1: String,
    pub street2: String,
    pub city: String,
    pub region: String,
    pub postal_code: String,
    pub country: String,
    pub phone: String,
    pub email: String,
    pub website: String,
    pub tax_id: String,
    pub payment_instructions: String,
}

pub fn get(conn: &(impl Query + ?Sized), company: CompanyId) -> FbResult<Details> {
    let text: Option<String> = conn
        .query_row(
            "SELECT data FROM company_document_details WHERE company_id=?1",
            [company.to_db()],
            |r| r.get(0),
        )
        .optional()?;
    match text {
        Some(text) => {
            if !fb_db::company_document_data::valid(&text) {
                return Err(FbError::validation("Invalid stored business details."));
            }
            fb_db::parse_unique_json_as(&text)
                .map_err(|_| FbError::validation("Invalid stored business details."))
        },
        None => Ok(Details::default()),
    }
}

pub fn logo(
    conn: &(impl Query + ?Sized),
    company: CompanyId,
) -> FbResult<Option<(String, Vec<u8>)>> {
    Ok(conn.query_row("SELECT logo_mime,logo FROM company_document_details WHERE company_id=?1 AND logo IS NOT NULL", [company.to_db()], |r| Ok((r.get(0)?,r.get(1)?))).optional()?)
}

pub fn clean(mut details: Details) -> FbResult<Details> {
    details.payment_instructions = details
        .payment_instructions
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut value = serde_json::to_value(details).expect("details");
    for &(key, label, max) in FIELDS {
        let text = py::py_strip(value[key].as_str().unwrap());
        if text.chars().count() > max {
            return Err(FbError::validation(format!(
                "{label}: use at most {max} characters."
            )));
        }
        for line in text.split('\n') {
            py::clean_free_text(line)?;
        }
        if key != "payment_instructions" && text.chars().any(char::is_control) {
            return Err(FbError::validation(format!(
                "{label}: enter a single line."
            )));
        }
        value[key] = json!(text);
    }
    let email = value["email"].as_str().unwrap();
    if !email.is_empty() && !crate::email_validation::valid_email(email) {
        return Err(FbError::validation("Enter a valid email address."));
    }
    // Website is displayed as escaped text, never fetched or turned into an external link.
    serde_json::from_value(value).map_err(|_| FbError::validation("Invalid business details."))
}

pub fn clean_logo(encoded: &str) -> FbResult<(String, Vec<u8>)> {
    let bad =
        || FbError::validation("Choose a PNG or JPEG logo up to 128 KB and 1200 × 1200 pixels.");
    if encoded.len() > LOGO_MAX_BYTES.div_ceil(3) * 4 {
        return Err(bad());
    }
    let bytes = STANDARD.decode(encoded).map_err(|_| bad())?;
    if bytes.is_empty() || bytes.len() > LOGO_MAX_BYTES {
        return Err(bad());
    }
    let format = image::guess_format(&bytes).map_err(|_| bad())?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
        return Err(bad());
    }
    let mut reader = ImageReader::with_format(Cursor::new(&bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(LOGO_MAX_PIXELS);
    limits.max_image_height = Some(LOGO_MAX_PIXELS);
    limits.max_alloc = Some(16 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(|_| bad())?;
    if image.width() == 0 || image.height() == 0 {
        return Err(bad());
    }
    // Decode and encode pixels only: filenames, EXIF, comments and trailing payloads never persist.
    let mut output = Cursor::new(Vec::new());
    image.write_to(&mut output, format).map_err(|_| bad())?;
    let bytes = output.into_inner();
    if bytes.len() > LOGO_MAX_BYTES {
        return Err(bad());
    }
    Ok((
        if format == ImageFormat::Png {
            "image/png"
        } else {
            "image/jpeg"
        }
        .into(),
        bytes,
    ))
}

pub struct Changes {
    pub details: Details,
    pub logo_upload: Option<String>,
    pub remove_logo: bool,
}

pub fn save(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    name: &str,
    currency: &str,
    changes: Changes,
) -> FbResult<crate::model::Company> {
    let Changes {
        details,
        logo_upload,
        remove_logo: remove,
    } = changes;
    let upload = logo_upload.as_deref();
    let details = clean(details)?;
    if remove && upload.is_some() {
        return Err(FbError::validation(
            "Choose a new logo or remove the current logo, then save.",
        ));
    }
    let uploaded = upload.map(clean_logo).transpose()?;
    book.write(|tx| {
        let user = services::access(tx, actor, company)?;
        let before = get(tx, company)?;
        let old_logo = logo(tx, company)?;
        let new_logo = if remove { None } else { uploaded.or_else(|| old_logo.clone()) };
        let company_row = services::edit_company_tx(tx, actor, company, name, currency)?;
        if before == details && old_logo == new_logo { return Ok(company_row); }
        let data = serde_json::to_string(&details).expect("details");
        // Permanent singleton identity refuses REPLACE, including with recursive triggers off.
        // Update in place; create the row only when this company has no prior details.
        let changed = tx.execute("UPDATE company_document_details SET data=?2,logo_mime=?3,logo=?4 WHERE company_id=?1",
            params![company.to_db(),data,new_logo.as_ref().map(|x| &x.0),new_logo.as_ref().map(|x| &x.1)])?;
        if changed == 0 {
            tx.execute("INSERT INTO company_document_details(company_id,data,logo_mime,logo) VALUES(?1,?2,?3,?4)",
                params![company.to_db(),data,new_logo.as_ref().map(|x| &x.0),new_logo.as_ref().map(|x| &x.1)])?;
        }
        let fingerprint = |logo: &Option<(String, Vec<u8>)>| logo.as_ref().map(|(mime, bytes)| json!({"mime":mime,"bytes":bytes.len(),"sha256":format!("{:x}",Sha256::digest(bytes))}));
        services::audit(tx,&user,company,"company.document_details_updated",format!("company:{company}"),
            json!({"before":before,"after":details,"logo_before":fingerprint(&old_logo),"logo_after":fingerprint(&new_logo)}))?;
        Ok(company_row)
    })
}

pub fn logo_json(
    conn: &(impl Query + ?Sized),
    actor: UserId,
    company: CompanyId,
) -> FbResult<Value> {
    services::access(conn, actor, company)?;
    Ok(match logo(conn, company)? {
        Some((mime, bytes)) => json!({"mime":mime,"base64":STANDARD.encode(bytes)}),
        None => Value::Null,
    })
}

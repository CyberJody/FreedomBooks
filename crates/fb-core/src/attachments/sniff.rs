//! Byte-for-byte structure rules from attachments/sniff.py. These validate containers, not image decoding.
use crate::{FbError, FbResult};
use unicode_normalization::UnicodeNormalization;
pub const TYPES: [&str; 4] = ["application/pdf", "image/png", "image/jpeg", "image/webp"];
pub fn label(t: &str) -> &str {
    match t {
        "application/pdf" => "PDF",
        "image/png" => "PNG",
        "image/jpeg" => "JPEG",
        "image/webp" => "WebP",
        _ => t,
    }
}
pub fn extension(t: &str) -> Option<&'static str> {
    Some(match t {
        "application/pdf" => ".pdf",
        "image/png" => ".png",
        "image/jpeg" => ".jpg",
        "image/webp" => ".webp",
        _ => return None,
    })
}
pub fn sniff(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"%PDF-") {
        Some(TYPES[0])
    } else if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(TYPES[1])
    } else if b.starts_with(b"\xff\xd8\xff") {
        Some(TYPES[2])
    } else if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        Some(TYPES[3])
    } else {
        None
    }
}
pub fn clean_name(raw: &str) -> String {
    let normalized = raw.nfc().collect::<String>().replace('\\', "/");
    let name = normalized.rsplit('/').next().unwrap_or("");
    let name = name
        .split(fb_money::py_isspace)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let name = name
        .chars()
        // Whitespace (including Zl/Zp) was collapsed above. Reuse the frozen Python 15.1 C* table.
        .filter(|c| fb_db::check_numbers::char_allowed(*c))
        .collect::<String>();
    let name = name
        .split(fb_money::py_isspace)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .map(|c| if "<>:\"/\\|?*;%".contains(c) { '_' } else { c })
        .collect::<String>();
    let mut name = name.trim_matches([' ', '.']).to_string();
    if name.chars().count() > 120 {
        let (stem, ext) = splitext(&name);
        let ext = if ext.chars().count() <= 10 { ext } else { "" };
        name = format!(
            "{}{}",
            stem.chars()
                .take(120 - ext.chars().count())
                .collect::<String>()
                .trim_end_matches([' ', '.']),
            ext
        );
    }
    if name.is_empty() {
        "attachment".into()
    } else {
        name
    }
}
fn splitext(s: &str) -> (&str, &str) {
    if let Some(i) = s.rfind('.')
        && s[..i].chars().any(|c| c != '.')
    {
        (&s[..i], &s[i..])
    } else {
        (s, "")
    }
}
pub fn display_name(raw: &str, t: &str) -> FbResult<String> {
    let mut name = clean_name(raw);
    let ext = splitext(&name).1.to_lowercase();
    let accepted = match t {
        "application/pdf" => &[".pdf"][..],
        "image/png" => &[".png"][..],
        "image/jpeg" => &[".jpg", ".jpeg", ".jpe", ".jfif"][..],
        "image/webp" => &[".webp"][..],
        _ => return Err(FbError::refused("type not allowed")),
    };
    if accepted.contains(&ext.as_str()) {
        return Ok(name);
    }
    const KNOWN: &str = ".pdf .png .jpg .jpeg .jpe .jfif .webp .html .htm .xhtml .shtml .svg .svgz .xml .xsl .js .mjs .css .json .exe .dll .com .bat .cmd .scr .msi .msp .ps1 .psm1 .vbs .vbe .wsf .hta .jar .apk .app .dmg .pkg .sh .bash .py .pl .php .rb .lnk .url .zip .rar .7z .tar .gz .iso .doc .docx .docm .xls .xlsx .xlsm .ppt .pptx .rtf .txt .csv .gif .bmp .tif .tiff .heic .heif .ico .eml .msg";
    let suffix = extension(t).ok_or_else(|| FbError::refused("type not allowed"))?;
    if KNOWN.split(' ').any(|e| e == ext) {
        return Err(FbError::refused(format!(
            "The file name ends in {ext} but the file is a {} {}. Rename it to end in {suffix} and attach it again.",
            label(t),
            if t == "application/pdf" {
                "document"
            } else {
                "image"
            }
        )));
    }
    if name.chars().count() + suffix.len() > 120 {
        name = name
            .chars()
            .take(120 - suffix.len())
            .collect::<String>()
            .trim_end_matches([' ', '.'])
            .to_string();
        if name.is_empty() {
            name = "attachment".into();
        }
    }
    Ok(name + suffix)
}
pub fn refusal(head: &[u8], name: &str) -> String {
    let shown = if name.is_empty() {
        "This file".into()
    } else {
        format!("\"{name}\"")
    };
    if head.get(4..8) == Some(b"ftyp")
        && [b"heic", b"heix", b"mif1", b"msf1", b"hevc", b"heif"]
            .iter()
            .any(|x| head.get(8..12) == Some(x.as_slice()))
    {
        format!(
            "{shown} is a HEIC photo (the iPhone default), which cannot be attached yet. Save or share the photo as JPEG and attach that."
        )
    } else {
        format!(
            "Only PDF, PNG, JPEG or WebP files can be attached. {shown} is a different kind of file (checked from its contents, not its name), so it was not attached."
        )
    }
}
pub fn invalid_message(t: &str, name: &str) -> String {
    let shown = if name.is_empty() {
        "This file".into()
    } else {
        format!("\"{name}\"")
    };
    format!(
        "{shown} is not a valid {} file (checked from its contents), so it was not attached.",
        label(t)
    )
}
fn contains(b: &[u8], s: &[u8]) -> bool {
    b.windows(s.len()).any(|w| w == s)
}
fn suspicious(b: &[u8]) -> bool {
    for (i, c) in b.iter().enumerate() {
        if *c == b'<' {
            for tag in [
                b"html".as_slice(),
                b"!doctype",
                b"script",
                b"svg",
                b"iframe",
            ] {
                let start = i + 1;
                let end = start + tag.len();
                if b.get(start..end)
                    .is_some_and(|v| v.eq_ignore_ascii_case(tag))
                    && b.get(end)
                        .is_some_and(|c| !c.is_ascii_alphanumeric() && *c != b'_')
                    && b.get(end..end + 16).is_some_and(|v| {
                        v.iter()
                            .all(|c| matches!(*c, b'\t' | b'\r' | b'\n' | 0x20..=0x7e))
                    })
                {
                    return true;
                }
            }
        }
        if *c == 0x7f
            && b.get(i..i + 4) == Some(b"\x7fELF")
            && b.get(i + 4).is_some_and(|c| [1, 2].contains(c))
            && b.get(i + 5).is_some_and(|c| [1, 2].contains(c))
            && b.get(i + 6) == Some(&1)
        {
            return true;
        }
        if *c == b'M'
            && b.get(i..i + 2) == Some(b"MZ")
            && let Some(n) = le(b, i + 0x3c)
            && (0x40..=0x1000).contains(&n)
            && b.get(i + n..i + n + 4) == Some(b"PE\0\0")
        {
            return true;
        }
    }
    contains(b, b"This program cannot be run in DOS mode")
        || contains(b, b"This program must be run under Win32")
}
fn le(b: &[u8], i: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?) as usize)
}
fn be(b: &[u8], i: usize) -> Option<usize> {
    Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?) as usize)
}
fn pdf_ok(b: &[u8]) -> bool {
    if b.len() < 8
        || !b.starts_with(b"%PDF-")
        || !b[5].is_ascii_digit()
        || b[6] != b'.'
        || !b[7].is_ascii_digit()
    {
        return false;
    }
    let end = b
        .iter()
        .rposition(|c| !b" \t\r\n\x0c\0".contains(c))
        .map_or(0, |i| i + 1);
    if !b[..end].ends_with(b"%%EOF") || b.len() - end + 5 > 1024 {
        return false;
    }
    let Some(i) = b[..b.len().min(1024)]
        .iter()
        .position(|c| matches!(*c, b'\r' | b'\n'))
    else {
        return false;
    };
    !suspicious(&b[i + 1..b.len().min(i + 1 + 4096)])
}
fn png_ok(b: &[u8]) -> bool {
    if !b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return false;
    }
    let mut pos = 8;
    let mut first = true;
    loop {
        let Some(n) = be(b, pos) else {
            return false;
        };
        let Some(end) = pos.checked_add(12 + n) else {
            return false;
        };
        if n > 0x7fffffff || end > b.len() {
            return false;
        }
        let k = &b[pos + 4..pos + 8];
        if !k.iter().all(u8::is_ascii_alphabetic)
            || crc32fast::hash(&b[pos + 4..pos + 8 + n]) as usize
                != be(b, end - 4).unwrap_or(usize::MAX)
        {
            return false;
        }
        if first && (k != b"IHDR" || n != 13) {
            return false;
        }
        first = false;
        pos = end;
        if k == b"IEND" {
            return n == 0 && pos == b.len();
        }
    }
}
fn jpeg_end(b: &[u8], mut p: usize) -> Option<usize> {
    if b.get(p..p + 2) != Some(b"\xff\xd8") {
        return None;
    }
    p += 2;
    let mut scanned = false;
    loop {
        if b.get(p) != Some(&255) {
            return None;
        }
        while b.get(p) == Some(&255) {
            p += 1;
        }
        let marker = *b.get(p)?;
        p += 1;
        if marker == 0xd9 {
            return scanned.then_some(p);
        }
        if matches!(marker, 0 | 0xd8) {
            return None;
        }
        if matches!(marker, 0xd0..=0xd7 | 1) {
            continue;
        }
        let n = u16::from_be_bytes(b.get(p..p + 2)?.try_into().ok()?) as usize;
        if n < 2 || p + n > b.len() {
            return None;
        }
        p += n;
        if marker == 0xda {
            scanned = true;
            loop {
                p += b.get(p..)?.iter().position(|c| *c == 255)?;
                let next = *b.get(p + 1)?;
                match next {
                    0 | 0xd0..=0xd7 => p += 2,
                    255 => p += 1,
                    _ => break,
                }
            }
        }
    }
}
fn jpeg_ok(b: &[u8]) -> bool {
    let mut p = 0;
    for _ in 0..16 {
        let Some(end) = jpeg_end(b, p) else {
            return false;
        };
        p = end;
        let rest = &b[p..];
        if rest.iter().all(|c| [0, 255].contains(c)) {
            return true;
        }
        if !rest.starts_with(b"\xff\xd8") {
            return false;
        }
    }
    false
}
fn webp_ok(b: &[u8]) -> bool {
    if b.len() < 20
        || !b.starts_with(b"RIFF")
        || b.get(8..12) != Some(b"WEBP")
        || le(b, 4) != Some(b.len() - 8)
        || ![b"VP8 ", b"VP8L", b"VP8X"]
            .iter()
            .any(|k| b.get(12..16) == Some(k.as_slice()))
    {
        return false;
    }
    let mut p = 12;
    while p < b.len() {
        if p + 8 > b.len() || !b[p..p + 4].iter().all(|c| (0x20..0x7f).contains(c)) {
            return false;
        }
        let Some(n) = le(b, p + 4) else {
            return false;
        };
        let Some(end) = p.checked_add(8 + n + (n & 1)) else {
            return false;
        };
        p = end;
    }
    p == b.len()
}
pub fn problem(b: &[u8], t: &str) -> Option<&'static str> {
    let good = match t {
        "application/pdf" => pdf_ok(b),
        "image/png" => png_ok(b),
        "image/jpeg" => jpeg_ok(b),
        "image/webp" => webp_ok(b),
        _ => return Some("type not allowed"),
    };
    if !good {
        Some("structure")
    } else if suspicious(&b[..b.len().min(65536)])
        || suspicious(&b[b.len().saturating_sub(65536)..])
    {
        Some("web page or program content")
    } else {
        None
    }
}

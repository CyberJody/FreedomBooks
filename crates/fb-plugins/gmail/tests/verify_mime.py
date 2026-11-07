"""Independent stdlib parser check for the synthetic Rust MIME probe only."""
from email import policy
from email.parser import BytesParser
from pathlib import Path
import sys


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: verify_mime.py SYNTHETIC_PROBE_PATH")
    raw = Path(sys.argv[1]).read_bytes()
    message = BytesParser(policy=policy.default).parsebytes(raw)
    assert not message.defects, message.defects
    assert message.get_content_type() == "multipart/mixed"
    assert str(message["From"]) == "sender@example.test"
    assert message["To"].addresses[0].addr_spec == "customer@example.test"
    assert message["To"].addresses[0].display_name == "Café customer"
    assert str(message["Subject"]) == "Synthetic invoice — café"
    assert str(message["Date"]) == "Tue, 14 Nov 2023 22:13:20 +0000"
    assert message["Message-ID"] is not None
    parts = list(message.iter_parts())
    assert len(parts) == 3
    assert all(not part.defects for part in parts)
    assert parts[0].get_content_type() == "text/plain"
    assert parts[0].get_content() == "PRIVATE_BODY_SENTINEL — café"
    attachments = list(message.iter_attachments())
    assert len(attachments) == 2
    assert attachments[0].get_filename() == "document.pdf"
    assert attachments[0].get_content_type() == "application/pdf"
    assert attachments[0].get_payload(decode=True) == b"%PDF-1.7\nSYNTHETIC_PDF_SENTINEL\x00\xff\n%%EOF"
    assert attachments[1].get_filename() == "notes.txt"
    assert attachments[1].get_payload(decode=True) == b"SYNTHETIC_NOTES"
    assert all(len(line) <= 998 for line in raw.split(b"\r\n"))
    print("Independent email.parser check: PASS; 3 MIME parts; 2 exact-byte attachments; Unicode headers/body and injected Date verified.")


if __name__ == "__main__":
    main()

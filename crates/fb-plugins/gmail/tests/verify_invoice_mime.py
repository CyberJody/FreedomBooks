"""Independent parser for synthetic HTML/CID/PDF MIME only; accepts stdin."""
from email import policy
from email.parser import BytesParser
import sys

message = BytesParser(policy=policy.default).parsebytes(sys.stdin.buffer.read())
assert not message.defects
assert str(message['Reply-To']) == 'reply@example.test'
assert str(message['Message-ID']) == '<fb-synthetic-attempt@freedombooks.local>'
assert message.get_content_type() == 'multipart/mixed'
alternative = list(message.iter_parts())[0]
assert alternative.get_content_type() == 'multipart/alternative'
plain, related = list(alternative.iter_parts())
assert plain.get_content_type() == 'text/plain'
assert plain.get_content() == 'PRIVATE_BODY_SENTINEL — café'
assert related.get_content_type() == 'multipart/related'
html, logo = list(related.iter_parts())
assert html.get_content_type() == 'text/html'
assert 'PRIVATE_HTML_SENTINEL — café' in html.get_content()
assert 'cid:freedombooks-logo' in html.get_content()
assert str(logo['Content-ID']) == '<freedombooks-logo>'
assert logo.get_content_disposition() == 'inline'
assert logo.get_payload(decode=True) == b'SYNTHETIC_LOGO'
files = list(message.iter_attachments())
assert [f.get_filename() for f in files] == ['document.pdf', 'notes.txt']
assert files[0].get_payload(decode=True) == b'%PDF-1.7\nSYNTHETIC_PDF_SENTINEL\x00\xff\n%%EOF'
assert all(not part.defects for part in message.walk())
print('Independent HTML/CID/PDF MIME verification passed.')

"""Frozen Python oracle, deterministic fake vectors; never edits the reference."""
import base64
import hashlib
import io
import json
import os
from pathlib import Path
import sys

sys.path.insert(0, os.environ.get('FB_REFERENCE', '/mnt/Data/Projects/FreedomBooks/wt/reference'))
from backups import crypto

OUT = Path(__file__).resolve().parents[2] / 'fb-core/tests/data/backups-crypto.json'
header = {'kdf': {'name': 'scrypt', 'n': 131072, 'r': 8, 'p': 1,
                  'salt': base64.b64encode(bytes(range(16))).decode()},
          'cipher': {'name': crypto.CIPHER_NAME, 'chunk_size': crypto.CHUNK,
                     'nonce_prefix': base64.b64encode(bytes(range(7))).decode()}}
aad = (json.dumps(header, sort_keys=True, ensure_ascii=False) + '\n').encode()
rows = []
for size in (0, 1, crypto.CHUNK - 1, crypto.CHUNK, crypto.CHUNK + 1, 2 * crypto.CHUNK):
    plain = bytes(i % 251 for i in range(size))
    sealed = io.BytesIO()
    crypto.encrypt_stream(io.BytesIO(plain), sealed, 'synthetic cafe\u0301 passphrase', header, aad)
    ciphertext = sealed.getvalue()
    opened = io.BytesIO()
    crypto.decrypt_stream(io.BytesIO(ciphertext), opened, 'synthetic caf\u00e9 passphrase', header, aad)
    assert opened.getvalue() == plain
    rows.append({'bytes': size, 'ciphertext_bytes': len(ciphertext),
                 'sha256': hashlib.sha256(ciphertext).hexdigest(),
                 'ciphertext': base64.b64encode(ciphertext).decode() if size < 2 else None})
OUT.parent.mkdir(exist_ok=True)
OUT.write_text(json.dumps({'header': header, 'aad': aad.decode(), 'vectors': rows}, indent=2) + '\n')
print('wrote', len(rows), 'Python crypto boundary vectors to', OUT)

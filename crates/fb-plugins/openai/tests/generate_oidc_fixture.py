"""Generate a synthetic RS256 verifier fixture; no private key is saved.

Optional regeneration helper (requires Python cryptography). The checked-in
fixture is self-contained and cargo tests do not require Python/cryptography.
"""
import base64
import json
from pathlib import Path
from cryptography.hazmat.primitives.asymmetric import rsa, padding
from cryptography.hazmat.primitives import hashes

key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
def b64(data): return base64.urlsafe_b64encode(data).rstrip(b'=').decode('ascii')
def integer(n): return b64(n.to_bytes((n.bit_length()+7)//8,'big'))
claims = dict(iss='https://auth.openai.com',aud=['oaiapp_fixture'],sub='synthetic-person',
              email='synthetic@example.invalid',nonce='synthetic-nonce',iat=1700000000,exp=1700003600)
head = b64(json.dumps(dict(alg='RS256',typ='JWT',kid='synthetic-rsa')).encode())
body = b64(json.dumps(claims).encode())
message = f'{head}.{body}'.encode()
signature = key.sign(message,padding.PKCS1v15(),hashes.SHA256())
public = key.public_key().public_numbers()
fixture = dict(token=f'{head}.{body}.{b64(signature)}',jwks=dict(keys=[dict(kty='RSA',kid='synthetic-rsa',use='sig',alg='RS256',n=integer(public.n),e=integer(public.e))]))
Path(__file__).with_name('rsa-identity.json').write_text(json.dumps(fixture,indent=2)+'\n')
print('Generated synthetic RSA identity fixture (no private key saved).')

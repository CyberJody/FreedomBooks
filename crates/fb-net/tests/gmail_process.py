"""Private-pipe Gmail integration against numeric-loopback synthetic providers only.
Usage: python3 gmail_process.py /absolute/path/to/fake-network/fb-net
"""
import base64, email, hashlib, http.server, json, os, subprocess, sys, threading, time, urllib.parse, urllib.request
from email import policy

COMPOSE='https://www.googleapis.com/auth/gmail.compose'
CLIENT='synthetic.apps.googleusercontent.com'
SECRET='SYNTHETIC_DESKTOP_CLIENT_SECRET'
PDF=b'%PDF-1.7\nSynthetic invoice attachment'
LOGO=b'\x89PNG\r\nSYNTHETIC_LOGO'
class Provider(http.server.BaseHTTPRequestHandler):
    calls=[]
    reply=(200, {'id':'draft123','message':{'id':'message123'}})
    challenge=None
    invalid_grant=False
    expected_brands=[]
    def log_message(self,*args): pass
    def do_POST(self):
        body=self.rfile.read(int(self.headers.get('Content-Length','0')))
        type(self).calls.append(self.path)
        status=200
        if self.path=='/token':
            fields=urllib.parse.parse_qs(body.decode())
            assert fields['client_id']==[CLIENT] and fields['client_secret']==[SECRET]
            if fields['grant_type']==['authorization_code']:
                verifier=fields['code_verifier'][0].encode()
                assert base64.urlsafe_b64encode(hashlib.sha256(verifier).digest()).decode().rstrip('=')==self.challenge
                assert fields['code']==['synthetic-code']
            else:
                assert fields['grant_type']==['refresh_token']
                assert fields['refresh_token']==['SYNTHETIC_REFRESH']
            value={'access_token':'SYNTHETIC_ACCESS','refresh_token':'SYNTHETIC_REFRESH','expires_in':3600,'token_type':'Bearer','scope':COMPOSE}
            if self.invalid_grant: status,value=400,{'error':'invalid_grant'}
        elif self.path in ['/drafts','/send']:
            assert self.headers['Authorization']=='Bearer SYNTHETIC_ACCESS'
            data=json.loads(body)
            raw=data['message']['raw'] if self.path=='/drafts' else data['raw']
            mime=email.message_from_bytes(base64.urlsafe_b64decode(raw+'='*(-len(raw)%4)),policy=policy.default)
            assert mime['To']=='buyer@example.test'
            assert str(mime['Subject'])=='Invoice café'
            assert mime['Message-ID']=='<fb-synthetic-attempt@freedombooks.local>'
            assert mime.get_body(preferencelist=('html',)).get_content().strip()=='<p>Synthetic café</p><img src="cid:freedombooks-logo">'
            attachments=[part.get_payload(decode=True) for part in mime.walk() if not part.is_multipart()]
            assert PDF in attachments and LOGO in attachments
            cids={part['Content-ID']:part.get_payload(decode=True) for part in mime.walk() if part['Content-ID']}
            for key in self.expected_brands:
                assert cids.get('<freedombooks-pay-'+key+'>',b'').startswith(b'\x89PNG'),cids.keys()
            status,value=self.reply
        elif self.path=='/revoke': value={}
        else: raise AssertionError(self.path)
        encoded=json.dumps(value).encode()
        self.send_response(status); self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(encoded)));self.end_headers();self.wfile.write(encoded)

server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Provider)
thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
env={'FB_NET_FAKE_GMAIL':f'http://127.0.0.1:{server.server_port}','PATH':os.environ.get('PATH','')}
binary=sys.argv[1]
def request(value):
    result=subprocess.run([binary],input=json.dumps(value)+'\n',capture_output=True,text=True,env=env,timeout=15)
    assert result.returncode==0 and result.stderr=='', (result.returncode,result.stderr)
    return json.loads(result.stdout)
def deliver(record,mode='DRAFT',**changes):
    data={'invoice':'00000000-0000-7000-8000-000000000001','mode':mode,'to':'buyer@example.test','subject':'Invoice café','text':'Synthetic plain text',
          'html':'<p>Synthetic café</p><img src="cid:freedombooks-logo">','pdf':base64.b64encode(PDF).decode(),
          'logo':{'mime':'image/png','data':base64.b64encode(LOGO).decode()},'message_id':'fb-synthetic-attempt@freedombooks.local'}
    data.update(changes)
    return request({'op':'gmail_deliver','record':record,'gmail':{'delivery':data}})
try:
    process=subprocess.Popen([binary],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,env=env)
    process.stdin.write(json.dumps({'op':'gmail_begin','gmail':{'client_id':CLIENT,'client_secret':SECRET,'from_address':'sender@example.test'}})+'\n');process.stdin.close()
    initial=json.loads(process.stdout.readline());assert initial['stage']=='authorize'
    auth=urllib.parse.parse_qs(urllib.parse.urlparse(initial['authorization_url']).query)
    assert auth['scope']==[COMPOSE] and auth['code_challenge_method']==['S256']
    assert SECRET not in initial['authorization_url']
    Provider.challenge=auth['code_challenge'][0]
    redirect=auth['redirect_uri'][0];assert urllib.parse.urlparse(redirect).hostname=='127.0.0.1'
    with urllib.request.urlopen(redirect+'?'+urllib.parse.urlencode({'state':auth['state'][0],'code':'synthetic-code'}),timeout=10) as response:
        assert response.status==200
    done=json.loads(process.stdout.readline());assert done['ok'] and done['stage']=='done'
    process.wait(timeout=10);assert process.returncode==0 and process.stderr.read()==''
    record=done['record'];assert json.loads(record)['client_secret']==SECRET
    assert json.loads(json.loads(record)['tokens'])['scope']==COMPOSE
    assert Provider.calls==['/token']
    draft=deliver(record);assert draft['outcome']=='accepted' and draft['draft_id']=='draft123' and draft['message_id']=='message123'
    Provider.reply=(200,{'id':'sent123'})
    sent=deliver(record,'SEND');assert sent['outcome']=='accepted' and sent['draft_id'] is None and sent['message_id']=='sent123'
    assert Provider.calls==['/token','/drafts','/send']
    Provider.reply=(200,{'id':'brand-draft','message':{'id':'brand-message'}})
    Provider.expected_brands=['zelle','stripe','visa','master','amex','discover','apple','bank']
    branded=deliver(record,brand_images=Provider.expected_brands);assert branded['outcome']=='accepted',branded
    Provider.expected_brands=[]
    before=len(Provider.calls)
    for selectors in [['unknown'],['zelle','zelle'],['visa']*9]:
        refused=deliver(record,brand_images=selectors);assert refused.get('outcome')=='refused' or refused.get('ok') is False
        assert len(Provider.calls)==before
    bad=deliver(record,to='bad\r\nBcc: injected@example.test');assert bad['outcome']=='refused' and len(Provider.calls)==before
    Provider.reply=(503,{})
    unknown=deliver(record);assert unknown['outcome']=='unknown' and len(Provider.calls)==before+1
    Provider.reply=(200,{'id':'incomplete-draft'})
    unknown=deliver(record);assert unknown['outcome']=='unknown' and len(Provider.calls)==before+2
    Provider.reply=(401,{})
    refused=deliver(record);assert refused['outcome']=='refused' and refused['code']=='needs_signin' and refused['record'] is None
    envelope=json.loads(record);tokens=json.loads(envelope['tokens']);tokens['expires_at']=0;envelope['tokens']=json.dumps(tokens)
    Provider.reply=(200,{'id':'draft-refresh','message':{'id':'message-refresh'}})
    refreshed=deliver(json.dumps(envelope));assert refreshed['outcome']=='accepted' and Provider.calls[-2:]==['/token','/drafts']
    assert json.loads(json.loads(refreshed['record'])['tokens'])['expires_at']>time.time()
    Provider.invalid_grant=True;before=len(Provider.calls)
    expired=deliver(json.dumps(envelope));assert expired['outcome']=='refused' and expired['record'] is None and len(Provider.calls)==before+1
    revoked=request({'op':'gmail_disconnect','record':record});assert revoked=={'ok':True,'revoked':True}
    invalid=request({'op':'gmail_deliver','record':record,'gmail':{'delivery':{'unexpected':True}}});assert invalid['ok'] is False
    print('PASS: OAuth PKCE, compose scope, draft/send MIME, refresh/revocation, header refusal, uncertain/no-retry, expired token and strict pipe protocol')
finally:
    server.shutdown();server.server_close()

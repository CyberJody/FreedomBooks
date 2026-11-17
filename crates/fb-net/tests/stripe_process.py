"""Exercise the real private-pipe helper with a local fake Stripe; no live requests."""
import copy,http.server,json,os,subprocess,sys,threading,urllib.parse
KEY='rk_test_SYNTHETIC_NOT_A_REAL_KEY'
LINK={'id':'00000000-0000-7000-8000-000000000001','company':'00000000-0000-7000-8000-000000000002','invoice':'00000000-0000-7000-8000-000000000003','revision':'00000000-0000-7000-8000-000000000004','amount':12345,'currency':'USD','description':'Synthetic Company Invoice 1042'}
META={f'fb_{k}':LINK[v] for k,v in [('request','id'),('company','company'),('invoice','invoice'),('revision','revision')]}
REMOTE={'id':'plink_Synthetic','livemode':False,'currency':'usd','active':True,'url':'https://buy.stripe.com/test_Synthetic','metadata':META,'restrictions':{'completed_sessions':{'limit':1}},'line_items':{'data':[{'quantity':1,'amount_total':12345,'currency':'usd'}],'has_more':False},'automatic_tax':{'enabled':False},'allow_promotion_codes':False}
SESSION={'id':'cs_test_Synthetic','livemode':False,'metadata':META,'mode':'payment','payment_link':'plink_Synthetic','amount_total':12345,'currency':'usd','created':1794168000,'status':'complete','payment_status':'paid','payment_intent':{'id':'pi_Synthetic','livemode':False,'metadata':META,'amount':12345,'amount_received':12345,'currency':'usd','status':'succeeded','latest_charge':{'id':'ch_Synthetic','livemode':False,'payment_intent':'pi_Synthetic','amount':12345,'amount_captured':12345,'amount_refunded':0,'disputed':False,'currency':'usd','paid':True,'captured':True}}}
class Provider(http.server.BaseHTTPRequestHandler):
    calls=[];post_status=200;account='acct_Synthetic';remote=REMOTE;rows=[SESSION];more=False;bad=False
    def log_message(self,*args):pass
    def do_GET(self):self.serve(False)
    def do_POST(self):self.serve(True)
    def serve(self,post):
        assert self.headers['Authorization']=='Bearer '+KEY
        assert self.headers['Stripe-Version']=='2026-10-28.endive'
        path=urllib.parse.urlsplit(self.path)
        fields=urllib.parse.parse_qs(self.rfile.read(int(self.headers.get('Content-Length',0))).decode()) if post else urllib.parse.parse_qs(path.query)
        self.calls.append((post,path.path,fields));status=200
        if path.path=='/v1/account':value={'id':self.account}
        elif path.path=='/v1/payment_links' and post:
            assert self.headers['Idempotency-Key']=='fb-invoice-'+LINK['id']
            assert fields['line_items[0][price_data][unit_amount]']==['12345']
            assert fields['restrictions[completed_sessions][limit]']==['1']
            assert fields['automatic_tax[enabled]']==['false']
            assert 'payment_method_types' not in fields
            for k,v in META.items():assert fields[f'metadata[{k}]']==[v] and fields[f'payment_intent_data[metadata][{k}]']==[v]
            status=self.post_status;value=self.remote
        elif path.path=='/v1/payment_links':
            value={'data':[self.remote],'has_more':self.more}
        elif path.path=='/v1/payment_links/plink_Synthetic':
            value=copy.deepcopy(self.remote)
            if post:assert fields['active']==['false'];value['active']=False
        elif path.path=='/v1/checkout/sessions':
            assert fields['payment_link']==['plink_Synthetic']
            if fields.get('status')==['open']:value={'data':[v for v in self.rows if v['status']=='open'],'has_more':False}
            else:
                assert fields['expand[]']==['data.payment_intent.latest_charge']
                value={'data':self.rows,'has_more':self.more}
        elif path.path.startswith('/v1/checkout/sessions/') and path.path.endswith('/expire') and post:
            sid=path.path.split('/')[-2];value=next(v for v in self.rows if v['id']==sid);assert value['status']=='open';value['status']='expired'
        else:raise AssertionError(path.path)
        body=b'bad json' if self.bad else json.dumps(value).encode()
        self.send_response(status);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Provider);threading.Thread(target=server.serve_forever,daemon=True).start()
env={'PATH':os.environ.get('PATH',''),'FB_NET_FAKE_STRIPE':f'http://127.0.0.1:{server.server_port}'}
binary=sys.argv[1]
def call(op,link=None,cursor='',known=None,**credential):
    record={'version':1,'key':KEY,'mode':'TEST','account':'acct_Synthetic'};record.update(credential)
    result=subprocess.run([binary],input=json.dumps({'op':'stripe_'+op,'record':json.dumps(record),'stripe':{'link':link,'cursor':cursor,'known_links':known or []}})+'\n',capture_output=True,text=True,env=env,timeout=10)
    assert result.returncode==0 and not result.stderr
    assert KEY not in result.stdout
    return json.loads(result.stdout)
def refresh():return call('refresh',{**LINK,'provider_id':'plink_Synthetic'})
try:
    assert call('probe',account='')=={'ok':True,'account':'acct_Synthetic','mode':'TEST'}
    assert call('conflicts',LINK)['conflict'] is True
    known=[{'request':LINK['id'],'provider':'plink_Synthetic'}]
    assert call('conflicts',LINK,known=known)['conflict'] is True
    assert call('conflicts',LINK,known=known)['blocking']==known
    Provider.remote=copy.deepcopy(REMOTE);Provider.remote['active']=False
    assert call('conflicts',LINK,known=known)['conflict'] is False
    assert call('conflicts',LINK,known=[{'request':LINK['id'],'provider':'plink_Different'}])['conflict'] is True
    Provider.remote=REMOTE
    Provider.more=True
    assert call('conflicts',LINK)['next']=='plink_Synthetic'
    Provider.more=False
    assert call('create',LINK)['outcome']=='created'
    assert refresh()['observations'][0]['status']=='READY'
    Provider.rows=copy.deepcopy([SESSION]);Provider.rows[0]['payment_intent']['status']='processing';Provider.rows[0]['payment_status']='unpaid'
    assert refresh()['observations'][0]['status']=='PENDING'
    Provider.rows=copy.deepcopy([SESSION]);Provider.rows[0]['payment_intent']['latest_charge']['amount_refunded']=100
    assert refresh()['observations'][0]['status']=='REFUNDED'
    Provider.rows[0]['payment_intent']['latest_charge']['disputed']=True
    assert refresh()['observations'][0]['status']=='DISPUTED'
    Provider.rows=copy.deepcopy([SESSION]);Provider.rows[0]['amount_total']=12000
    assert refresh()['observations'][0]['status']=='MISMATCH'
    Provider.rows=copy.deepcopy([SESSION]);Provider.rows[0]['payment_intent']['livemode']=True
    assert not refresh()['ok']
    Provider.rows=copy.deepcopy([SESSION]);Provider.rows[0]['metadata']={**META,'fb_company':'wrong'}
    assert not refresh()['ok']
    Provider.rows=[SESSION];Provider.more=True
    assert refresh()['next']=='cs_test_Synthetic'
    assert not call('refresh',{**LINK,'provider_id':'plink_Synthetic'},'cs_test_Synthetic')['ok']
    Provider.more=False
    assert call('recover',LINK)['found']['id']=='plink_Synthetic'
    Provider.rows=copy.deepcopy([SESSION]);Provider.rows[0]['status']='open'
    closed=call('deactivate',{**LINK,'provider_id':'plink_Synthetic'})
    assert closed['link']['active'] is False and closed['closed'] is True and Provider.rows[0]['status']=='expired'
    Provider.rows=[SESSION]
    before=len(Provider.calls);assert not call('create',LINK,mode='LIVE')['ok'];assert len(Provider.calls)==before
    Provider.account='acct_Wrong';assert call('create',LINK)['outcome']=='refused';assert len(Provider.calls)==before+1
    Provider.account='acct_Synthetic';Provider.post_status=503;before=len(Provider.calls)
    assert call('create',LINK)['outcome']=='unknown';assert len(Provider.calls)==before+2
    Provider.post_status=403;assert call('create',LINK)['outcome']=='refused'
    Provider.post_status=200;Provider.remote=copy.deepcopy(REMOTE);Provider.remote['line_items']['data'][0]['amount_total']=1
    assert call('create',LINK)['outcome']=='unknown'
    Provider.remote=copy.deepcopy(REMOTE);Provider.remote['url']='http://127.0.0.1/leak'
    assert call('create',LINK)['outcome']=='unknown'
    print('PASS: Stripe fixed hosts/key isolation, single-request link creation/idempotency, exact cents, pagination, delayed success, refunds/disputes, mismatches, scoped metadata, deactivation and uncertain/no retry')
finally:server.shutdown();server.server_close()

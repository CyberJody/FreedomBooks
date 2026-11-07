"""Boundary scenarios translated from payables tests and FX6 purchase-line tests."""
from gen_payables_scenarios import setup,payload,payment,step,post,void,save,key,deepcopy
s=setup()
# Saved snapshots never silently refresh or reprice when the directory changes.
s += [{'op':'save_account','company':'co','ref':'income','code':'400','name':'Income provenance','kind':'INCOME'}, {'op':'l2_save','company':'co','ref':'item','model':'item','values':{'code':'Part','description':'Original item','kind':'NONSTOCK','sale_rate':'90','purchase_cost':'3','income_account':'income'}}]
p=payload('Snapshot');p['lines'][0].update(item_id='item',quantity='2',rate='3.25');s += [step('save',ref='b',values=p),{'op':'l2_save','company':'co','record':'item','model':'item','values':{'description':'Changed item','purchase_cost':'99','income_account':None}},{'op':'l2_save','company':'co','record':'vendor','model':'vendor','values':{'display_name':'Changed vendor','billing_address':'New address'}},{'op':'save_account','company':'co','account':'income','code':'400','name':'Income provenance','kind':'INCOME','active':False}]
p.update(bill_id='b',expected_revision=1);s += [step('save',ref='b',values=p),post('b',2),void('b',3)]
# Active/retained historical selections are distinct from eligibility to post.
p=payload('Archive');p['lines'][0]['item_id']='item';s += [step('save',ref='archived',values=p),{'op':'l2_active','company':'co','model':'item','record':'item','active':False}]
p.update(bill_id='archived',expected_revision=1);s += [step('save',ref='archived',values=deepcopy(p),expect_error=True)];p['lines'][0]['keep_archived']=True;s += [step('save',ref='archived',values=p),post('archived',2,expect_error=True)]
s += [{'op':'l2_active','company':'co','model':'item','record':'item','active':True},post('archived',2)]
# Distinct original AP accounts in one payment, independent of today's setup.
p=payload('AP1','10');p['lines'][0]['quantity']='1';s += [step('save',ref='a1',values=p),post('a1'),{'op':'l2_setup','company':'co','expected_revision':1,'values':{'payables':'ap2','banks':['bank']}}]
p=payload('AP2','20');p['lines'][0]['quantity']='1';s += [step('save',ref='a2',values=p),post('a2'),{'op':'l2_setup','company':'co','expected_revision':2,'values':{'payables':'ap','banks':['bank']}}]
p=payment('a1','12');p['allocations']=[{'bill_id':'a2','amount':'7'},{'bill_id':'a1','amount':'5'}];s += [step('review_payment',review='payr',values=p),step('pay',ref='pay',values=p,review='payr')]
# Duplicate numeric check references are refused, reversed references are reusable.
p2=payment('a2','1');s += [step('pay',ref='bad',values=deepcopy(p2),expect_error=True),step('reverse',payment='pay',expected_revision=1,date='2026-10-24',reason='Correction'),step('pay',ref='pay2',values=p2)]
# Exact old command retries survive the reversed original.
s += [step('pay',ref='pay',values=p,review='payr')]
# Strict amount/input contracts.
for amount in ['0','-1','NaN','Infinity','1.001','100000000000000.00',True,1.2]:
 p=payment('a1',amount,'free text');s += [step('pay',ref='bad',values=p,expect_error=True)]
for change in [dict(date='2026-10-22'),dict(amount='10.01'),dict(allocations=[]),dict(allocations=[dict(bill_id='a1',amount='5')]*2),dict(bank_id='ap'),dict(allocations=[dict(bill_id='a1',amount='1')]*51)]:
 p=payment('a1','10','free text');p.update(change);s += [step('pay',ref='bad',values=p,expect_error=True)]
# Unicode text is bounded by code points, and source journals only contain a prefix.
p=payload('🚀'*120,'100');p['lines'][0]['quantity']='1';p['memo']='漢é🚀x'*500;s += [step('save',ref='long',values=p),post('long')]
p=payment('long','1','🚀'*120);p['memo']='漢é🚀x'*500;s += [step('pay',ref='longpay',values=p),step('reverse',payment='longpay',expected_revision=1,date='2026-10-24',reason='漢é🚀xy'*100)]
s += [void('long')]
# Date.fromisoformat variants, and invalid calendar strings.
for i,(dt,due) in enumerate([('20261023','20261123'),('2026-W43-5','2026-W48-1'),('2026W435','2026W481'),('2026-W43','2026-W48'),('2026W43','2026W48')]):
 p=payload();p.update(date=dt,due_date=due);s += [step('save',ref='date'+str(i),values=p)]
for dt in ['2026-1-2','2026-02-30','0000-01-01','2026W-43','NaN','٢٠٢٦-10-23']:
 p=payload();p['date']=dt;s += [step('save',ref='bad',values=p,expect_error=True)]
save('boundaries',s)
s=setup();allocations=[]
for i in range(50):
 p=payload('', '1');p['lines'][0]['quantity']='1';name=f'b{i}';s += [step('save',ref=name,values=p),post(name)];allocations.append(dict(bill_id=name,amount='1'))
p=payment('b0','50');p['allocations']=list(reversed(allocations));s += [step('pay',ref='pay50',values=p),step('reverse',payment='pay50',expected_revision=1,date='2026-10-24',reason='Correction')]
save('fifty-allocations',s)
print('wrote payables boundary and fifty-allocation scenarios')

s=setup();s += [{'op':'l2_save','company':'co','record':'vendor','model':'vendor','values':{'default_expense_account':'expense','default_terms':'30'}}]
fields={'document_date':'2026-10-23','document_number':'Receipt 42','lines':[{'description':'Extracted purchase','quantity':'1.005','rate':'1.00','amount':'1.01'},{'description':'Supplier sales tax','quantity':'1','rate':'0.08','amount':'0.08'}]}
k=key();s += [step('receipt',vendor='vendor',ref='receipt',request_key=k,fields=fields),step('receipt',vendor='vendor',ref='receipt',request_key=k,fields=fields),post('receipt'),void('receipt')]
fields['document_number']='Receipt explicit due';fields['due_date']='2026-12-15';s += [step('receipt',vendor='vendor',ref='receipt2',request_key=key(),fields=fields)]
save('receipt-core',s)

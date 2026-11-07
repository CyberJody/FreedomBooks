"""Deterministic backup workflows and 500 local unlock attempts (seed 41030)."""
from pathlib import Path
import json
import random
R=Path(__file__).resolve().parents[3]
scenario={'users':[{'username':'owner','is_staff':True}], 'as_of':['2026-12-31'], 'steps':[
 {'op':'create_company','ref':'c','name':'Synthetic backup company'},
 {'op':'save_account','company':'c','ref':'cash','code':'1000','name':'Cash','kind':'ASSET'},
 {'op':'save_account','company':'c','ref':'capital','code':'3000','name':'Capital','kind':'EQUITY'},
 {'op':'save_account','company':'c','ref':'zero','code':'9000','name':'Zero account','kind':'EXPENSE'},
 {'op':'save_draft','company':'c','ref':'j','date':'2026-10-01','memo':'Synthetic contribution','lines':[{'account':'cash','debit':'123.45','credit':'0'},{'account':'capital','debit':'0','credit':'123.45'}]},
 {'op':'post_journal','company':'c','journal':'j'},
 {'op':'save_draft','company':'c','ref':'draft','date':'2026-10-02','memo':'Synthetic draft','lines':[{'account':'cash','debit':'10','credit':'0'},{'account':'capital','debit':'0','credit':'10'}]},
 {'op':'backups_snapshot','ref':'first'},
 {'op':'post_journal','company':'c','journal':'draft'},
 {'op':'backups_verify','backup':'first'},
 {'op':'backups_restore','backup':'first'},
 {'op':'backups_refuse_existing','backup':'first','expect_error':True},
 {'op':'backups_snapshot','ref':'second'},
 {'op':'backups_restore','backup':'second'},
]}
for step in scenario['steps']:
 step.setdefault('expect_error', False)
(R/'crates/fb-harness/scenarios/backups-lifecycle.json').write_text(json.dumps(scenario,indent=2)+'\n')
rng=random.Random(41030);now=0
random_scenario={'users':[{'username':'owner','is_staff':True}],'steps':[]}
for _ in range(500):
 now+=rng.choice([0,1,4,90,900,901])
 random_scenario['steps'].append({'op':'backups_attempt','correct':rng.randrange(5)==0,'now':now,'expect_error':False})
(R/'crates/fb-harness/scenarios/backups-random-500.json').write_text(json.dumps(random_scenario,indent=2)+'\n')

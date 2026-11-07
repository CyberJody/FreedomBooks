import json, random, subprocess, os
from pathlib import Path

root = Path(__file__).resolve().parents[3]
alphabet = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 -./&',()#éèêëáàâäåñçöüøæÉÈÊËÁÀÂÄÅÑÇÖÜØÆ"
rng = random.Random(20261030)
texts = list(alphabet) + ['İ', 'ı', 'ß', 'ẞ', 'ſ', 'K', 'ǰ']
texts += [''.join(rng.choices(alphabet, k=rng.randrange(1, 25))) for _ in range(10000)]
sql = "BEGIN READ ONLY; SELECT json_agg(json_build_array(x,lower(x),upper(x))) FROM json_array_elements_text($fb$" + json.dumps(texts) + "$fb$::json) AS t(x); ROLLBACK;"
sql_path = root/'target/portable-casing-oracle.sql'
sql_path.parent.mkdir(exist_ok=True)
sql_path.write_text(sql)
result = subprocess.run(['psql','-X','-At','-d',os.environ.get('FB_CASING_ORACLE_DB','fbv2_l2a_fxref'),'-f',str(sql_path)],check=True,text=True,capture_output=True)
rows = json.loads(next(line for line in result.stdout.splitlines() if line.startswith('[')))
(root/'crates/fb-db/tests/pg_case_vectors.json').write_text(json.dumps(rows,ensure_ascii=False,separators=(',',':'))+'\n')
print(f'Captured {len(rows)} live PostgreSQL LOWER/UPPER vectors (10000 seeded realistic strings, alphabet, Unicode edge cases)')

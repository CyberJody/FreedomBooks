"""Keep the reviewer's destructive synthetic-book probes as refusing regressions."""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REVIEW = ROOT.parents[1] / 'staging/r-print'
target = ROOT / 'electron/scripts'
text = (REVIEW / 'save-path-test.cjs').read_text()
text = text.replace("const out = __dirname;", "const out = process.env.FB_SMOKE_SHOTS;\nfs.mkdirSync(path.join(out,'tmp'),{recursive:true});")
text = text.replace("const source = '/mnt/Data/Projects/FreedomBooks/wt-v2/r-print/electron/main/printing.cjs';", "const source = path.resolve(__dirname,'../main/printing.cjs');\nconst localRequire = require('node:module').createRequire(source);")
text = text.replace("require:name=>name==='electron'?electron:require(name)", "require:name=>name==='electron'?electron:require('node:module').createRequire(path.resolve(__dirname,'../main/printing.cjs'))(name)")
text = text.replace("result.push({kind, error, sentinelPreserved:", "assert.ok(error,kind+' must refuse');\n    assert.equal(fs.readFileSync(book,'utf8'),'SYNTHETIC BOOK SENTINEL',kind+' book unchanged');\n    result.push({kind, error, sentinelPreserved:")
(target / 'save-path-regression.cjs').write_text(text)
text = (REVIEW / 'save-live.cjs').read_text()
text = text.replace("const root='/mnt/Data/Projects/FreedomBooks/wt-v2/r-print/electron';", "const root=path.resolve(__dirname,'..');\nconst out=process.env.FB_SMOKE_SHOTS;\nfs.mkdirSync(path.join(out,'tmp'),{recursive:true});\nconst hash=file=>require('node:crypto').createHash('sha256').update(fs.readFileSync(file)).digest('hex');")
text = text.replace("path.join(__dirname,'tmp/live-save-')", "path.join(out,'tmp/live-save-')")
text = text.replace("const before=fs.readFileSync(book).subarray(0,5).toString();assert.notEqual(before,'%PDF-');", "const before=hash(book);")
text = text.replace("assert.equal(overwritten,true,'review reproduction: PDF overwrite of synthetic open book');", "assert.equal(overwritten,false,'PDF must never overwrite the synthetic open book');\n      assert.equal(hash(book),before,'open book SHA-256 must remain unchanged');\n      const errorText=await page.locator('body').innerText();\n      assert.match(errorText,/Choose a different file name\\. That location is used by FreedomBooks\\./);\n      evidence.push({variant,bookHashUnchanged:true});")
text = text.replace("path.join(__dirname,'save-live-results.json')", "path.join(out,'save-live-results.json')")
(target / 'save-live-regression.cjs').write_text(text)
print('Copied reviewer save-path and live-book regressions; now require refusal and unchanged book hash')

"""DESKTOP-JODY1: both Electron smokes reach Report a problem through the footer link (smoke-report.cjs)."""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OLD = '''    await page.goto('fb://app/help/report/');
    assert.equal(await heading(page), 'Report a problem');
    await shot(page, 'd2-16-report-problem');
'''
NEW = '''    await require('./smoke-report.cjs')({ app, page, shots, shot, pass, heading });
    await shot(page, 'd2-16-report-problem');
'''
for name in ['smoke.cjs', 'smoke-secfix1.cjs']:
    p = ROOT / 'electron/scripts' / name
    t = p.read_text()
    assert t.count(OLD) == 1, name
    p.write_text(t.replace(OLD, NEW))
p = ROOT / 'electron/scripts/smoke-report.cjs'
t = p.read_text()
old = "  await page.waitForFunction(() => true);\n"
assert t.count(old) == 1
p.write_text(t.replace(old, ''))

p = ROOT / 'docs/REPORTING-PROBLEMS.md'
t = p.read_text()
old = '''FreedomBooks does not show its version inside the app yet. Instead:

- Look at the file you downloaded. The version is usually part of its name.
- Or look at the [Releases page](https://github.com/CyberJody/FreedomBooks/releases) and tell us which release you installed, and roughly when.
- If you built FreedomBooks yourself, give us the commit: run `git rev-parse --short HEAD` in the project folder.
'''
new = '''In FreedomBooks, click **Report a problem** at the bottom of any screen. That page shows your FreedomBooks version and your operating system, ready to copy into your report. Its **Open the GitHub issue page** link opens the new-issue page in your web browser.

If you built FreedomBooks yourself, also give us the commit: run `git rev-parse --short HEAD` in the project folder.
'''
assert t.count(old) == 1
p.write_text(t.replace(old, new))
print('ok')

"""Honor filesystem resolution of '..' after a parent symlink, before lexical normalization."""
from pathlib import Path
import sys
ROOT=Path(__file__).resolve().parents[1]
if sys.argv[1]=='test':
    p=ROOT/'electron/tests/save-file.test.cjs'
    p.write_text(p.read_text()+r'''

test('parent symlink followed by dot-dot resolves before protected-location checks',t=>{
  const {dataDir,output,saver}=fixture(t);
  const key=path.join(dataDir,'book-key.sealed');fs.writeFileSync(key,'SYNTHETIC KEY');
  const alias=path.join(output,'alias');fs.symlinkSync(path.join(dataDir,'books'),alias);
  rejected(()=>saver.write(`${alias}/../book-key.sealed`,'BAD'));
  assert.equal(fs.readFileSync(key,'utf8'),'SYNTHETIC KEY');
  assert.equal(fs.existsSync(path.join(output,'book-key.sealed')),false,'never save to a different lexical location');
});
''')
elif sys.argv[1]=='fix':
    p=ROOT/'electron/main/save-file.cjs';text=p.read_text()
    for old,new in [
        ('const entry = stat(chosen);','const entry = stat(file);'),
        ('const parent = fs.realpathSync(path.dirname(chosen));','const parent = fs.realpathSync(path.dirname(file));'),
        ('entry ? fs.realpathSync(chosen) :','entry ? fs.realpathSync(file) :'),
        ('real(path.resolve(root))','real(path.isAbsolute(root) ? root : path.resolve(root))'),
        ('real(path.resolve(sensitive))','real(path.isAbsolute(sensitive) ? sensitive : path.resolve(sensitive))'),
    ]:
        assert text.count(old)==1,(old,text.count(old));text=text.replace(old,new)
    p.write_text(text)
elif sys.argv[1]=='native':
    p=ROOT/'electron/main/save-file.cjs';text=p.read_text()
    assert 'fs.realpathSync(' in text
    p.write_text(text.replace('fs.realpathSync(', 'fs.realpathSync.native('))
elif sys.argv[1]=='root-test':
    p=ROOT/'electron/tests/save-file.test.cjs';text=p.read_text()
    old="  assert.equal(fs.existsSync(path.join(output,'book-key.sealed')),false,'never save to a different lexical location');"
    new=old+r'''
  const previous=process.env.XDG_DATA_HOME;
  process.env.XDG_DATA_HOME=`${alias}/..`;
  t.after(()=>{if(previous===undefined)delete process.env.XDG_DATA_HOME;else process.env.XDG_DATA_HOME=previous;});
  const xdg=path.join(dataDir,'FreedomBooks');fs.mkdirSync(xdg);
  rejected(()=>create().write(path.join(xdg,'new.csv'),'BAD'));
  assert.equal(fs.existsSync(path.join(xdg,'new.csv')),false,'XDG root dot-dot preserves filesystem semantics too');'''
    assert text.count(old)==1;p.write_text(text.replace(old,new))
elif sys.argv[1]=='root-fix':
    p=ROOT/'electron/main/save-file.cjs';text=p.read_text()
    old="    const roots = [path.join(process.env.XDG_DATA_HOME || path.join(os.homedir(), '.local/share'), 'FreedomBooks'), dataDir].filter(Boolean);"
    new="    const xdgHome = process.env.XDG_DATA_HOME || path.join(os.homedir(), '.local/share');\n    const roots = [xdgHome + path.sep + 'FreedomBooks', dataDir].filter(Boolean);"
    assert text.count(old)==1;p.write_text(text.replace(old,new))
else:
    raise SystemExit('test | fix | native | root-test | root-fix')

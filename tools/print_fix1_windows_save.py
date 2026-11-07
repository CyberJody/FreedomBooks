"""Keep Windows output working without opening directory handles as regular files."""
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
p=ROOT/'electron/main/save-file.cjs';text=p.read_text()
old="    const dir = fs.openSync(selected.parent, fs.constants.O_RDONLY | (fs.constants.O_DIRECTORY || 0) | (fs.constants.O_NOFOLLOW || 0));"
new="    const dir = process.platform === 'win32' ? null : fs.openSync(selected.parent, fs.constants.O_RDONLY | (fs.constants.O_DIRECTORY || 0) | (fs.constants.O_NOFOLLOW || 0));"
assert text.count(old)==1;text=text.replace(old,new)
text=text.replace('      const opened = fs.fstatSync(dir);','      const opened = dir === null ? fs.statSync(selected.parent) : fs.fstatSync(dir);')
old='''      if (fd !== undefined) fs.closeSync(fd);
      if (created) fs.unlinkSync(temporary);
      fs.closeSync(dir);'''
new='''      try {
        if (fd !== undefined) fs.closeSync(fd);
        if (created) fs.unlinkSync(temporary);
      } finally {
        if (dir !== null) fs.closeSync(dir);
      }'''
assert text.count(old)==1;text=text.replace(old,new);p.write_text(text)
p=ROOT/'electron/tests/save-file.test.cjs';text=p.read_text()
text=text.replace('function instrument(overrides) {','function instrument(overrides, platform = process.platform) {')
text=text.replace('const sandbox={process,Buffer,module:', 'const sandbox={process:{platform,env:process.env},Buffer,module:')
text+=r'''

test('Windows fallback replaces regular output without opening or fsyncing a directory',t=>{
  const {book,output,dataDir}=fixture(t);
  const selected=path.join(output,'invoice.pdf');
  const module=instrument({
    openSync:(file,flags,...rest)=>{assert.equal(flags,'wx');return fs.openSync(file,flags,...rest);},
    fsyncSync:fd=>{assert.equal(fs.fstatSync(fd).isFile(),true);fs.fsyncSync(fd);},
  },'win32');
  for(const content of ['NEW PDF','REPLACEMENT PDF']) {
    module.create({dataDir,bookPath:()=>book}).write(selected,content);
    assert.equal(fs.readFileSync(selected,'utf8'),content);
  }
});
'''
p.write_text(text)
print('Windows uses canonical-path identity checks; Linux retains fd-pinned writes')

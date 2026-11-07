"""Extend the unchanged review flows with real PDF replacement and CSV refusal checks."""
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def edit(name,old,new):
    p=ROOT/name;text=p.read_text();assert old in text,(name,old);p.write_text(text.replace(old,new))

edit('electron/scripts/smoke-secfix1.cjs', "const csvFile = path.join(userData, 'audit-history.csv');", "const csvFile = path.join(shots, 'audit-history.csv');fs.rmSync(csvFile,{force:true});")
edit('electron/scripts/save-live-regression.cjs', "      evidence.push({variant,bookHashUnchanged:true});", '')
edit('electron/scripts/save-live-regression.cjs', "evidence.push({variant,liveBookOverwrittenByPdf:overwritten,defaultPath:state.options.defaultPath,error:state.error});", "evidence.push({variant,liveBookOverwrittenByPdf:overwritten,bookHashUnchanged:true,message:errorText,defaultPath:state.options.defaultPath});")
old="      console.log(JSON.stringify(evidence.at(-1)));"
new=r'''      console.log(JSON.stringify(evidence.at(-1)));
      // Same print action must still support new output and replacing a confirmed regular PDF.
      const normal=path.join(out,`normal-${variant}.pdf`);fs.rmSync(normal,{force:true});
      for(const replace of [false,true]) {
        if(replace)fs.writeFileSync(normal,'OLD PDF');
        await app.evaluate(({dialog},file)=>{dialog.showSaveDialog=async()=>({canceled:false,filePath:file});},normal);
        await page.click('#print-now');await page.waitForFunction(()=>!document.querySelector('#print-now').disabled);
        assert.equal(fs.readFileSync(normal).subarray(0,5).toString(),'%PDF-');
        assert.equal(hash(book),before);
        assert.equal(fs.statSync(normal).mode&0o777,0o600);
        console.log(`SAVE PDF ${replace?'replacement':'new'} OK (${variant})`);
      }
      await page.goto(`fb://app/c/${company}/history/`);
      for(const file of [book,selected]) {
        await app.evaluate(({dialog},file)=>{dialog.showSaveDialog=async()=>({canceled:false,filePath:file});},file);
        const dismissed=page.waitForEvent('dialog');
        await page.click('a:has-text("Download this list")');
        assert.equal((await dismissed).message(),'Choose a different file name. That location is used by FreedomBooks.');
        assert.equal(hash(book),before,'CSV refuses without changing open book hash');
      }
      const csv=path.join(out,`history-${variant}.csv`);fs.rmSync(csv,{force:true});
      for(const replace of [false,true]) {
        if(replace)fs.writeFileSync(csv,'OLD CSV');
        await app.evaluate(({dialog},file)=>{dialog.showSaveDialog=async()=>({canceled:false,filePath:file});},csv);
        await page.click('a:has-text("Download this list")');
        for(let n=0;n<100 && (!fs.existsSync(csv) || fs.readFileSync(csv,'utf8')==='OLD CSV');n++)await page.waitForTimeout(50);
        assert.match(fs.readFileSync(csv,'utf8'),/When,User,Action,Document,Reason/);
        assert.equal(hash(book),before);
        console.log(`SAVE CSV ${replace?'replacement':'new'} OK (${variant})`);
      }
      evidence.push({variant,pdfNew:true,pdfReplace:true,csvRefused:true,csvNew:true,csvReplace:true,bookHashUnchanged:true});'''
edit('electron/scripts/save-live-regression.cjs',old,new)
print('Real app save regressions cover PDF/CSV protected paths, normal output and replacement')

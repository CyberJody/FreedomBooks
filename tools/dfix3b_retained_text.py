"""Use same-draft/same-field membership because journal forms have no line IDs."""
from pathlib import Path
p=Path(__file__).resolve().parents[1]/'crates/fb-core/src/services.rs'
t=p.read_text()
old='''        for (index, row) in input.lines.iter().enumerate() {
            let prior = old_lines.get(index);
            for (key, old) in [("description", prior.map(|l| l.description.as_str())), ("name", prior.map(|l| l.name.as_str()))] {
                if let Some(raw) = row[key].as_str() { py::clean_changed_free_text(raw, old)?; }
            }
        }'''
new='''        // The frozen journal form supplies no stable line identity. Deleting
        // or reordering rows must not turn retained legacy text into new input.
        // Exempt only the same field's exact text from this journal's lines.
        for row in &input.lines {
            for key in ["description", "name"] {
                if let Some(raw) = row[key].as_str() {
                    let retained = old_lines.iter().any(|line| match key {
                        "description" => line.description == raw,
                        "name" => line.name == raw,
                        _ => unreachable!(),
                    });
                    py::clean_changed_free_text(raw, retained.then_some(raw))?;
                }
            }
        }'''
assert t.count(old)==1
p.write_text(t.replace(old,new))

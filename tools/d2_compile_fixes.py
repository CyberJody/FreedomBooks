from pathlib import Path


def replace(path, old, new):
    p=Path(path);text=p.read_text();assert old in text,(path,old);p.write_text(text.replace(old,new))


replace('crates/fb-app/Cargo.toml','uuid = { version = "1.18", features = ["v4"] }','uuid = { version = "1.18", features = ["v4"] }\nchrono-tz = "0.10"')
replace('crates/fb-app/src/screens/d2.rs','|(label,key,optional,detail,route,kind)|','|(label,key,optional,detail,route,kind,_)|')
replace('crates/fb-templates/src/django.rs','''        "Y-m-d" => Ok(day.to_owned()),''','''        "Y-m-d" => Ok(day.to_owned()),
        "Y-m-d H:i:s" | "Y-m-d H:i:s T" => {
            let time=text.get(11..19).unwrap_or("00:00:00");
            Ok(format!("{day} {time}{}",if format.as_deref()==Some("Y-m-d H:i:s T"){" UTC"}else{""}))
        },''')

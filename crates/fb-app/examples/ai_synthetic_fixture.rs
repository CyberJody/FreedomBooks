//! Isolated integration fixture generator, never used by the product. Fixed key, synthetic data only.
#[allow(dead_code)]
#[path = "../tests/ai_support/mod.rs"]
mod ai_support;
fn main() {
    let destination =
        std::path::PathBuf::from(std::env::args().nth(1).expect("isolated output directory"));
    std::fs::create_dir_all(&destination).unwrap();
    let builtin = std::env::args().nth(2).as_deref() == Some("--builtin");
    let mut f = ai_support::Fixture::with_builtin_book(builtin);
    let key_hex = if builtin { fb_app::keys::to_hex(&fb_core::backups::portable::BUILTIN_KEY) } else { "2a".repeat(32) };
    let manifest = serde_json::json!({"company_id":f.company.to_db(),"other_company_id":f.other.to_db(),"bank":f.bank,"income":f.income,"expense":f.expense,"username":"Synthetic operator","user_password":ai_support::PASSWORD,"key_hex":key_hex,"builtin":builtin,"book":destination.join("synthetic.fbbook")});
    f.app.lock();
    std::fs::copy(&f.path, destination.join("synthetic.fbbook")).unwrap();
    if builtin {
        std::fs::copy(fb_app::protection::record_path(&f.path),fb_app::protection::record_path(&destination.join("synthetic.fbbook"))).unwrap();
    }
    std::fs::write(destination.join("fixture.json"), manifest.to_string()).unwrap();
    println!("Created isolated synthetic AI fixture.");
}

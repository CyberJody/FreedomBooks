//! Pin every writer/SQL-binding path across core and desktop. AST-based so multiline calls count.
//! A new path requires an explicit review of its Book::write/final-mask routing.
use std::{collections::BTreeMap, path::Path};
use syn::visit::{self, Visit};

#[derive(Default)]
struct Calls {
    function: String,
    writes: BTreeMap<String, usize>,
    desktop: bool,
    path: String,
}
impl<'a> Visit<'a> for Calls {
    fn visit_item_fn(&mut self, item: &'a syn::ItemFn) {
        if item.attrs.iter().any(|a| a.path().is_ident("test")) { return; }
        let previous = std::mem::replace(&mut self.function, item.sig.ident.to_string());
        visit::visit_item_fn(self, item);
        self.function = previous;
    }
    fn visit_item_mod(&mut self, item: &'a syn::ItemMod) {
        if item.ident == "tests" || item.attrs.iter().any(|a| a.path().is_ident("cfg") && a.meta.require_list().is_ok_and(|l| l.tokens.to_string() == "test")) { return; }
        visit::visit_item_mod(self, item);
    }
    fn visit_impl_item_fn(&mut self, item: &'a syn::ImplItemFn) {
        let previous = std::mem::replace(&mut self.function, item.sig.ident.to_string());
        visit::visit_impl_item_fn(self, item);
        self.function = previous;
    }
    fn visit_expr_method_call(&mut self, item: &'a syn::ExprMethodCall) {
        let method = item.method.to_string();
        if ["write", "execute", "execute_batch", "prepare", "atomic", "with_text_guard"].contains(&method.as_str()) {
            *self.writes.entry(format!("{}:{method}", self.function)).or_default() += 1;
        }
        if self.desktop {
            // These reviewed coordinators use Book::write and its guarded Writer. Keep raw
            // SQLite connections/transactions forbidden everywhere, including these functions.
            let allowed = matches!((self.path.as_str(), self.function.as_str(), method.as_str()),
                ("crates/fb-app/src/identity.rs", "enroll_owner" | "save_desktop_user", "write" | "execute")
                | ("crates/fb-app/src/session.rs", "create_book_with_chart", "write" | "execute")
                | ("crates/fb-app/src/ai/import.rs", "run", "write"));
            assert!(allowed || !["connection", "transaction", "transaction_with_behavior", "execute", "execute_batch", "write"].contains(&method.as_str()), "{}:{}: unreviewed desktop writer: {method}", self.path, self.function);
        }
        visit::visit_expr_method_call(self, item);
    }
}
fn scan(root: &Path, dir: &Path, inventory: &mut BTreeMap<String, BTreeMap<String, usize>>) {
    for item in std::fs::read_dir(dir).unwrap() {
        let path = item.unwrap().path();
        if path.is_dir() { scan(root, &path, inventory); }
        else if path.extension().is_some_and(|s| s == "rs") {
            let relative = path.strip_prefix(root).unwrap().to_str().unwrap();
            if relative.contains("/tests") { continue; }
            let ast = syn::parse_file(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let mut calls = Calls { path: relative.to_owned(), desktop: relative.starts_with("crates/fb-app/") || relative.starts_with("crates/fb-node/"), ..Default::default() };
            calls.visit_file(&ast);
            if !calls.writes.is_empty() { inventory.insert(relative.to_owned(), calls.writes); }
        }
    }
}

#[test]
fn every_core_and_desktop_write_path_is_in_the_central_mask_inventory() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
    let mut actual = BTreeMap::new();
    for crate_name in ["fb-core", "fb-app", "fb-node"] { scan(root, &root.join("crates").join(crate_name).join("src"), &mut actual); }
    // The host's opaque envelope store is a typed fb-db API. Pin its SQL too;
    // desktop callers retain the same prohibition against raw writes.
    let vault = "crates/fb-db/src/plugin_credentials.rs";
    let ast = syn::parse_file(&std::fs::read_to_string(root.join(vault)).unwrap()).unwrap();
    let mut calls = Calls::default();
    calls.visit_file(&ast);
    actual.insert(vault.to_owned(), calls.writes);
    let fixture = root.join("crates/fb-core/tests/data/write_inventory.json");
    if std::env::var_os("FB_UPDATE_WRITE_INVENTORY").is_some() {
        std::fs::write(&fixture, serde_json::to_string_pretty(&actual).unwrap() + "\n").unwrap();
    }
    let expected: BTreeMap<String, BTreeMap<String, usize>> = serde_json::from_slice(&std::fs::read(fixture).unwrap()).unwrap();
    assert_eq!(actual, expected, "write paths changed; review central masking before updating inventory");
    for module in ["sales", "payables", "transfers", "services.rs", "accounts.rs", "documents.rs", "company_documents.rs"] {
        assert!(actual.keys().any(|p| p.contains(module)), "missing {module}");
    }
    let db = std::fs::read_to_string(root.join("crates/fb-db/src/lib.rs")).unwrap();
    assert!(db.contains("text_guards: std::cell::RefCell::new(self.retained_text_guards.values().cloned().collect())"));
    assert!(db.contains("*text = self.guarded_text(text)?;"));
    assert!(db.contains("if !self.text_guards.borrow().is_empty() && !statement.readonly()"));
    assert!(db.contains("if !self.text_guards.borrow().is_empty() { return Err(Self::guarded_sql_error()); }"));
    assert!(db.contains("if !self.pending_credential_masks.is_empty()"));
}

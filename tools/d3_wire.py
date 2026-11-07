"""Wire D3 modules, templates and typed commands with asserted edits."""
from pathlib import Path
import runpy

ROOT = Path(__file__).resolve().parents[1]
def edit(name, old, new):
    p = ROOT / name
    text = p.read_text()
    assert old in text, (name, old)
    p.write_text(text.replace(old, new))

inventory = runpy.run_path(str(ROOT / 'tools/d3_hand_fixes.py'))['FILES']
edit('crates/fb-templates/src/lib.rs', 'pub mod django;', 'pub mod django;\nmod d3;')
edit('crates/fb-templates/src/lib.rs', 'const TEMPLATES: &[(&str, &str)] = &[',
     'const TEMPLATES: &[(&str, &str)] = &[\n' + ''.join(f'    ("{n}.html", include_str!("../templates/{n}.html")),\n' for n in inventory))
edit('crates/fb-templates/src/lib.rs', 'env.add_filter("date", django::date);', 'env.add_filter("date", d3::date);\n        d3::register(&mut env);')
edit('crates/fb-app/src/screens.rs', 'mod d2;', 'mod d2;\nmod d3;')
edit('crates/fb-app/src/screens.rs', '"company_home" => (home(app, &route)?, 200),', '_ if d3::is_route(&name) => d3::render(app, &route, &params, path)?,\n        "company_home" => (home(app, &route)?, 200),')
edit('crates/fb-app/src/screens.rs', '_ if d2::is_form(&route.name) => d2::submit(app, &route, path, fields),', '_ if d2::is_form(&route.name) => d2::submit(app, &route, path, fields),\n        _ if d3::is_route(&route.name) => d3::submit(app, &route, path, fields),')
# D2 already implements the frozen form primitives; share them within screens.
for name in ['put','draw','token','data_value','error','decorate','nonfield','clean_text','clean_date','clean_integer','clean_revision','checked']:
    edit('crates/fb-app/src/screens/d2.rs', 'fn ' + name + '(', 'pub(super) fn ' + name + '(')
edit('crates/fb-core/src/lib.rs', 'mod email_validation;', 'pub mod email_validation;')

commands = '''
    SaveDirectoryRecord { company_id: Id, record_id: Option<Id>, values: DirectoryValues },
    SetDirectoryActive { company_id: Id, kind: DirectoryKind, record_id: Id, active: bool },
    SaveTaxRate { company_id: Id, item_id: Option<Id>, expected_revision: Option<i64>, name: String, description: String, rate: String, agency_id: Option<Id>, new_agency_name: String },
    SaveTaxGroup { company_id: Id, item_id: Option<Id>, expected_revision: Option<i64>, name: String, description: String, member_ids: Vec<Id> },
    SaveTaxSettings { company_id: Id, expected_revision: i64, charge_tax: bool, default_item_id: Option<Id>, owed_basis: String, filing_frequency: String },
    SetTaxItemActive { company_id: Id, item_id: Id, active: bool, expected_revision: i64 },
    FinishTaxBulkStep { company_id: Id, customers: bool, items: bool, digest: String },
    SaveReminderSettings { company_id: Id, days: i64, expected: i64 },
'''
edit('crates/fb-app/src/commands.rs', 'pub enum Command {', 'pub enum Command {' + commands)
types = '''
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DirectoryKind { Customer, Vendor, Item }

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum DirectoryValues {
    Customer { display_name: String, business_name: String, contact_name: String, email: String, phone: String, billing_address: String, notes: String, tax_status: Option<String>, exemption_number: Option<String> },
    Vendor { display_name: String, business_name: String, contact_name: String, email: String, phone: String, billing_address: String, notes: String, default_expense_account: Option<Id>, default_terms: Option<String>, defaults_shown: bool },
    Item { code: String, description: String, item_kind: String, sale_rate: String, purchase_cost: Option<String>, income_account: Option<Id>, expense_account: Option<Id>, taxable: Option<bool> },
}
'''
edit('crates/fb-app/src/commands.rs', '/// Ids travel as the core\'s canonical UUID text.', types + '\n/// Ids travel as the core\'s canonical UUID text.')
edit('crates/fb-app/src/session.rs', 'let open = self.open_mut()?;\n        let actor = open.owner;\n        let book = &mut open.book;\n        let data = match command {',
     'if screens::is_d3_command(&command) { return screens::run_d3_command(self, command); }\n        let open = self.open_mut()?;\n        let actor = open.owner;\n        let book = &mut open.book;\n        let data = match command {')
edit('crates/fb-app/src/session.rs', 'Command::SubmitForm { .. } | Command::ExportHistory{..} => unreachable!("handled above"),',
     'Command::SubmitForm { .. } | Command::ExportHistory{..} => unreachable!("handled above"),\n            _ => unreachable!("D3 commands handled above"),')
edit('crates/fb-app/src/screens.rs', 'pub(crate) use d2::export_history;', 'pub(crate) use d2::export_history;\npub(crate) use d3::{is_d3_command, run_d3_command};')
names = ['SaveDirectoryRecord','SetDirectoryActive','SaveTaxRate','SaveTaxGroup','SaveTaxSettings','SetTaxItemActive','FinishTaxBulkStep','SaveReminderSettings']
edit('electron/main/guards.cjs', "const COMMANDS = new Set([", "const COMMANDS = new Set([" + ', '.join(repr(n) for n in names) + ', ')

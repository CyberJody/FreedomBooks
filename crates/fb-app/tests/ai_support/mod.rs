use fb_app::{App, Command, Outcome, keys};
use fb_core::{CompanyId, UserId, bankfeeds as feeds, services};
use serde_json::{Value, json};
pub const PASSWORD: &str = "Synthetic individual AI password";
pub struct Fixture {
    pub app: App,
    pub path: std::path::PathBuf,
    pub company: CompanyId,
    pub other: CompanyId,
    pub outsider: UserId,
    pub bank: String,
    pub income: String,
    pub expense: String,
    pub other_account: String,
    _dir: tempfile::TempDir,
}
pub fn run(app: &mut App, value: Value) -> Value {
    match app
        .run(serde_json::from_value::<Command>(value).unwrap())
        .unwrap()
    {
        Outcome::Done { data } => data,
        other => panic!("{other:?}"),
    }
}
impl Fixture {
    pub fn new() -> Self {
        Self::with_builtin_book(false)
    }
    pub fn with_builtin_book(builtin: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.fbbook");
        let mut app = App::new();
        let company = if builtin {
            app.create_protected_book(&path,"Synthetic operator","Synthetic local AI",None)
        } else {
            app.create_book(
                &path,
                &keys::MasterKey::new([42; 32]),
                "Synthetic operator",
                "Synthetic local AI",
            )
        }
            .unwrap()
            .company_id;
        app.enroll_owner(PASSWORD).unwrap();
        let mut account = |code: &str, name: &str, detail: &str| {
            run(&mut app,json!({"type":"SaveAccount","company_id":company.to_db(),"code":code,"name":name,"detail_type":detail}))["id"].as_str().unwrap().to_owned()
        };
        let bank = account("100", "Checking", "BANK");
        let ar = account("110", "Receivables", "RECEIVABLE");
        let uf = account("120", "Undeposited", "OTHER_CURRENT_ASSET");
        let income = account("400", "Sales", "INCOME");
        let expense = account("500", "Office", "EXPENSE");
        run(
            &mut app,
            json!({"type":"SaveSetup","company_id":company.to_db(),"expected_revision":0,"receivables_id":ar,"undeposited_id":uf,"bank_ids":[bank]}),
        );
        let o = app.open_mut().unwrap();
        let outsider =
            fb_core::users::create_user(&mut o.book, "Synthetic other user", false).unwrap();
        let other = services::create_company(
            &mut o.book,
            o.owner,
            "Synthetic other company",
            "USD",
            false,
        )
        .unwrap()
        .id;
        let other_account = services::save_account(
            &mut o.book,
            o.owner,
            other,
            services::AccountInput::new("500", "Other Office").detail("EXPENSE"),
        )
        .unwrap()
        .id
        .to_db();
        let secret = feeds::SecretSet::none();
        let conn = feeds::connect(
            &mut o.book,
            o.owner,
            company,
            &json!("Synthetic feed"),
            "bridge.simplefin.org",
            &secret,
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let pull:feeds::StagedPull=serde_json::from_value(json!({"accounts":[{"conn_key":"synthetic","remote_id":"synthetic-checking","name":"Synthetic Checking","org_name":"Synthetic bank","currency":"USD","balance":10000,"available":null,"balance_epoch":1793534400,"transactions":[{"remote_id":"synthetic-1","posted_on":"2026-11-01","transacted_on":null,"amount":-1234,"description":"Synthetic office supplies","pending":false}]}]})).unwrap();
        feeds::store_pull(
            &mut o.book,
            o.owner,
            company,
            &conn,
            &pull,
            "2026-11-01T12:00:00Z",
            false,
            None,
            &secret,
        )
        .unwrap();
        let feed = feeds::items(&mut o.book, o.owner, company).unwrap()[0]
            .feed_account_id
            .clone();
        feeds::map_account(&mut o.book, o.owner, company, &feed, &json!(bank), &secret).unwrap();
        app.ai_select_company(company).unwrap();
        Self {
            app,
            path,
            company,
            other,
            outsider,
            bank,
            income,
            expense,
            other_account,
            _dir: dir,
        }
    }
    pub fn journal(&self, amount: &str) -> Value {
        json!({"request_key":uuid::Uuid::new_v4().to_string(),"date":"2026-11-02","memo":"Synthetic import","lines":[{"account":self.expense,"debit":amount,"credit":"0.00","description":"Office","name":""},{"account":self.bank,"debit":"0.00","credit":amount,"description":"Checking","name":""}]})
    }
    pub fn count(&mut self, table: &str) -> i64 {
        self.app
            .open_mut()
            .unwrap()
            .book
            .read(|q| q.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)))
            .unwrap()
    }
}

//! SQLite single-writer equivalents of document/directory concurrency tests.
mod common;
use fb_core::{Book, CompanyId, directory as dir, directory::Model, documents as d, services, users};
use serde_json::json;
use std::sync::{Arc, Barrier};
fn fixture(tag: &str) -> (std::path::PathBuf, fb_core::UserId, CompanyId) {
    let path = common::book_path(tag);
    let mut b = Book::create(&path, &common::KEY).unwrap();
    let u = users::create_user(&mut b, "writer", false).unwrap();
    let c = services::create_company(&mut b, u, "Concurrent", "USD", false).unwrap().id;
    (path, u, c)
}
#[test]
fn initial_sequence_and_existing_counter_races_allocate_once_in_order() {
    let (path, u, c) = fixture("sequence-race");
    let barrier = Arc::new(Barrier::new(3));
    let workers = (0..2)
        .map(|worker| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut b = Book::open(&path, &common::KEY).unwrap();
                barrier.wait();
                (0..50)
                    .map(|i| {
                        let key = format!("00000000-0000-0000-{worker:04}-{i:012}");
                        b.write(|tx| d::create_identity(tx, u, c, "INV", &key)).unwrap()["ordinal"]
                            .as_i64()
                            .unwrap()
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let mut numbers = workers.into_iter().flat_map(|w| w.join().unwrap()).collect::<Vec<_>>();
    numbers.sort();
    assert_eq!(numbers, (1..=100).collect::<Vec<_>>());
}
#[test]
fn same_key_initial_sequence_race_has_one_identity_and_one_audit() {
    let (path, u, c) = fixture("retry-race");
    let barrier = Arc::new(Barrier::new(3));
    let workers = (0..2)
        .map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut b = Book::open(&path, &common::KEY).unwrap();
                barrier.wait();
                b.write(|tx| d::create_identity(tx, u, c, "INV", "00000000-0000-0000-0000-000000000001"))
                    .unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let ids = workers.into_iter().map(|w| w.join().unwrap()).collect::<Vec<_>>();
    assert_eq!(ids[0], ids[1]);
    let b = Book::open(&path, &common::KEY).unwrap();
    let count: i64 = b
        .connection()
        .query_row(
            "SELECT count(*) FROM ledger_auditevent WHERE action='document.identified'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}
#[test]
fn case_insensitive_item_creation_race_has_one_winner() {
    let (path, u, c) = fixture("item-race");
    let barrier = Arc::new(Barrier::new(3));
    let workers = [" Widget ", "widget"]
        .into_iter()
        .map(|code| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut b = Book::open(&path, &common::KEY).unwrap();
                barrier.wait();
                dir::save_record(
                    &mut b,
                    u,
                    c,
                    Model::Item,
                    &json!({"code":code,"kind":"SERVICE","sale_rate":"12"}),
                    None,
                )
                .is_ok()
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    assert_eq!(workers.into_iter().map(|w| usize::from(w.join().unwrap())).sum::<usize>(), 1);
}

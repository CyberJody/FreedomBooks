#[test]
fn portable_simple_casing_matches_live_postgres_vectors() {
    let rows: Vec<Vec<String>> =
        serde_json::from_str(include_str!("pg_case_vectors.json")).unwrap();
    for row in &rows {
        assert_eq!(
            fb_db::collation::lower(&row[0]),
            row[1],
            "lower {:?}",
            row[0]
        );
        assert_eq!(
            fb_db::collation::upper(&row[0]),
            row[2],
            "upper {:?}",
            row[0]
        );
    }
    println!("{} live PostgreSQL LOWER/UPPER vectors matched", rows.len());
}

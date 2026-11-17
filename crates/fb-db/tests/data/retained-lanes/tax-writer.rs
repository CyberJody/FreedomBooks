
#[test]
fn write_retained_lane_fixture() {
    let out=std::path::PathBuf::from(std::env::var_os("FB_LANE_FIXTURE_OUT").expect("explicit synthetic output"));
    let mut f = Fx::new();
    let group = f.group.clone(); f.invoice("2026-08-10", "200.00", Some(&group));
    let p = f.input(&f.state_agency(),"2026-10-20","3.00"); let _payment = f.pay(&p).unwrap();
    f.book.snapshot(&out, &[31;32]).unwrap();
}

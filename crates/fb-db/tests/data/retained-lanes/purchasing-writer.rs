
#[test]
fn write_retained_lane_fixture() {
    let out=std::path::PathBuf::from(std::env::var_os("FB_LANE_FIXTURE_OUT").expect("explicit synthetic output"));
    let mut f = Fx::new();
    let p = f.payload(); let draft = f.save(&p); let bill = f.post(&draft);
    let p = f.payment(&bill,"0.50"); let _payment = f.pay(&p);
    f.book.snapshot(&out, &[42;32]).unwrap();
}

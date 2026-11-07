//! The Electron plugin smoke's fake providers (WAVE4 item 5): the fake SimpleFIN Bridge and the fake
//! OpenAI issuer from `tests/fakes`, on numeric loopback. Prints one JSON line with their addresses
//! and a one-time setup token, then serves until stdin closes. Synthetic credentials only.
#[path = "../tests/fakes/mod.rs"]
mod fakes;

use std::io::Read;

fn main() {
    let bridge = fakes::FakeBridge::start("SmokeUserQ7x2Kd9Lm4Rt", "SmokePassZ8vN3bW6yHc1Jd5Fs", fakes::sample_accounts());
    let (openai, _provider) = fakes::fake_openai();
    println!(
        "{}",
        serde_json::json!({"simplefin_port": bridge.port(), "setup_token": bridge.setup_token(), "openai": openai.url("")})
    );
    let mut sink = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut sink);
    let requests = bridge.state.lock().map(|s| s.requests.clone()).unwrap_or_default();
    eprintln!("fake bridge requests: {requests:?}");
}

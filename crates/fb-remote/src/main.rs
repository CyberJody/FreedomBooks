//! `fb-remote gateway` (host) or `fb-remote client` (connected computer). See the library docs.
#![forbid(unsafe_code)]
fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let code = match mode.as_str() {
        "gateway" => fb_remote::gateway::run(std::io::stdin().lock(), std::io::stdout()),
        "client" => fb_remote::client_main::run(std::io::stdin().lock(), std::io::stdout()),
        _ => {
            eprintln!("usage: fb-remote gateway|client (started by FreedomBooks, not by hand)");
            2
        }
    };
    std::process::exit(code);
}

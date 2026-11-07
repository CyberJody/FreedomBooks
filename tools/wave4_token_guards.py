"""Install compile-time negative trait assertions on every plaintext token owner."""
from pathlib import Path

root = Path(__file__).resolve().parents[1]
targets = {
    "crates/fb-plugins/simplefin/src/client.rs": ["StoredConnection"],
    "crates/fb-plugins/m365/src/tokens.rs": ["Credentials", "TokenResponse"],
    "crates/fb-plugins/gmail/src/tokens.rs": ["Tokens", "Stored", "Response"],
}
for name, owners in targets.items():
    path = root / name
    source = path.read_text()
    assert "mod token_trait_guard" not in source
    body = '''
#[cfg(test)]
mod token_trait_guard {
    use super::*;
    #[test]
    fn token_owners_cannot_be_serialized_or_debugged() {
        trait AmbiguousIfSerialize<A> { fn check() {} }
        impl<T: ?Sized> AmbiguousIfSerialize<()> for T {}
        struct Serializes;
        impl<T: ?Sized + serde::Serialize> AmbiguousIfSerialize<Serializes> for T {}
        trait AmbiguousIfDebug<A> { fn check() {} }
        impl<T: ?Sized> AmbiguousIfDebug<()> for T {}
        struct Debugs;
        impl<T: ?Sized + std::fmt::Debug> AmbiguousIfDebug<Debugs> for T {}
'''
    for owner in owners:
        body += f"        let _ = <{owner} as AmbiguousIfSerialize<_>>::check;\n"
        body += f"        let _ = <{owner} as AmbiguousIfDebug<_>>::check;\n"
    path.write_text(source + body + "    }\n}\n")

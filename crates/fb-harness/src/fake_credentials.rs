//! Synthetic vault fixtures let differential restores use the production capability
//! path. No test-support capability constructor is enabled in this executable.
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce, aead::{Aead, Payload}};
use fb_core::{Book, FbError, FbResult};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

pub(crate) fn wrapping_key(master: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    let mut key = Zeroizing::new([0; 32]);
    Hkdf::<Sha256>::new(None, master).expand(b"plugin-credentials", &mut *key).expect("valid HKDF length");
    key
}

pub(crate) fn store(book: &mut Book, master: &[u8; 32], company: &str, connection: &str) -> FbResult<()> {
    let wrapper = wrapping_key(master);
    let mut key = Zeroizing::new([0; 32]);
    let mut wn = [0; 12];
    let mut n = [0; 12];
    for bytes in [&mut key[..], &mut wn[..], &mut n[..]] {
        getrandom::fill(bytes).map_err(|_| FbError::validation("Synthetic credential randomness failed."))?;
    }
    let aad = |purpose| fb_db::credential_mask::credential_aad(company, "simplefin", connection, purpose);
    let cipher = |key: &[u8]| ChaCha20Poly1305::new_from_slice(key).expect("32-byte fixture key");
    let wrapped = cipher(&*wrapper).encrypt(Nonce::from_slice(&wn), Payload { msg: &*key, aad: &aad(b"key") }).expect("fixture encryption");
    let encrypted = cipher(&*key).encrypt(Nonce::from_slice(&n), Payload {
        msg: br#"{"access_url":"https://HarnessUser12345:HarnessPassword12345@fixture.test/access"}"#,
        aad: &aad(b"data"),
    }).expect("fixture encryption");
    book.write(|tx| {
        tx.execute("INSERT INTO plugin_credential(company_id,plugin_id,connection_id,wrapped_key,wrap_nonce,cipher_blob,nonce) VALUES(?1,'simplefin',?2,?3,?4,?5,?6)",
            fb_db::rusqlite::params![company, connection, wrapped, wn.as_slice(), encrypted, n.as_slice()])?;
        Ok(())
    })
}

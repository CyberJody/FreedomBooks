//! Typed persistence for opaque credential envelopes. Cryptography and access
//! checks belong to the desktop host; all SQL stays behind the Book writer.
use crate::{Book, rusqlite::{self, OptionalExtension, params}};

pub struct CredentialEnvelope {
    pub wrapped_key: Vec<u8>,
    pub wrap_nonce: Vec<u8>,
    pub cipher_blob: Vec<u8>,
    pub nonce: Vec<u8>,
}

impl Book {
    pub fn credential_envelope(&mut self, company: &str, plugin: &str, connection: &str) -> rusqlite::Result<Option<CredentialEnvelope>> {
        self.read(|tx| tx.query_row(
            "SELECT wrapped_key,wrap_nonce,cipher_blob,nonce FROM plugin_credential WHERE company_id=?1 AND plugin_id=?2 AND connection_id=?3",
            params![company, plugin, connection],
            |r| Ok(CredentialEnvelope { wrapped_key: r.get(0)?, wrap_nonce: r.get(1)?, cipher_blob: r.get(2)?, nonce: r.get(3)? }),
        ).optional())
    }

    pub fn put_credential_envelope(&mut self, company: &str, plugin: &str, connection: &str, envelope: &CredentialEnvelope) -> rusqlite::Result<()> {
        self.write(|tx| tx.execute(
            "INSERT INTO plugin_credential(company_id,plugin_id,connection_id,wrapped_key,wrap_nonce,cipher_blob,nonce) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(company_id,plugin_id,connection_id) DO UPDATE SET wrapped_key=excluded.wrapped_key,wrap_nonce=excluded.wrap_nonce,cipher_blob=excluded.cipher_blob,nonce=excluded.nonce",
            params![company, plugin, connection, envelope.wrapped_key, envelope.wrap_nonce, envelope.cipher_blob, envelope.nonce],
        ))?;
        Ok(())
    }

    pub fn delete_credential_envelope(&mut self, company: &str, plugin: &str, connection: &str) -> rusqlite::Result<()> {
        self.write(|tx| tx.execute(
            "DELETE FROM plugin_credential WHERE company_id=?1 AND plugin_id=?2 AND connection_id=?3",
            params![company, plugin, connection],
        ))?;
        Ok(())
    }
}

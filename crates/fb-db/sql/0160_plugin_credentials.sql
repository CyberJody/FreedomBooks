-- Opaque AEAD envelopes only. The host owns serialization and scope authentication.
CREATE TABLE plugin_credential (
    company_id TEXT NOT NULL REFERENCES ledger_company(id) ON DELETE RESTRICT,
    plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 100),
    connection_id TEXT NOT NULL CHECK(length(connection_id)=36),
    wrapped_key BLOB NOT NULL CHECK(length(wrapped_key)=48),
    wrap_nonce BLOB NOT NULL CHECK(length(wrap_nonce)=12),
    cipher_blob BLOB NOT NULL CHECK(length(cipher_blob) BETWEEN 16 AND 1048592),
    nonce BLOB NOT NULL CHECK(length(nonce)=12),
    PRIMARY KEY(company_id,plugin_id,connection_id)
) STRICT;

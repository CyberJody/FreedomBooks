-- Remote access: the host's sealed record ("Host these books", docs/hosting.md). One row per book,
-- written only by the book process. The record is a JSON document kept as bytes: whether this
-- book has been hosted, the hosting choice and address, the remote audit log's key and kept head,
-- the paired-computer list's key and counter, revoked computers and the host's TLS identity. It
-- lives in the encrypted book so that putting back older copies of the files beside it (the audit
-- log, its checkpoint, the list of paired computers) is detected. It carries no accounting data.
CREATE TABLE remote_host (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    record BLOB NOT NULL CHECK(length(record) BETWEEN 2 AND 1048576)
) STRICT;

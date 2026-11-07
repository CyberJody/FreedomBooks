# W1b-smtp-FIX1 verification — 2026-10-30

Scope: plugin-local fix on `w1b-smtp`, starting at `f5334d2`. No framework changes,
merge, real account, provider connection, UI preview or screenshot. Loopback fakes
and in-memory readers only. `Cargo.lock` is not included in this change.

## Fix

The SMTP transport no longer uses `AsyncSmtpConnection`'s unbounded `read_line`.
`src/transport/connection.rs` uses lettre 0.11.23's TCP/Rustls, command/AUTH and
response helpers with one plugin-owned bounded reader above the TLS layer.
The dependency is pinned exactly because its low-level `AsyncNetworkStream` API
is deprecated. This is a local workaround, not a claim that upstream is fixed.

Limits include prefixes and CRLF: 1,000 bytes per line, 100,000 per response.
Accumulation is a fixed-size boxed slice, with a separate 1,024-byte input buffer.
Overflow is checked before copying each fragment. Errors drop the socket without
waiting for QUIT. TLS verification, required encryption before AUTH, pinned
addresses and the total deadline remain. The SystemResolver/framework endpoint
and accepted/unknown-send findings remain outside this fix.

## Failing first

Before changing production code, ran:

    cargo test --offline -p fb-plugin-smtp unterminated_oversized -- --nocapture

Actual result:

    test result: FAILED. 0 passed; 7 failed; 0 ignored; 0 measured; 36 filtered out; finished in 5.00s

All seven failed the early-rejection assertion: elapsed times were approximately
5.001 seconds with a five-second attempt deadline. Each fake sent 1,001 bytes
without a newline and waited for client closure rather than closing first.

Regression names, under `tests::reply_limits`:

- `unterminated_oversized_greeting_is_rejected_early_and_closed`
- `unterminated_oversized_ehlo_is_rejected_early_and_closed`
- `unterminated_oversized_starttls_reply_is_rejected_early_and_closed`
- `unterminated_oversized_post_tls_ehlo_is_rejected_early_and_closed`
- `unterminated_oversized_auth_is_rejected_early_and_closed`
- `unterminated_oversized_data_is_rejected_early_and_closed`
- `unterminated_oversized_final_reply_is_rejected_early_and_closed`

## Green verification

    cargo test --locked --offline -p fb-plugin-smtp
    test result: ok. 50 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.28s

    cargo clippy --locked --offline -p fb-plugin-smtp --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.15s

    cargo fmt -p fb-plugin-smtp --check
    git diff --check

All commands exited zero. Existing tests still cover HugeGreeting WITH newline,
certificate hostname/chain rejection, required STARTTLS, TLS-only AUTH, both AUTH
mechanisms, MIME and dot-transparency roundtrips, timeout/slow-drip cancellation,
no retry and fixed/redacted errors.

Additional coverage:

- `unterminated_multiline_greeting_obeys_total_response_ceiling`
- `unterminated_multiline_post_tls_ehlo_obeys_total_response_ceiling`
- `unterminated_multiline_final_reply_obeys_total_response_ceiling`
- `exact_line_and_response_ceilings_accept_and_one_extra_byte_rejects`
- `infinite_unterminated_line_and_multiline_response_have_bounded_reads`
- `reply_framing_rejects_bad_crlf_mixed_codes_and_incomplete_responses`
- `response_budget_resets_between_replies_not_between_continuation_lines`

The multiline fakes send exactly 100,000 legal continuation bytes followed by
four unterminated bytes. They prove that a short final line cannot bypass the
whole-response ceiling. Infinite-input tests generate bytes on demand and assert
upstream consumption stops within the relevant ceiling plus one fixed input
buffer, for fragment sizes 1, 7 and 1,024. These are deterministic storage/read
bounds, not a claim about measured whole-process RSS.

A serial, uncaptured-output transport run also passed:

    cargo test --offline -p fb-plugin-smtp tests::reply_limits -- --nocapture --test-threads=1
    test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 40 filtered out; finished in 0.67s

Examples of actual fake observations:

    Greeting StartTls: rejection=2.165301ms, peer_closed=true, bytes=1001
    Greeting ImplicitTls: rejection=44.644644ms, peer_closed=true, bytes=1001
    StartTls StartTls: rejection=2.38091ms, peer_closed=true, bytes=1001
    EncryptedEhlo ImplicitTls: rejection=99.694895ms, peer_closed=true, bytes=100004
    Final ImplicitTls: rejection=99.449715ms, peer_closed=true, bytes=1001

Every stage asserts rejection below one second against a five-second total
limit, a single connection, actual peer EOF/reset and no plaintext AUTH.

## Left / suggested lessons

Independent re-review and lead merge remain. Framework endpoint-policy and
accepted/unknown-send handling remain as already recorded in FRAMEWORK-REQUESTS.
Jody's real SMTP account/app-password sign-in remains deferred to wave 4.

Test unterminated lines and partial final multiline responses, not just large
completed lines. Check byte ceilings below line-oriented parsing and above TLS;
verify the peer sees closure, not merely that an error eventually returns.

# Portable text contract

`fb_text` uses ICU4X `icu_collator` **2.4.0**, compiled data, `en-US`,
Secondary / Shifted / Symbol / case-first False / case-level On. Equal ICU
weights receive a deterministic UTF-8 byte tie-break, as PostgreSQL does.
Every supported OS uses these embedded tables; no installed locale is required.
Changing this version or configuration requires an explicit book/index migration.

The Linux acceptance test generates 20,000 realistic account-code/name strings and
checks 219,999 pairs (including every glibc-sorted adjacent pair). An order
disagreement is accepted **only** when the same strings compare EQUAL with ICU4X
at Tertiary strength and alternate=Shifted (the other settings above retained).
That check omits the byte tie-break. Differences in letters, accents or case still
fail. The harness uses this exact predicate on every pair whose order is inverted;
it reports accepted inversions as **collation ties**, never warnings.

FIX1 found these five exotic pairs. glibc orders the left string first; the chosen
ICU4X order puts the right first (JSON escapes preserve combining marks):

| Left | Right |
| --- | --- |
| `"\u0301[\u00c5\u0131_"` | `"\u00c5\u0131"` |
| `"\u00e9"` | `"E/\u0301"` |
| `"\u0301'k"` | `".\u212a"` |
| `"{,?l"` | `"l:"` |
| `"}y"` | `"y=\\"` |

FIX2 found the realistic pair `"1\u00e2"` / `"1,\u00e2"` (`1â` / `1,â`),
also with left first in glibc and right first in ICU4X. It is a punctuation tie
under the predicate above. The exotic diagnostic deliberately prints all order
differences without failing; the realistic acceptance test fails any disagreement
outside that predicate. This is a measured corpus result, not a claim that every
Unicode string agrees. In particular the diagnostic covers combining marks and
case distinctions outside the realistic alphabet.

The broader FIX3 diagnostic (10,000 exotic strings / 209,999 pairs) prints 859
differences on glibc 2.41, including non-ties involving combining marks and case.
It is intentionally informational; those differences would fail a harness order
comparison. The six pairs above record the FIX1/FIX2 findings, not an exhaustive
enumeration of Unicode differences or a claim that all exotic differences are ties.

Dev Master's decision adopts ICU4X order as the v2 contract: ignorable-punctuation
tie ordering is not meaningful book data, glibc also changes it between releases,
and portable installations need reproducible embedded tables. The frozen Python
answer key remains unchanged.

`fb_lower` and `fb_upper` use ICU4X `icu_casemap` 2.4.0 compiled **simple** Unicode
scalar mappings. They do not perform full-case expansions or locale-sensitive
Turkish casing. Item/tax service duplicate checks use UPPER; their final unique
indexes use LOWER, preserving the Python application's two separate checks.
10,109 vectors captured from PostgreSQL 17 `en_US.UTF-8`/libc prove both mappings
on every realistic-alphabet character and 10,000 seeded strings plus edge cases.
Turkish dotless `ı` stays `ı` under LOWER and becomes `I` under UPPER; thus it
collides with `i` in the service UPPER check. Kelvin sign `K` becomes `k` under
LOWER but stays `K` under UPPER; its collision with `K` is caught by the LOWER
index. These match this PostgreSQL oracle. Dotted `İ` lowers to `i` without a
combining dot, and `ß` does not expand to `SS`. Full Rust string casing is not
an equivalent uniqueness implementation.

Reproduce: `cargo test -p fb-db --test collation --test casing -- --nocapture`;
`cargo test -p fb-db --test collation -- --ignored --nocapture` runs the Linux-only
full diagnostic. The casing probe script is `fb-harness/py/probe_portable_casing.py`.
Definitions: [ICU4X collator](https://docs.rs/icu_collator/2.4.0/icu_collator/),
[ICU4X simple casing](https://docs.rs/icu_casemap/2.4.0/icu_casemap/struct.CaseMapperBorrowed.html).

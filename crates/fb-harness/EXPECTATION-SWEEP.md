# INT-L3-FIX1 scenario expectation sweep

Historical record of the first sweep. The missing-record prepayment expectations
described below were defects, corrected by MAIN-SCENARIOS. See
[SCENARIO-AUDIT.md](SCENARIO-AUDIT.md) for the complete current audit and intended
refusals. Every checked-in step now pins success or a named refusal; the comparer
also rechecks outcomes directly against the scenario file.

The harness now defaults every omitted expectation to success and fails on an
unexpected outcome from either implementation. The first strict run exposed
the following previously unmarked refusals. Steps are zero-based indexes in
the pre-sweep fixtures. Each retained refusal now pins both kind and message.

The L2a cases intentionally exercise duplicate/case uniqueness, invalid fields,
stale previews, numbering, tax configuration and source ownership.
Banking also tests a receipt already held in a live deposit and an invalid
register preset. These refusals are now explicit.
Seeded random workflows intentionally try invalid amounts, unbalanced posting, unauthorized
actors, invalid account changes, stale/refused reviews and already-voided
documents. Random prepayment commands can select application/refund refs whose
earlier record was refused; their DoesNotExist refusals are explicitly pinned.

The half-cent `h9` scientific value `4.9e-3` rounds to zero. Its draft refusal
is now explicit; the dependent post of that nonexistent journal was removed
from both the fixture and its generator. No product financial behavior changed.

| Scenario | Previously unmarked step indexes |
| --- | --- |
| banking-deposits.json | 17 |
| banking-review-register-filters.json | 39 |
| edge-half-cent.json | 30, 31 |
| l2a-collation.json | 50, 51, 52, 53, 54, 55, 58, 59, 62, 63 |
| l2a-directory.json | 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 27, 29 |
| l2a-numbering.json | 36, 37, 38, 40 |
| l2a-rate-edit-oddity.json | 14 |
| l2a-source-lifecycle.json | 12, 13, 14, 15 |
| l2a-tax-rounding.json | 19, 20, 21, 22, 23, 26 |
| prepayments-random-500.json | 16, 20, 21, 22, 23, 24, 25, 26, 27, 28, 32, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 46, 47, 49, 52, 55, 56, 58, 62, 63, 69, 73, 77, 78, 79, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 92, 93, 94, 95, 96, 99, 100, 103, 105, 106, 107, 108, 111, 113, 114, 115, 116, 117, 118, 120, 123, 124, 126, 128, 130, 131, 132, 133, 134, 135, 136, 137, 138, 139, 141, 143, 144, 145, 146, 147, 148, 149, 150, 151, 152, 154, 155, 156, 158, 160, 161, 162, 163, 165, 166, 167, 168, 169, 170, 172, 173, 175, 176, 179, 181, 182, 183, 184, 185, 187, 188, 190, 191, 192, 193, 194, 195, 196, 197, 201, 202, 203, 204, 206, 208, 210, 211, 212, 213, 214, 215, 218, 219, 221, 223, 224, 226, 229, 230, 232, 233, 235, 236, 238, 241, 242, 249, 251, 252, 253, 255, 256, 257, 259, 260, 262, 263, 264, 265, 267, 268, 270, 271, 274, 275, 277, 278, 281, 282, 283, 285, 286, 287, 290, 294, 296, 297, 298, 300, 301, 305, 306, 307, 309, 310, 311, 312, 313, 314, 316, 317, 318, 321, 322, 325, 326, 327, 328, 329, 330, 332, 333, 335, 336, 337, 339, 340, 342, 343, 344, 346, 352, 353, 356, 357, 358, 359, 361, 363, 364, 365, 366, 367, 370, 371, 372, 373, 374, 375, 376, 380, 383, 384, 385, 386, 388, 389, 390, 391, 392, 393, 394, 396, 397, 399, 400, 401, 402, 404, 406, 409, 410, 412, 413, 414, 416, 417, 418, 420, 421, 423, 426, 427, 429, 430, 431, 433, 434, 435, 439, 440, 441, 444, 446, 447, 448, 451, 452, 454, 455, 456, 457, 458, 459, 461, 463, 465, 466, 468, 469, 470, 475, 476, 477, 480, 481, 482, 483, 484, 485, 486, 487, 488, 490, 492, 493, 495, 496, 498, 499, 500, 501, 503, 505, 506, 507, 508, 509, 510, 511, 512 |
| random-1000.json | 76, 201, 234, 236, 309, 314, 334, 370, 413, 491, 499, 533, 543, 548, 581, 626, 629, 655, 661, 667, 683, 688, 732, 781, 796, 831, 860, 886, 895, 916, 922, 964, 983, 1031, 1088, 1136, 1163, 1217, 1252, 1286, 1317, 1406, 1415, 1468, 1483, 1486, 1488, 1511, 1520, 1568, 1650, 1675, 1683, 1700, 1711, 1720, 1752, 1762, 1803, 1818, 1842, 1859, 1860, 1868, 1884, 1887, 1916, 1921, 1947, 2000, 2023, 2030, 2152, 2157, 2166, 2172, 2174, 2189, 2222, 2268, 2278, 2279, 2294, 2314, 2326, 2338, 2341, 2352, 2358 |

Total previously unmarked refusals: **457** (including the removed h9 post).
Total retained refusal expectations pinned across all scenarios: **1445**.

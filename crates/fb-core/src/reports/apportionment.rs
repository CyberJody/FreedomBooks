use crate::Money;
use std::cmp::Ordering;

/// Monotonic Webster (Sainte-Laguë) integer-cent apportionment.
///
/// Guarantees:
/// - Exact sum (`sum(out) == amount`),
/// - Exact proportionality at full weight (`alloc(weights, sum(weights)) == weights`),
/// - Monotonicity in `amount` (increasing `amount` never decreases any component),
/// - Exact odd symmetry (`alloc(weights, -amount) == -alloc(weights, amount)`).
pub(crate) fn monotonic_share(weights: &[i128], amount: Money) -> Vec<Money> {
    let mut out = vec![Money::ZERO; weights.len()];
    if amount.is_zero() {
        return out;
    }
    let active: Vec<(usize, i128)> = weights
        .iter()
        .enumerate()
        .filter(|(_, w)| **w > 0)
        .map(|(i, w)| (i, *w))
        .collect();
    let total: i128 = active.iter().map(|(_, w)| *w).sum();
    if total == 0 {
        return out;
    }
    let sign = if amount.cents() < 0 { -1 } else { 1 };
    let k = amount.cents().abs();
    let full = k / total;
    let rem = k % total;
    let mut counts: Vec<i128> = active
        .iter()
        .map(|(_, w)| (2 * rem * *w + total) / (2 * total))
        .collect();
    let mut sum: i128 = counts.iter().sum();

    let best_add = |counts: &[i128]| -> usize {
        (0..active.len())
            .max_by(|&a, &b| {
                let wa = active[a].1;
                let wb = active[b].1;
                let da = 2 * counts[a] + 1;
                let db = 2 * counts[b] + 1;
                (wa * db).cmp(&(wb * da)).then_with(|| b.cmp(&a))
            })
            .expect("active is non-empty")
    };
    let worst_remove = |counts: &[i128]| -> Option<usize> {
        (0..active.len())
            .filter(|&i| counts[i] > 0)
            .min_by(|&a, &b| {
                let wa = active[a].1;
                let wb = active[b].1;
                let da = 2 * counts[a] - 1;
                let db = 2 * counts[b] - 1;
                (wa * db).cmp(&(wb * da)).then_with(|| b.cmp(&a))
            })
    };

    while sum < rem {
        let idx = best_add(&counts);
        counts[idx] += 1;
        sum += 1;
    }
    while sum > rem {
        let idx = worst_remove(&counts).expect("sum > rem >= 0 implies a positive entry");
        counts[idx] -= 1;
        sum -= 1;
    }
    loop {
        let add = best_add(&counts);
        let Some(remove) = worst_remove(&counts) else {
            break;
        };
        if add == remove {
            break;
        }
        let lhs = active[add].1 * (2 * counts[remove] - 1);
        let rhs = active[remove].1 * (2 * counts[add] + 1);
        let improves = match lhs.cmp(&rhs) {
            Ordering::Greater => true,
            Ordering::Equal => add < remove,
            Ordering::Less => false,
        };
        if improves {
            counts[remove] -= 1;
            counts[add] += 1;
        } else {
            break;
        }
    }
    for (pos, &(orig_idx, w)) in active.iter().enumerate() {
        out[orig_idx] = Money::from_cents(sign * (full * w + counts[pos]));
    }
    out
}

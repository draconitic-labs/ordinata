//! Small numerical primitives, each with a named rule and an independent check.
//!
//! These are written out here rather than pulled from a statistics crate for two
//! reasons. They are the pieces whose exact behaviour has to be reproducible from
//! the bundle by someone who does not have Ordinata, so they must be *specifiable*
//! in a paragraph. And they are the pieces a dependency could silently change
//! between versions, which would move every interval Ordinata has ever published.

use rand_chacha::ChaCha20Rng;
use rand_core::{RngCore, SeedableRng};
use sha2::{Digest, Sha256};

use crate::spec::BOOTSTRAP_SEED_DOMAIN;

/// Evaluate a polynomial by Horner's rule, highest power first.
fn horner(coefficients: &[f64], x: f64) -> f64 {
    let mut value = 0.0;
    for coefficient in coefficients {
        value = value * x + coefficient;
    }
    value
}

// Wichura's AS241 (algorithm PPND16), coefficients highest power first.
#[allow(clippy::unreadable_literal)]
const CENTRAL_NUMERATOR: [f64; 8] = [
    2_509.080_928_730_122_7,
    33_430.575_583_588_13,
    67_265.770_927_008_7,
    45_921.953_931_549_87,
    13_731.693_765_509_46,
    1_971.590_950_306_551_3,
    133.141_667_891_784_38,
    3.387_132_872_796_366_5,
];
#[allow(clippy::unreadable_literal)]
const CENTRAL_DENOMINATOR: [f64; 8] = [
    5_226.495_278_852_854,
    28_729.085_735_721_943,
    39_307.895_800_092_71,
    21_213.794_301_586_597,
    5_394.196_021_424_751,
    687.187_007_492_057_9,
    42.313_330_701_600_91,
    1.0,
];
#[allow(clippy::unreadable_literal)]
const MIDDLE_NUMERATOR: [f64; 8] = [
    7.745_450_142_783_414e-4,
    0.022_723_844_989_269_184,
    0.241_780_725_177_450_6,
    1.270_458_252_452_368_4,
    3.647_848_324_763_204_5,
    5.769_497_221_460_691,
    4.630_337_846_156_546,
    1.423_437_110_749_683_5,
];
#[allow(clippy::unreadable_literal)]
const MIDDLE_DENOMINATOR: [f64; 8] = [
    1.050_750_071_644_416_9e-9,
    5.475_938_084_995_345e-4,
    0.015_198_666_563_616_457,
    0.148_103_976_427_480_08,
    0.689_767_334_985_1,
    1.676_384_830_183_803_8,
    2.053_191_626_637_759,
    1.0,
];
#[allow(clippy::unreadable_literal)]
const TAIL_NUMERATOR: [f64; 8] = [
    2.010_334_399_292_288_1e-7,
    2.711_555_568_743_487_6e-5,
    0.001_242_660_947_388_078_4,
    0.026_532_189_526_576_124,
    0.296_560_571_828_504_87,
    1.784_826_539_917_291_3,
    5.463_784_911_164_114,
    6.657_904_643_501_103,
];
#[allow(clippy::unreadable_literal)]
const TAIL_DENOMINATOR: [f64; 8] = [
    2.044_263_103_389_939_7e-15,
    1.421_511_758_316_446e-7,
    1.846_318_317_510_054_8e-5,
    7.868_691_311_456_133e-4,
    0.014_875_361_290_850_615,
    0.136_929_880_922_735_8,
    0.599_832_206_555_888,
    1.0,
];

/// The standard normal quantile, by Wichura's AS241 (algorithm PPND16).
///
/// Accurate to about sixteen significant figures across the whole open interval,
/// which is far more than an interval bound needs, and it is a published algorithm,
/// so a reader can check it against a reference implementation rather than taking
/// this one on faith.
///
/// Returns `None` for `p` outside `(0, 1)`, where the quantile does not exist.
pub(crate) fn normal_quantile(p: f64) -> Option<f64> {
    if !p.is_finite() || p <= 0.0 || p >= 1.0 {
        return None;
    }

    let q = p - 0.5;
    if q.abs() <= 0.425 {
        let r = 0.180_625 - q * q;
        return Some(q * horner(&CENTRAL_NUMERATOR, r) / horner(&CENTRAL_DENOMINATOR, r));
    }

    let tail = if q < 0.0 { p } else { 1.0 - p };
    let r = (-tail.ln()).sqrt();

    let value = if r <= 5.0 {
        let r = r - 1.6;
        horner(&MIDDLE_NUMERATOR, r) / horner(&MIDDLE_DENOMINATOR, r)
    } else {
        let r = r - 5.0;
        horner(&TAIL_NUMERATOR, r) / horner(&TAIL_DENOMINATOR, r)
    };

    Some(if q < 0.0 { -value } else { value })
}

/// The `p`-quantile of an already-ascending sample, by linear interpolation at rank
/// `(n - 1) * p`. This is Hyndman and Fan type 7, the default in R and `NumPy`.
///
/// `sorted` must be non-empty, ascending, and free of `NaN`.
pub(crate) fn percentile_of_sorted(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() || !(0.0..=1.0).contains(&p) {
        return None;
    }
    let last = sorted.len() - 1;
    if last == 0 {
        return sorted.first().copied();
    }
    #[allow(clippy::cast_precision_loss)]
    let rank = p * last as f64;
    let below = rank.floor();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let index = below as usize;
    let fraction = rank - below;
    let lower = *sorted.get(index)?;
    if fraction == 0.0 {
        return Some(lower);
    }
    let upper = *sorted.get(index + 1)?;
    Some(lower + fraction * (upper - lower))
}

/// Open the named bootstrap seed stream.
///
/// The raw seed the project declares is not used as a `ChaCha` key directly. It is
/// domain-separated first:
///
/// ```text
/// key = SHA-256("ordinata/bootstrap/v1" || seed.to_le_bytes())
/// ```
///
/// Two things follow. Neighbouring seeds (1, 2, 3) produce unrelated streams rather
/// than closely related ones. And the label pins the stream to this use, so if
/// Ordinata ever grows a second random process it cannot accidentally reuse the same
/// numbers.
pub(crate) fn bootstrap_stream(seed: u64) -> ChaCha20Rng {
    let mut hasher = Sha256::new();
    hasher.update(BOOTSTRAP_SEED_DOMAIN.as_bytes());
    hasher.update(seed.to_le_bytes());
    let key: [u8; 32] = hasher.finalize().into();
    ChaCha20Rng::from_seed(key)
}

/// Draw a uniform index in `0..n` by rejection against the largest multiple of `n`
/// that fits in a `u64`.
///
/// The obvious `next_u64() % n` is biased whenever `n` does not divide `2^64`. The
/// bias is tiny at these sizes, but "tiny bias in the resampler" is exactly the kind
/// of thing that is impossible to argue about later, so it is simply removed. The
/// rejection probability is below one in `2^48` for any `n` Ordinata accepts.
///
/// This is spelled out rather than taken from a distribution crate so that the
/// bundle's description of the seed stream is complete: given the seed, the hash rule
/// above, and this rule, a third party can regenerate the exact indices.
///
/// The range and the result are `usize`, because every caller is choosing a row and
/// rows are counted in `usize`. Taking a `u64` and handing one back would push a cast
/// out to each call site, and a cast at a call site is a place where the invariant that
/// makes it safe -- the result is below `n`, and `n` came in as a `usize` -- is no
/// longer written down next to the code that relies on it.
pub(crate) fn uniform_below(rng: &mut ChaCha20Rng, n: usize) -> usize {
    debug_assert!(n > 0, "uniform_below needs a non-empty range");
    if n <= 1 {
        return 0;
    }
    let modulus = n as u64;
    let span: u128 = 1 << 64;
    let usable = span - (span % u128::from(modulus));
    loop {
        let draw = rng.next_u64();
        if u128::from(draw) < usable {
            let index = draw % modulus;
            // `index < modulus`, and `modulus` is `n` widened from a `usize`, so the
            // value came from a `usize` and fits back into one. The conversion cannot
            // fail; `try_from` is used anyway so that if it ever somehow could, the
            // result is a visible zero rather than a silently truncated row index.
            return usize::try_from(index).unwrap_or(0);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]
    use super::*;

    #[test]
    fn normal_quantile_matches_published_values() {
        // Reference values for the standard normal distribution.
        let cases = [
            (0.5, 0.0),
            (0.975, 1.959_963_984_540_054),
            (0.95, 1.644_853_626_951_472_2),
            (0.9, 1.281_551_565_544_600_4),
            (0.99, 2.326_347_874_040_840_8),
            (0.995, 2.575_829_303_548_900_4),
            (0.9995, 3.290_526_731_491_925_5),
            (0.025, -1.959_963_984_540_054),
            (0.75, 0.674_489_750_196_081_7),
        ];
        for (p, expected) in cases {
            let got = normal_quantile(p).unwrap();
            assert!(
                (got - expected).abs() < 1e-11,
                "quantile({p}) = {got}, expected {expected}"
            );
        }
    }

    #[test]
    fn normal_quantile_is_symmetric_and_monotone() {
        let mut previous = f64::NEG_INFINITY;
        let mut step = 1;
        while step < 999 {
            let p = f64::from(step) / 1000.0;
            let value = normal_quantile(p).unwrap();
            assert!(value > previous, "not monotone at {p}");
            previous = value;
            let mirrored = normal_quantile(1.0 - p).unwrap();
            assert!(
                (value + mirrored).abs() < 1e-9,
                "not symmetric at {p}: {value} vs {mirrored}"
            );
            step += 1;
        }
    }

    #[test]
    fn normal_quantile_reaches_far_into_the_tails() {
        // Exercises the r > 5 branch, which a 0.5..0.999 confidence level never hits
        // but which must still be right.
        let deep = normal_quantile(1e-20).unwrap();
        assert!((deep + 9.262_340_089_798_408).abs() < 1e-9, "{deep}");
    }

    #[test]
    fn normal_quantile_refuses_the_endpoints_where_it_does_not_exist() {
        for p in [0.0, 1.0, -0.1, 1.1, f64::NAN, f64::INFINITY] {
            assert!(normal_quantile(p).is_none(), "{p} should have no quantile");
        }
    }

    #[test]
    fn percentile_interpolates_the_way_numpy_does() {
        let sample = [1.0, 2.0, 3.0, 4.0];
        // rank = 0.25 * 3 = 0.75, so three quarters of the way from 1 to 2.
        assert!((percentile_of_sorted(&sample, 0.25).unwrap() - 1.75).abs() < 1e-12);
        assert!((percentile_of_sorted(&sample, 0.5).unwrap() - 2.5).abs() < 1e-12);
        assert!((percentile_of_sorted(&sample, 0.75).unwrap() - 3.25).abs() < 1e-12);
        assert!((percentile_of_sorted(&sample, 0.0).unwrap() - 1.0).abs() < 1e-12);
        assert!((percentile_of_sorted(&sample, 1.0).unwrap() - 4.0).abs() < 1e-12);
    }

    #[test]
    fn percentile_of_a_single_sample_is_that_sample() {
        assert_eq!(percentile_of_sorted(&[7.0], 0.0), Some(7.0));
        assert_eq!(percentile_of_sorted(&[7.0], 0.5), Some(7.0));
        assert_eq!(percentile_of_sorted(&[7.0], 1.0), Some(7.0));
        assert_eq!(percentile_of_sorted(&[], 0.5), None);
    }

    #[test]
    fn the_seed_stream_is_reproducible_and_domain_separated() {
        let draw = |seed: u64| {
            let mut rng = bootstrap_stream(seed);
            (0..4).map(|_| rng.next_u64()).collect::<Vec<_>>()
        };
        assert_eq!(
            draw(42),
            draw(42),
            "the same seed must give the same stream"
        );
        assert_ne!(draw(42), draw(43));
        // Neighbouring seeds are unrelated because the seed is hashed, not used raw.
        assert_ne!(draw(0), draw(1));
        assert_ne!(draw(1), draw(2));
    }

    #[test]
    fn the_seed_stream_is_pinned_to_published_bytes() {
        // These are the first four words of the stream for seed 1. They are pinned
        // because changing them would silently move every bootstrap interval
        // Ordinata has ever published. This test may only be updated together with a
        // bump of `spec::MODEL_VERSION` and the seed rule in the manifest.
        let mut rng = bootstrap_stream(1);
        let observed: Vec<u64> = (0..4).map(|_| rng.next_u64()).collect();
        assert_eq!(observed, PINNED_STREAM_FOR_SEED_1, "the bootstrap seed stream changed; this is a model-version-breaking change, not a test to update");
    }

    const PINNED_STREAM_FOR_SEED_1: [u64; 4] = [
        2_822_814_457_429_046_329,
        16_801_753_624_787_019_377,
        11_075_015_041_852_747_633,
        6_736_927_797_298_618_453,
    ];

    #[test]
    fn uniform_below_stays_in_range_and_covers_it() {
        let mut rng = bootstrap_stream(7);
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..10_000 {
            let value = uniform_below(&mut rng, 5);
            assert!(value < 5);
            seen.insert(value);
        }
        assert_eq!(seen.len(), 5, "every index in range must be reachable");
    }

    #[test]
    fn uniform_below_is_close_to_flat() {
        // Not a rigorous uniformity proof, just a guard against an off-by-one that
        // would starve the last index or double-count the first.
        let mut rng = bootstrap_stream(11);
        let n = 7_usize;
        let draws = 70_000;
        let mut counts = vec![0_u32; n];
        for _ in 0..draws {
            let index = uniform_below(&mut rng, n);
            counts[index] += 1;
        }
        let expected = f64::from(draws) / n as f64;
        for (index, count) in counts.iter().enumerate() {
            let deviation = (f64::from(*count) - expected).abs() / expected;
            assert!(
                deviation < 0.1,
                "index {index} drawn {count} times, expected about {expected}"
            );
        }
    }

    #[test]
    fn uniform_below_handles_a_single_index() {
        let mut rng = bootstrap_stream(1);
        assert_eq!(uniform_below(&mut rng, 1), 0);
    }
}

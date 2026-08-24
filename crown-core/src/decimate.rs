/// Reduce a sample window to at most `width` (min, max) pairs, one per pixel
/// column. Min/max rather than stride-sampling so a single-sample artifact
/// stays visible instead of falling between columns.
///
/// Precondition: every sample must be finite. `f32::min`/`max` ignore NaN,
/// so a non-finite input would silently violate the `lo <= hi` invariant.
/// `Live::push_raw` enforces this before samples ever reach a ring.
pub fn decimate(samples: &[f32], width: usize) -> Vec<(f32, f32)> {
    if width == 0 || samples.is_empty() {
        return Vec::new();
    }
    let cols = width.min(samples.len());
    (0..cols)
        .map(|col| {
            let start = col * samples.len() / cols;
            let end = (((col + 1) * samples.len()) / cols).max(start + 1);
            samples[start..end]
                .iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &v| {
                    (lo.min(v), hi.max(v))
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_nothing() {
        assert!(decimate(&[], 100).is_empty());
        assert!(decimate(&[1.0, 2.0], 0).is_empty());
    }

    #[test]
    fn produces_one_pair_per_column() {
        let samples: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        assert_eq!(decimate(&samples, 100).len(), 100);
    }

    #[test]
    fn never_produces_more_columns_than_samples() {
        assert_eq!(decimate(&[1.0, 2.0, 3.0], 500).len(), 3);
    }

    #[test]
    fn each_pair_is_ordered_and_covers_its_column() {
        let samples: Vec<f32> = (0..100).map(|i| if i % 2 == 0 { -1.0 } else { 1.0 }).collect();
        for (lo, hi) in decimate(&samples, 10) {
            assert!(lo <= hi, "min {lo} must not exceed max {hi}");
            assert_eq!((lo, hi), (-1.0, 1.0));
        }
    }

    #[test]
    fn preserves_a_spike_that_naive_sampling_would_drop() {
        let mut samples = vec![0.0f32; 1000];
        samples[517] = 99.0;
        let out = decimate(&samples, 10);
        assert!(out.iter().any(|&(_, hi)| hi == 99.0), "spike must survive decimation");
    }
}

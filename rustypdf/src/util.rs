//! Small numeric helpers shared across passes.
//!
//! Three passes independently needed "the value most of this material is set at" — line building
//! for a line's font size, document statistics for the body size, and maths reconstruction for a
//! formula's base size. They differ only in what they weigh by, so the shape lives here once.

/// The most-represented value in a weighted sample, within a tolerance.
///
/// Not the mean, and not the plain mode. A paper's headings, captions and footnotes would drag a
/// mean well off the body value, while an exact mode cannot cope with the sub-point variation
/// that real font sizes have. Values within `tolerance` of each other are pooled, and the
/// heaviest pool wins.
pub fn dominant<T>(
    items: impl IntoIterator<Item = T>,
    tolerance: f32,
    value: impl Fn(&T) -> f32,
    weight: impl Fn(&T) -> f32,
) -> Option<f32> {
    let mut pools: Vec<(f32, f32)> = Vec::new();

    for item in items {
        let (v, w) = (value(&item), weight(&item));
        if !v.is_finite() || !w.is_finite() {
            continue;
        }
        match pools
            .iter_mut()
            .find(|(pooled, _)| (*pooled - v).abs() < tolerance)
        {
            Some((_, total)) => *total += w,
            None => pools.push((v, w)),
        }
    }

    pools
        .into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(v, _)| v)
}

/// The median of a sample, or `None` when it is empty.
///
/// Used wherever an outlier would do damage: a line's baseline must not be dragged by a
/// superscript, and leading must not be dragged by a column break.
pub fn median(values: &mut [f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f32::total_cmp);
    Some(values[values.len() / 2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_heaviest_pool_wins_not_the_most_frequent() {
        // Three small items and one heavy one: weight decides.
        let items = [(10.0, 1.0), (10.0, 1.0), (10.0, 1.0), (24.0, 50.0)];
        assert_eq!(dominant(items, 0.25, |i| i.0, |i| i.1), Some(24.0));
    }

    #[test]
    fn near_values_pool_together() {
        // Real font sizes vary by fractions of a point.
        let items = [(9.96, 5.0), (9.97, 5.0), (9.95, 5.0), (12.0, 8.0)];
        assert_eq!(dominant(items, 0.25, |i| i.0, |i| i.1), Some(9.96));
    }

    #[test]
    fn non_finite_values_are_ignored() {
        let items = [(f32::NAN, 100.0), (10.0, 1.0)];
        assert_eq!(dominant(items, 0.25, |i| i.0, |i| i.1), Some(10.0));
    }

    #[test]
    fn an_empty_sample_has_no_dominant_value() {
        let items: [(f32, f32); 0] = [];
        assert_eq!(dominant(items, 0.25, |i| i.0, |i| i.1), None);
    }

    #[test]
    fn median_ignores_order_and_outliers() {
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&mut [1.0, 1.0, 1.0, 1000.0]), Some(1.0));
        assert_eq!(median(&mut []), None);
    }
}

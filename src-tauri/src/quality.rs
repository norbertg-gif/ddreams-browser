// Calibrated 2026-11-12 on ~4600 photos: deliberately soft or shallow-focus shots score 25-100, so only
// flag the extreme end. Sharpness is mainly useful for comparing photos inside one series.
pub const BLUR_MAX_SHARP: f64 = 15.0;
// 10 % clipped is normal for bright skies and high-key portraits; flag only heavy clipping.
pub const OVER_MIN_HI: f64 = 0.20;
pub const UNDER_MAX_MEAN: f64 = 0.08;
pub const QUALITY_VERSION: i64 = 1;

pub fn flags(sharp: f64, hi: f64, _lo: f64, mean: f64) -> Vec<&'static str> {
    let mut result = Vec::new();
    if sharp < BLUR_MAX_SHARP { result.push("blur"); }
    if hi >= OVER_MIN_HI { result.push("over"); }
    if mean <= UNDER_MAX_MEAN { result.push("under"); }
    result
}

pub fn blur_sql() -> String { format!("q.sharp < {BLUR_MAX_SHARP}") }
pub fn over_sql() -> String { format!("q.hi >= {OVER_MIN_HI}") }
pub fn under_sql() -> String { format!("q.mean <= {UNDER_MAX_MEAN}") }
pub fn flag_sql(flag: &str) -> Option<String> {
    match flag { "blur" => Some(blur_sql()), "over" => Some(over_sql()), "under" => Some(under_sql()), _ => None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_use_inclusive_bounds_as_specified() {
        assert_eq!(flags(BLUR_MAX_SHARP - 0.01, OVER_MIN_HI, 0.0, UNDER_MAX_MEAN), vec!["blur", "over", "under"]);
        assert!(flags(BLUR_MAX_SHARP, OVER_MIN_HI - 0.001, 0.0, UNDER_MAX_MEAN + 0.001).is_empty());
    }

    #[test]
    fn unknown_flag_has_no_sql_fragment() { assert_eq!(flag_sql("unknown"), None); }
}

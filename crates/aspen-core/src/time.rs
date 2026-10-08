//! Timestamps as the harnesses write them.

/// ISO-8601 `YYYY-MM-DDTHH:MM:SS(.fff)Z` → epoch seconds (UTC only).
/// Split on its delimiters, never sliced by byte position, so any input
/// is safe. (Three copies existed; the Codex one sliced and panicked on
/// non-ASCII input; the 2026-10 quality pass.)
pub fn parse_iso(s: &str) -> Option<f64> {
    let s = s.trim_end_matches('Z');
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    let mut t = time.split(':');
    let (h, mi): (i64, i64) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    let sec: f64 = t.next()?.parse().ok()?;
    // days from civil (Howard Hinnant)
    let (y2, m2) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days as f64 * 86400.0 + h as f64 * 3600.0 + mi as f64 * 60.0 + sec)
}

#[cfg(test)]
mod tests {
    use super::parse_iso;

    #[test]
    fn parses_and_never_panics() {
        assert_eq!(parse_iso("1970-01-01T00:00:10Z"), Some(10.0));
        assert_eq!(
            parse_iso("2026-09-04T00:00:00.000Z").map(|t| t as i64),
            Some(1788480000)
        );
        let t = parse_iso("2026-09-07T21:50:34.330Z").unwrap();
        assert!((t - 1788817834.33).abs() < 0.001);
        for bad in [
            "",
            "2026",
            "ééééééééééééééééééééé",
            "2026-09-07Tx",
            "2026-ü9-07T00:00:00Z",
        ] {
            assert_eq!(parse_iso(bad), None, "{bad:?}");
        }
    }
}

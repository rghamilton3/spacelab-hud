pub fn fmt_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{}d {}h {:02}m", days, hours, mins)
    } else {
        format!("{}h {:02}m", hours, mins)
    }
}

pub fn fmt_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else {
        format!("{:.0} MB", bytes as f64 / 1_048_576.0)
    }
}

pub fn fmt_load_avg(one: f64, five: f64, fifteen: f64) -> String {
    format!("{:.2}  {:.2}  {:.2}", one, five, fifteen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_uptime_below_one_day_omits_days() {
        assert_eq!(fmt_uptime(0), "0h 00m");
        assert_eq!(fmt_uptime(60), "0h 01m");
        assert_eq!(fmt_uptime(3_661), "1h 01m");
        assert_eq!(fmt_uptime(86_399), "23h 59m");
    }

    #[test]
    fn fmt_uptime_includes_days_at_and_above_one_day() {
        assert_eq!(fmt_uptime(86_400), "1d 0h 00m");
        assert_eq!(fmt_uptime(86_400 + 3600 + 120), "1d 1h 02m");
        assert_eq!(fmt_uptime(3 * 86_400 + 5 * 3600 + 30 * 60), "3d 5h 30m");
    }

    #[test]
    fn fmt_bytes_sub_gib_uses_mb_units() {
        assert_eq!(fmt_bytes(0), "0 MB");
        assert_eq!(fmt_bytes(1_048_576), "1 MB");
        assert_eq!(fmt_bytes(524_288_000), "500 MB");
    }

    #[test]
    fn fmt_bytes_at_or_above_gib_uses_gb_units() {
        assert_eq!(fmt_bytes(1_073_741_824), "1.0 GB");
        assert_eq!(fmt_bytes(5_368_709_120), "5.0 GB");
    }

    #[test]
    fn fmt_bytes_handles_max_value() {
        let s = fmt_bytes(u64::MAX);
        assert!(s.ends_with(" GB"), "expected GB suffix, got: {s}");
    }

    #[test]
    fn fmt_load_avg_uses_two_decimals_and_double_space() {
        assert_eq!(fmt_load_avg(0.0, 0.0, 0.0), "0.00  0.00  0.00");
        assert_eq!(fmt_load_avg(1.5, 2.0, 1.23456), "1.50  2.00  1.23");
    }
}

//! 日期工具:`YYYY-MM-DD` ↔ 距 1970-01-01 的天数(i32)。
//!
//! 采用 Howard Hinnant 的 civil ↔ days 算法(proleptic Gregorian 历),不依赖第三方库。

/// 解析 `YYYY-MM-DD` 为距 1970-01-01 的天数。格式非法返回 None。
pub fn parse_date(s: &str) -> Option<i32> {
    let s = s.trim();
    let mut parts = s.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None; // 多余分段
    }
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d) as i32)
}

/// 把天数格式化为 `YYYY-MM-DD`。
pub fn format_date(days: i32) -> String {
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400; // [0, 399]
    let mi = m as i64;
    let doy = (153 * (if mi > 2 { mi - 3 } else { mi + 9 }) + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_zero() {
        assert_eq!(parse_date("1970-01-01"), Some(0));
        assert_eq!(format_date(0), "1970-01-01");
    }

    #[test]
    fn roundtrip() {
        for s in ["2026-06-18", "2000-02-29", "1999-12-31", "2024-01-01"] {
            let d = parse_date(s).unwrap();
            assert_eq!(format_date(d), s);
        }
    }

    #[test]
    fn ordering() {
        assert!(parse_date("2026-01-01").unwrap() < parse_date("2026-12-31").unwrap());
    }

    #[test]
    fn rejects_bad() {
        assert_eq!(parse_date("not-a-date"), None);
        assert_eq!(parse_date("2026-13-01"), None);
        assert_eq!(parse_date("2026-06"), None);
    }
}

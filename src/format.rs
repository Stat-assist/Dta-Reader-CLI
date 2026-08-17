//! Numeric and date/time formatting.
//!
//! Two concerns live here:
//!   1. Faithful numeric text matching Stata's `export delimited` precision (float = 8 significant
//!      digits, double = 16, integers exact), used for both CSV export and JSON numbers.
//!   2. Stata's %fmt date/time rendering (%tc/%tC/%td/%tw/%tm/%tq/%th/%ty with the detail-code
//!      language), ported from the Java `StataFormat`, used to render date-formatted variables.

use crate::model::VarType;

// 
// Numeric formatting
// 

const FLOAT_SIG: usize = 8;
const DOUBLE_SIG: usize = 16;

/// Faithful numeric text for a non-missing value, given its storage type. Returns a valid JSON
/// number (integers exact; float/double via `%g`-style significant digits with a leading zero
/// kept for |x|<1). Callers that need Stata's CSV rendering pass the result through
/// [`drop_leading_zero`].
pub fn number(value: f64, vtype: &VarType) -> String {
    match vtype {
        VarType::Byte | VarType::Int | VarType::Long => format!("{}", value as i64),
        VarType::Float => format_g(value, FLOAT_SIG),
        VarType::Double => format_g(value, DOUBLE_SIG),
        // A summarize/computed statistic has no storage type; treat as double precision.
        _ => format_g(value, DOUBLE_SIG),
    }
}

/// A computed statistic (mean, sd, ...) at double precision.
pub fn stat_number(value: f64) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    format_g(value, DOUBLE_SIG)
}

/// Stata's CSV rendering drops the leading zero of magnitudes < 1 (".5", "-.5"). JSON must keep it.
pub fn drop_leading_zero(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("0.") {
        format!(".{}", rest)
    } else if let Some(rest) = s.strip_prefix("-0.") {
        format!("-.{}", rest)
    } else {
        s.to_string()
    }
}

/// C `printf("%g")`-style formatting with `sig` significant digits: fixed notation when the
/// decimal exponent E satisfies -4 <= E < sig, else scientific; trailing zeros stripped. Always
/// returns a valid JSON number (keeps the leading zero for |x|<1).
pub fn format_g(value: f64, sig: usize) -> String {
    if value == 0.0 {
        return "0".to_string();
    }
    if !value.is_finite() {
        return "0".to_string();
    }
    let sig = sig.max(1);

    // Round to `sig` significant digits in scientific form to read off the exponent robustly.
    let sci = format!("{:.*e}", sig - 1, value); // e.g. "3.5799999e0", "1.234e-9"
    let (mantissa, exp_str) = sci.split_once('e').expect("scientific format has 'e'");
    let e: i32 = exp_str.parse().expect("valid exponent");

    if e >= -4 && (e as i64) < sig as i64 {
        // Fixed notation with (sig - 1 - E) fractional digits.
        let decimals = (sig as i32 - 1 - e).max(0) as usize;
        let fixed = format!("{:.*}", decimals, value);
        strip_trailing_zeros(&fixed)
    } else {
        // Scientific, Stata style: mantissa (zeros stripped) + e + sign + >=2-digit exponent.
        let mant = strip_trailing_zeros(mantissa);
        let sign = if e < 0 { '-' } else { '+' };
        format!("{}e{}{:02}", mant, sign, e.abs())
    }
}

fn strip_trailing_zeros(s: &str) -> String {
    if !s.contains('.') {
        return s.to_string();
    }
    let trimmed = s.trim_end_matches('0');
    let trimmed = trimmed.strip_suffix('.').unwrap_or(trimmed);
    trimmed.to_string()
}

// 
// Date / time formatting (ported from Java StataFormat)
// 

/// If `fmt` is a Stata date/time format, returns its type char ('c','C','d','w','m','q','h','y')
/// and the detail-code string that follows. Handles a leading justification/zero flag.
pub fn parse_date_format(fmt: &str) -> Option<(char, String)> {
    let bytes: Vec<char> = fmt.chars().collect();
    if bytes.first() != Some(&'%') {
        return None;
    }
    let mut i = 1;
    while i < bytes.len() && matches!(bytes[i], '-' | '~' | '0') {
        i += 1;
    }
    if bytes.get(i) != Some(&'t') {
        return None;
    }
    i += 1;
    let t = *bytes.get(i)?;
    i += 1;
    let details: String = bytes[i..].iter().collect();
    Some((t, details))
}

/// Render a date-formatted numeric value the way Stata's display would (e.g. "20jan2010",
/// "2010m1"). Returns None for generic/business-calendar types we don't interpret.
pub fn render_date(t: char, details: &str, value: f64) -> Option<String> {
    if value.is_nan() {
        return Some(String::new());
    }
    let details = if details.is_empty() {
        default_details(t)
    } else {
        details.to_string()
    };
    let parts = match t {
        'c' | 'C' => DateParts::from_datetime_ms(value as i64),
        'd' => DateParts::from_days(value as i64),
        'w' => DateParts::weekly(value as i64),
        'm' => DateParts::monthly(value as i64),
        'q' => DateParts::quarterly(value as i64),
        'h' => DateParts::half_yearly(value as i64),
        'y' => DateParts::yearly(value as i64),
        _ => return None,
    };
    Some(render_details(&details, &parts))
}

fn default_details(t: char) -> String {
    match t {
        'c' | 'C' => "DDmonCCYY_HH:MM:SS",
        'd' => "DDmonCCYY",
        'w' => "CCYY!www",
        'm' => "CCYY!mnn",
        'q' => "CCYY!qq",
        'h' => "CCYY!hh",
        'y' => "CCYY",
        _ => "",
    }
    .to_string()
}

struct DateParts {
    year: i64,
    month: i64, // 1..12
    day: i64,   // 1..31
    hour: i64,
    minute: i64,
    second: i64,
    millis: i64,
    dow_mon0: i64, // 0=Mon..6=Sun, or -1
    day_of_year: i64,
    week: i64,
    quarter: i64,
    half: i64,
}

impl DateParts {
    fn blank() -> DateParts {
        DateParts {
            year: 0,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            millis: 0,
            dow_mon0: -1,
            day_of_year: -1,
            week: -1,
            quarter: -1,
            half: -1,
        }
    }

    fn from_days(stata_days: i64) -> DateParts {
        // Stata epoch 1960-01-01; Hinnant's civil_from_days uses 1970-01-01 (offset 3653 days).
        let z = stata_days - 3653;
        let (year, month, day) = civil_from_days(z);
        let mut p = DateParts::blank();
        p.year = year;
        p.month = month;
        p.day = day;
        // weekday_from_days: 0=Sun..6=Sat; convert to 0=Mon..6=Sun.
        let dow_sun0 = (z.rem_euclid(7) + 4).rem_euclid(7); // 1970-01-01 is Thursday
        p.dow_mon0 = (dow_sun0 + 6).rem_euclid(7);
        p.day_of_year = stata_days - days_since_stata_epoch(year, 1, 1) + 1;
        p.quarter = (month - 1) / 3 + 1;
        p.half = (month - 1) / 6 + 1;
        p
    }

    fn from_datetime_ms(ms: i64) -> DateParts {
        let day = ms.div_euclid(86_400_000);
        let rem = ms.rem_euclid(86_400_000);
        let mut p = DateParts::from_days(day);
        p.hour = rem / 3_600_000;
        p.minute = (rem % 3_600_000) / 60_000;
        p.second = (rem % 60_000) / 1000;
        p.millis = rem % 1000;
        p
    }

    fn weekly(w: i64) -> DateParts {
        let mut p = DateParts::blank();
        p.year = 1960 + w.div_euclid(52);
        p.week = w.rem_euclid(52) + 1;
        p
    }

    fn monthly(m: i64) -> DateParts {
        let mut p = DateParts::blank();
        p.year = 1960 + m.div_euclid(12);
        p.month = m.rem_euclid(12) + 1;
        p.quarter = (p.month - 1) / 3 + 1;
        p.half = (p.month - 1) / 6 + 1;
        p
    }

    fn quarterly(q: i64) -> DateParts {
        let mut p = DateParts::blank();
        p.year = 1960 + q.div_euclid(4);
        p.quarter = q.rem_euclid(4) + 1;
        p.month = (p.quarter - 1) * 3 + 1;
        p.half = (p.quarter - 1) / 2 + 1;
        p
    }

    fn half_yearly(h: i64) -> DateParts {
        let mut p = DateParts::blank();
        p.year = 1960 + h.div_euclid(2);
        p.half = h.rem_euclid(2) + 1;
        p.month = (p.half - 1) * 6 + 1;
        p.quarter = (p.half - 1) * 2 + 1;
        p
    }

    fn yearly(y: i64) -> DateParts {
        let mut p = DateParts::blank();
        p.year = y;
        p
    }
}

/// Days from 1970-01-01 to civil (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Days from civil (year, month, day) to 1970-01-01, then shifted to Stata's 1960 epoch.
fn days_since_stata_epoch(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days_since_1970 = era * 146097 + doe - 719468;
    days_since_1970 + 3653
}

const MONTH_ABBR: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const MONTH_FULL: [&str; 12] = [
    "january", "february", "march", "april", "may", "june", "july", "august", "september",
    "october", "november", "december",
];
const DAY_ABBR: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const DAY_FULL: [&str; 7] = [
    "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday",
];

// Detail codes, longest-first so greedy matching disambiguates (e.g. "Month" before "Mon").
const CODES: [&str; 39] = [
    "DAYNAME", "Dayname", "Month", "month", "a.m.", "A.M.", ".sss", "Mon", "mon", "JJJ", "jjj",
    "Day", "day", ".ss", "CC", "cc", "YY", "yy", "NN", "nn", "DD", "dd", "Da", "da", "WW", "ww",
    "HH", "Hh", "hH", "hh", "MM", "mm", "SS", "ss", "am", "AM", ".s", "h", "q",
];

fn render_details(details: &str, p: &DateParts) -> String {
    let chars: Vec<char> = details.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '+' => {
                i += 1;
            }
            '!' => {
                if i + 1 < chars.len() {
                    out.push(chars[i + 1]);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            '_' => {
                out.push(' ');
                i += 1;
            }
            '.' | ',' | ':' | '-' | '/' | '\\' => {
                out.push(c);
                i += 1;
            }
            _ => {
                let rest: String = chars[i..].iter().collect();
                if let Some(code) = CODES.iter().find(|code| rest.starts_with(*code)) {
                    out.push_str(&render_code(code, p));
                    i += code.chars().count();
                } else {
                    out.push(c);
                    i += 1;
                }
            }
        }
    }
    out
}

fn render_code(code: &str, p: &DateParts) -> String {
    let hour12 = ((p.hour + 11) % 12) + 1;
    let month_idx = (p.month - 1).clamp(0, 11) as usize;
    let dow = p.dow_mon0;
    match code {
        "CC" => pad2(p.year / 100),
        "cc" => (p.year / 100).to_string(),
        "YY" => pad2(p.year.rem_euclid(100)),
        "yy" => p.year.rem_euclid(100).to_string(),
        "JJJ" => {
            if p.day_of_year < 0 {
                String::new()
            } else {
                format!("{:03}", p.day_of_year)
            }
        }
        "jjj" => {
            if p.day_of_year < 0 {
                String::new()
            } else {
                p.day_of_year.to_string()
            }
        }
        "Month" => capitalize(MONTH_FULL[month_idx]),
        "month" => MONTH_FULL[month_idx].to_string(),
        "Mon" => capitalize(MONTH_ABBR[month_idx]),
        "mon" => MONTH_ABBR[month_idx].to_string(),
        "NN" => pad2(p.month),
        "nn" => p.month.to_string(),
        "DD" => pad2(p.day),
        "dd" => p.day.to_string(),
        "DAYNAME" => {
            if dow < 0 {
                String::new()
            } else {
                pad_right(DAY_FULL[dow as usize], 9)
            }
        }
        "Dayname" => {
            if dow < 0 {
                String::new()
            } else {
                DAY_FULL[dow as usize].to_string()
            }
        }
        "Day" => {
            if dow < 0 {
                String::new()
            } else {
                DAY_ABBR[dow as usize].to_string()
            }
        }
        "Da" => {
            if dow < 0 {
                String::new()
            } else {
                DAY_ABBR[dow as usize][..2].to_string()
            }
        }
        "day" => {
            if dow < 0 {
                String::new()
            } else {
                DAY_ABBR[dow as usize].to_lowercase()
            }
        }
        "da" => {
            if dow < 0 {
                String::new()
            } else {
                DAY_ABBR[dow as usize][..2].to_lowercase()
            }
        }
        "h" => {
            if p.half < 0 {
                String::new()
            } else {
                p.half.to_string()
            }
        }
        "q" => {
            if p.quarter < 0 {
                String::new()
            } else {
                p.quarter.to_string()
            }
        }
        "WW" => {
            if p.week < 0 {
                String::new()
            } else {
                pad2(p.week)
            }
        }
        "ww" => {
            if p.week < 0 {
                String::new()
            } else {
                p.week.to_string()
            }
        }
        "HH" => pad2(p.hour),
        "Hh" => pad2(hour12),
        "hH" => p.hour.to_string(),
        "hh" => hour12.to_string(),
        "MM" => pad2(p.minute),
        "mm" => p.minute.to_string(),
        "SS" => pad2(p.second),
        "ss" => p.second.to_string(),
        ".s" => format!(".{}", p.millis / 100),
        ".ss" => format!(".{}", pad2(p.millis / 10)),
        ".sss" => format!(".{:03}", p.millis),
        "am" | "AM" => am_pm(code, p.hour),
        "a.m." | "A.M." => am_pm_dotted(code, p.hour),
        _ => String::new(),
    }
}

fn am_pm(code: &str, hour: i64) -> String {
    let s = if hour < 12 { "am" } else { "pm" };
    if code == "AM" {
        s.to_uppercase()
    } else {
        s.to_string()
    }
}

fn am_pm_dotted(code: &str, hour: i64) -> String {
    let s = if hour < 12 { "a.m." } else { "p.m." };
    if code == "A.M." {
        s.to_uppercase()
    } else {
        s.to_string()
    }
}

fn pad2(v: i64) -> String {
    format!("{:02}", v)
}

fn pad_right(s: &str, width: usize) -> String {
    let mut out = s.to_string();
    while out.chars().count() < width {
        out.push(' ');
    }
    out
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_and_zero() {
        assert_eq!(number(4099.0, &VarType::Int), "4099");
        assert_eq!(number(-5.0, &VarType::Long), "-5");
        assert_eq!(number(0.0, &VarType::Byte), "0");
    }

    #[test]
    fn float_uses_eight_significant_digits() {
        // 3.58f widened to f64; Stata's export shows "3.5799999".
        let f358 = 3.58_f32 as f64;
        assert_eq!(number(f358, &VarType::Float), "3.5799999");
        assert_eq!(number(2.5_f32 as f64, &VarType::Float), "2.5");
    }

    #[test]
    fn double_uses_sixteen_significant_digits() {
        assert_eq!(number(1.0 / 3.0, &VarType::Double), "0.3333333333333333");
        assert_eq!(
            number(123456789.123456789, &VarType::Double),
            "123456789.1234568"
        );
    }

    #[test]
    fn csv_drops_leading_zero_like_stata() {
        assert_eq!(drop_leading_zero("0.3333333333333333"), ".3333333333333333");
        assert_eq!(drop_leading_zero("-0.5"), "-.5");
        assert_eq!(drop_leading_zero("42"), "42");
    }

    #[test]
    fn dates_daily_and_quarterly() {
        // sp500: %td day 14977 -> 02jan2001; gnp96: %tq quarter 28 -> 1967q1.
        assert_eq!(render_date('d', "", 14977.0).unwrap(), "02jan2001");
        assert_eq!(render_date('q', "", 28.0).unwrap(), "1967q1");
        assert_eq!(render_date('m', "", 0.0).unwrap(), "1960m1");
        assert_eq!(render_date('y', "", 2010.0).unwrap(), "2010");
    }

    #[test]
    fn date_epoch_is_1960() {
        // Stata day 0 is 01jan1960.
        assert_eq!(render_date('d', "", 0.0).unwrap(), "01jan1960");
    }
}

//! Parsing and display helpers shared by the CLI commands.

use std::time::{Duration, SystemTime};

const KIB: u64 = 1024;

/// Binary units (KiB, MiB, GiB...), matching the GUI and the spec.
pub fn human_bytes(bytes: u64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= KIB as f64 && unit < units.len() - 1 {
        value /= KIB as f64;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", units[unit])
    }
}

/// Parses sizes like `100MB`, `5GiB` or `2048`. Units are powers of 1024 for both
/// the `B` and `iB` forms, so `MB` and `MiB` mean the same thing here.
pub fn parse_size(input: &str) -> Result<u64, String> {
    let s = input.trim();
    let split = s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let value: f64 = num.parse().map_err(|_| format!("invalid size '{input}'"))?;
    let multiplier = match unit
        .trim()
        .to_ascii_lowercase()
        .trim_end_matches("ib")
        .trim_end_matches('b')
    {
        "" => 1,
        "k" => KIB,
        "m" => KIB.pow(2),
        "g" => KIB.pow(3),
        "t" => KIB.pow(4),
        _ => return Err(format!("unknown size unit in '{input}'")),
    };
    Ok((value * multiplier as f64) as u64)
}

/// Parses durations like `2d`, `12h`, `30m`, `45s`, `1w`. A bare number means days.
pub fn parse_duration(input: &str) -> Result<Duration, String> {
    let s = input.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let value: u64 = num
        .parse()
        .map_err(|_| format!("invalid duration '{input}'"))?;
    let secs_per_unit = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "d" => 86_400,
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "w" => 604_800,
        _ => return Err(format!("unknown duration unit in '{input}'")),
    };
    Ok(Duration::from_secs(value * secs_per_unit))
}

/// `91d`, `3h` or `unknown`. Unknown ages are shown, never hidden.
pub fn age_label(newest: Option<SystemTime>, now: SystemTime) -> String {
    let Some(t) = newest else {
        return "unknown".into();
    };
    let Ok(age) = now.duration_since(t) else {
        return "0s".into();
    };
    let secs = age.as_secs();
    if secs >= 86_400 {
        format!("{}d", secs / 86_400)
    } else if secs >= 3_600 {
        format!("{}h", secs / 3_600)
    } else {
        format!("{}m", secs / 60)
    }
}

pub fn age_days(newest: Option<SystemTime>, now: SystemTime) -> Option<u64> {
    newest
        .and_then(|t| now.duration_since(t).ok())
        .map(|d| d.as_secs() / 86_400)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("100MB").unwrap(), 100 * 1024 * 1024);
        assert_eq!(parse_size("5GiB").unwrap(), 5 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("2048").unwrap(), 2048);
        assert!(parse_size("12XB").is_err());
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(12), "12 B");
    }

    #[test]
    fn durations() {
        assert_eq!(
            parse_duration("2d").unwrap(),
            Duration::from_secs(2 * 86_400)
        );
        assert_eq!(
            parse_duration("12h").unwrap(),
            Duration::from_secs(12 * 3_600)
        );
        assert_eq!(
            parse_duration("3").unwrap(),
            Duration::from_secs(3 * 86_400)
        );
        assert!(parse_duration("2x").is_err());
    }
}

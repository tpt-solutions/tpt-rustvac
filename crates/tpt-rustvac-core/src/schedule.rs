//! Command lines for the scheduled `rustvac check`. Everything here is a pure function that
//! builds arguments or text, so it can be unit-tested. The CLI runs the commands.
//!
//! Windows registers a per-user task named `rustvac-check` with `schtasks` (no admin needed).
//! Linux and macOS use a user crontab line marked with `# rustvac`.

use std::path::Path;

pub const TASK_NAME: &str = "rustvac-check";
pub const CRON_MARKER: &str = "# rustvac";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Every {
    Daily,
    Weekly,
}

impl std::str::FromStr for Every {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "daily" => Ok(Every::Daily),
            "weekly" => Ok(Every::Weekly),
            _ => Err(format!("unknown interval '{s}' (daily or weekly)")),
        }
    }
}

impl Every {
    pub fn as_str(self) -> &'static str {
        match self {
            Every::Daily => "daily",
            Every::Weekly => "weekly",
        }
    }

    pub fn period(self) -> std::time::Duration {
        std::time::Duration::from_secs(match self {
            Every::Daily => 86_400,
            Every::Weekly => 7 * 86_400,
        })
    }
}

/// Parses `HH:MM`, 24-hour.
pub fn parse_time(s: &str) -> Result<(u8, u8), String> {
    let bad = || format!("invalid time '{s}' (use HH:MM, 24-hour)");
    let (h, m) = s.trim().split_once(':').ok_or_else(bad)?;
    let (h, m): (u8, u8) = (h.parse().map_err(|_| bad())?, m.parse().map_err(|_| bad())?);
    if h > 23 || m > 59 {
        return Err(bad());
    }
    Ok((h, m))
}

/// The command the task runs: `"<exe>" [--config "<path>"] check [--warn-below SIZE] [--notify]`.
fn check_command(
    exe: &Path,
    config: Option<&Path>,
    warn_below: Option<&str>,
    quote: fn(&str) -> String,
) -> String {
    let mut cmd = quote(&exe.to_string_lossy());
    if let Some(c) = config {
        cmd.push_str(" --config ");
        cmd.push_str(&quote(&c.to_string_lossy()));
    }
    cmd.push_str(" check");
    if let Some(w) = warn_below {
        cmd.push_str(" --warn-below ");
        cmd.push_str(w);
        cmd.push_str(" --notify");
    }
    cmd
}

fn dquote(s: &str) -> String {
    format!("\"{s}\"")
}

/// Arguments for `schtasks`, to create or replace the task.
pub fn schtasks_create_args(
    exe: &Path,
    config: Option<&Path>,
    every: Every,
    at: (u8, u8),
    warn_below: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "/create".into(),
        "/tn".into(),
        TASK_NAME.into(),
        "/tr".into(),
        check_command(exe, config, warn_below, dquote),
        "/sc".into(),
    ];
    match every {
        Every::Daily => args.push("DAILY".into()),
        Every::Weekly => {
            args.push("WEEKLY".into());
            args.push("/d".into());
            args.push("MON".into());
        }
    }
    args.push("/st".into());
    args.push(format!("{:02}:{:02}", at.0, at.1));
    args.push("/f".into());
    args
}

pub fn schtasks_query_args() -> Vec<String> {
    vec![
        "/query".into(),
        "/tn".into(),
        TASK_NAME.into(),
        "/v".into(),
        "/fo".into(),
        "list".into(),
    ]
}

pub fn schtasks_delete_args() -> Vec<String> {
    vec![
        "/delete".into(),
        "/tn".into(),
        TASK_NAME.into(),
        "/f".into(),
    ]
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// One crontab line, ending in the marker so it can be found and removed again.
pub fn crontab_line(
    exe: &Path,
    config: Option<&Path>,
    every: Every,
    at: (u8, u8),
    warn_below: Option<&str>,
) -> String {
    let when = match every {
        Every::Daily => format!("{} {} * * *", at.1, at.0),
        Every::Weekly => format!("{} {} * * 1", at.1, at.0),
    };
    format!(
        "{when} {} {CRON_MARKER}",
        check_command(exe, config, warn_below, shell_quote)
    )
}

/// The marked lines in a crontab.
pub fn crontab_find(existing: &str) -> Vec<&str> {
    existing
        .lines()
        .filter(|l| l.contains(CRON_MARKER))
        .collect()
}

/// The crontab with every marked line removed. Other lines are kept untouched.
pub fn crontab_without(existing: &str) -> String {
    let mut out: String = existing
        .lines()
        .filter(|l| !l.contains(CRON_MARKER))
        .collect::<Vec<_>>()
        .join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// The crontab with our line added, replacing any earlier one.
pub fn crontab_with(existing: &str, line: &str) -> String {
    let mut out = crontab_without(existing);
    out.push_str(line);
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn times() {
        assert_eq!(parse_time("09:05").unwrap(), (9, 5));
        assert_eq!(parse_time("23:59").unwrap(), (23, 59));
        assert!(parse_time("24:00").is_err());
        assert!(parse_time("9").is_err());
        assert!(parse_time("ab:cd").is_err());
    }

    #[test]
    fn schtasks_daily() {
        let args = schtasks_create_args(
            &PathBuf::from(r"C:\Program Files\rustvac\rustvac.exe"),
            None,
            Every::Daily,
            (9, 0),
            None,
        );
        assert_eq!(
            args,
            vec![
                "/create",
                "/tn",
                "rustvac-check",
                "/tr",
                r#""C:\Program Files\rustvac\rustvac.exe" check"#,
                "/sc",
                "DAILY",
                "/st",
                "09:00",
                "/f"
            ]
        );
    }

    #[test]
    fn schtasks_weekly_with_config_and_warning() {
        let args = schtasks_create_args(
            &PathBuf::from(r"C:\r\rustvac.exe"),
            Some(&PathBuf::from(r"C:\my cfg\config.toml")),
            Every::Weekly,
            (18, 30),
            Some("50GB"),
        );
        assert_eq!(
            args[4],
            r#""C:\r\rustvac.exe" --config "C:\my cfg\config.toml" check --warn-below 50GB --notify"#
        );
        assert_eq!(
            &args[5..],
            ["/sc", "WEEKLY", "/d", "MON", "/st", "18:30", "/f"]
        );
    }

    #[test]
    fn crontab_lines() {
        let line = crontab_line(
            &PathBuf::from("/usr/bin/rustvac"),
            None,
            Every::Daily,
            (9, 5),
            None,
        );
        assert_eq!(line, "5 9 * * * '/usr/bin/rustvac' check # rustvac");
        let weekly = crontab_line(
            &PathBuf::from("/opt/it's/rustvac"),
            None,
            Every::Weekly,
            (0, 0),
            None,
        );
        assert!(weekly.starts_with("0 0 * * 1 '/opt/it'\\''s/rustvac' check"));
    }

    #[test]
    fn crontab_edit_keeps_other_lines() {
        let existing = "0 1 * * * backup\n5 9 * * * old check # rustvac\n";
        let line = "5 9 * * * new check # rustvac";
        assert_eq!(
            crontab_find(existing),
            vec!["5 9 * * * old check # rustvac"]
        );
        assert_eq!(crontab_without(existing), "0 1 * * * backup\n");
        assert_eq!(
            crontab_with(existing, line),
            "0 1 * * * backup\n5 9 * * * new check # rustvac\n"
        );
        assert_eq!(crontab_with("", line), "5 9 * * * new check # rustvac\n");
        assert_eq!(crontab_without("5 9 * * * x # rustvac\n"), "");
    }
}

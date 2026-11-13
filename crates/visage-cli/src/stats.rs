//! `visage stats`: how often face unlock works on this machine.
//!
//! `visaged` logs one `visage-auth-outcome v1` line per `Verify` call
//! (see `visage_ipc::outcome`). This reads those lines back from the journal
//! and turns them into the numbers a user, or a maintainer reading a field
//! report, needs: how many login attempts succeeded, why the others failed,
//! at what time of day, and how long a successful unlock took.
//!
//! The journal is the store on purpose. The daemon already writes every
//! verify there, it is root-readable only, and journald enforces its own size
//! and age limits. A second store inside `faces.db` would need its own row
//! cap, its own migration and a write on the authentication path.
//!
//! ⚠️ A daemon call is not a login attempt. Since #127 `pam_visage` retries a
//! verify inside its `timeout=N` budget: 200 ms after a transient failure such
//! as "no face", 1.5 s after a face that did not match. One sudo can therefore
//! produce several calls, most of them failures, and still unlock. Counting
//! calls would report those retries as failed logins, so calls closer together
//! than [`ATTEMPT_GAP`] are grouped into one attempt.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, FixedOffset, Local, Timelike};
use std::collections::BTreeMap;
use std::process::Command;
use std::time::Duration;
use visage_ipc::outcome::{self, Outcome, Record};

/// Calls closer together than this belong to one login attempt.
///
/// It is above both of `pam_visage`'s retry waits (200 ms and 1.5 s, plus the
/// D-Bus round trip) and below the time a person takes to type a password
/// after face unlock falls through and then try again.
pub const ATTEMPT_GAP: Duration = Duration::from_secs(3);

/// The order in which an attempt's calls decide what the attempt was.
///
/// A match anywhere wins. Otherwise the most decisive thing seen names the
/// attempt: a face that was compared and rejected says more than "no face",
/// which says more than an error. Dark frames rank above "no face" because a
/// dark frame is why a face is often not found.
const DECISIVE: [Outcome; 8] = [
    Outcome::Matched,
    Outcome::NotMatched,
    Outcome::LivenessFailed,
    Outcome::DarkFrames,
    Outcome::NoFace,
    Outcome::Timeout,
    Outcome::CameraError,
    Outcome::EngineError,
];

/// One `Verify` call, as read from the journal.
#[derive(Debug, Clone, Copy)]
pub struct Call {
    /// When the outcome line was logged, in the journal's local time.
    pub at: DateTime<FixedOffset>,
    pub record: Record,
}

impl Call {
    /// When the call started: the outcome time less the daemon's own timing.
    fn started(&self) -> DateTime<FixedOffset> {
        self.at
            - chrono::Duration::from_std(self.record.elapsed).unwrap_or(chrono::Duration::zero())
    }
}

/// One login attempt: a run of face-attempt calls with no gap over [`ATTEMPT_GAP`].
#[derive(Debug, Clone)]
pub struct Attempt {
    pub start: DateTime<FixedOffset>,
    pub end: DateTime<FixedOffset>,
    pub calls: usize,
    pub outcome: Outcome,
}

impl Attempt {
    pub fn succeeded(&self) -> bool {
        self.outcome == Outcome::Matched
    }
}

/// Parse `journalctl -o short-iso-precise` output.
///
/// Each line is `<timestamp> <host> <identifier>[<pid>]: <message>`. Lines
/// without an outcome line (other daemon output, `-- Boot --` markers) are
/// skipped.
pub fn parse_journal(text: &str) -> Vec<Call> {
    text.lines()
        .filter_map(|line| {
            let (stamp, rest) = line.split_once(' ')?;
            let at = DateTime::parse_from_rfc3339(stamp).ok()?;
            let record = outcome::parse_line(rest)?;
            Some(Call { at, record })
        })
        .collect()
}

/// Group face-attempt calls into login attempts. Calls refused before the
/// camera ran (rate limited, denied, ...) are not attempts and are left out.
pub fn group_attempts(calls: &[Call]) -> Vec<Attempt> {
    let mut face: Vec<&Call> = calls
        .iter()
        .filter(|c| c.record.outcome.is_face_attempt())
        .collect();
    face.sort_by_key(|c| c.started());

    let gap = chrono::Duration::from_std(ATTEMPT_GAP).unwrap_or(chrono::Duration::zero());
    let mut groups: Vec<Vec<&Call>> = Vec::new();
    for call in face {
        match groups.last_mut() {
            Some(group)
                if call.started() - group.iter().map(|c| c.at).max().unwrap_or(call.at) <= gap =>
            {
                group.push(call)
            }
            _ => groups.push(vec![call]),
        }
    }

    groups
        .into_iter()
        .map(|group| {
            let outcome = DECISIVE
                .into_iter()
                .find(|o| group.iter().any(|c| c.record.outcome == *o))
                .unwrap_or(group[0].record.outcome);
            Attempt {
                start: group
                    .iter()
                    .map(|c| c.started())
                    .min()
                    .unwrap_or(group[0].at),
                end: group.iter().map(|c| c.at).max().unwrap_or(group[0].at),
                calls: group.len(),
                outcome,
            }
        })
        .collect()
}

/// Everything `visage stats` prints, in one place so text and JSON agree.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub days: u32,
    pub first: Option<DateTime<FixedOffset>>,
    pub last: Option<DateTime<FixedOffset>>,
    pub calls: usize,
    pub attempts: usize,
    pub succeeded: usize,
    /// Failed attempts by the outcome that decided them.
    pub failed: BTreeMap<Outcome, usize>,
    /// Local hour of the attempt's start → (attempts, succeeded).
    pub by_hour: BTreeMap<u32, (usize, usize)>,
    /// Start of the first call to end of the last, for succeeded attempts.
    pub unlock_median: Option<Duration>,
    pub unlock_p90: Option<Duration>,
    /// Calls refused before the camera ran, by outcome.
    pub refused: BTreeMap<Outcome, usize>,
}

pub fn build_report(calls: &[Call], days: u32) -> Report {
    let attempts = group_attempts(calls);

    let mut failed = BTreeMap::new();
    let mut by_hour: BTreeMap<u32, (usize, usize)> = BTreeMap::new();
    let mut unlock: Vec<Duration> = Vec::new();
    for a in &attempts {
        let hour = by_hour.entry(a.start.hour()).or_default();
        hour.0 += 1;
        if a.succeeded() {
            hour.1 += 1;
            unlock.push((a.end - a.start).to_std().unwrap_or_default());
        } else {
            *failed.entry(a.outcome).or_default() += 1;
        }
    }
    unlock.sort();

    let mut refused = BTreeMap::new();
    for c in calls.iter().filter(|c| !c.record.outcome.is_face_attempt()) {
        *refused.entry(c.record.outcome).or_default() += 1;
    }

    Report {
        days,
        first: calls.iter().map(|c| c.started()).min(),
        last: calls.iter().map(|c| c.at).max(),
        calls: calls.len(),
        attempts: attempts.len(),
        succeeded: attempts.iter().filter(|a| a.succeeded()).count(),
        failed,
        by_hour,
        unlock_median: percentile(&unlock, 50),
        unlock_p90: percentile(&unlock, 90),
        refused,
    }
}

/// Nearest-rank percentile of an ascending slice.
fn percentile(sorted: &[Duration], pct: usize) -> Option<Duration> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (pct * sorted.len()).div_ceil(100).max(1);
    Some(sorted[rank - 1])
}

fn rate(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
}

fn secs(d: Duration) -> String {
    format!("{:.1} s", d.as_secs_f64())
}

pub fn render_text(r: &Report) -> String {
    let mut out = format!("Face unlock on this machine, last {} days\n", r.days);
    let (Some(first), Some(last)) = (r.first, r.last) else {
        out.push_str("\nNo verify outcomes were found in this window.\n");
        return out;
    };
    out.push_str(&format!(
        "Outcomes logged from {} to {}\n\n",
        first.format("%Y-%m-%d %H:%M"),
        last.format("%Y-%m-%d %H:%M")
    ));

    if r.attempts == 0 {
        out.push_str("No face attempts in this window.\n");
    } else {
        out.push_str(&format!(
            "  Login attempts  {:>5}   succeeded {} ({:.0}%)\n",
            r.attempts,
            r.succeeded,
            100.0 * rate(r.succeeded, r.attempts)
        ));
        out.push_str(&format!(
            "  Verify calls    {:>5}   (calls under {} s apart are one attempt)\n",
            r.calls,
            ATTEMPT_GAP.as_secs()
        ));
        if let (Some(median), Some(p90)) = (r.unlock_median, r.unlock_p90) {
            out.push_str(&format!(
                "  Time to unlock          median {}, p90 {}\n",
                secs(median),
                secs(p90)
            ));
        }

        if !r.failed.is_empty() {
            out.push_str("\nFailed attempts by cause\n");
            let mut causes: Vec<_> = r.failed.iter().collect();
            causes.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            for (outcome, n) in causes {
                out.push_str(&format!("  {:<24}{:>5}\n", outcome.describe(), n));
            }
        }

        out.push_str("\nBy hour of day (local)   attempts  succeeded\n");
        for (hour, (attempts, ok)) in &r.by_hour {
            out.push_str(&format!(
                "  {:02}:00                  {:>8}  {:>9}\n",
                hour, attempts, ok
            ));
        }
    }

    if !r.refused.is_empty() {
        let parts: Vec<String> = r
            .refused
            .iter()
            .map(|(o, n)| format!("{} {}", o.describe(), n))
            .collect();
        out.push_str(&format!(
            "\nRefused before the camera ran: {}\n",
            parts.join(", ")
        ));
    }
    out
}

pub fn render_json(r: &Report) -> serde_json::Value {
    let names = |m: &BTreeMap<Outcome, usize>| -> serde_json::Map<String, serde_json::Value> {
        m.iter()
            .map(|(o, n)| (o.as_str().to_string(), serde_json::json!(n)))
            .collect()
    };
    let by_hour: serde_json::Map<String, serde_json::Value> = r
        .by_hour
        .iter()
        .map(|(h, (attempts, ok))| {
            (
                format!("{h:02}"),
                serde_json::json!({ "attempts": attempts, "succeeded": ok }),
            )
        })
        .collect();
    serde_json::json!({
        "format": 1,
        "days": r.days,
        "first": r.first.map(|t| t.to_rfc3339()),
        "last": r.last.map(|t| t.to_rfc3339()),
        "calls": r.calls,
        "attempts": r.attempts,
        "succeeded": r.succeeded,
        "success_rate": rate(r.succeeded, r.attempts),
        "attempt_gap_ms": ATTEMPT_GAP.as_millis() as u64,
        "failed_attempts": names(&r.failed),
        "by_hour": by_hour,
        "unlock_ms": {
            "median": r.unlock_median.map(|d| d.as_millis() as u64),
            "p90": r.unlock_p90.map(|d| d.as_millis() as u64),
        },
        "refused_calls": names(&r.refused),
    })
}

/// `visage stats`: read the journal and print the report.
pub fn run(days: u32, json: bool) -> Result<()> {
    let since = Local::now() - chrono::Duration::days(i64::from(days));
    let output = Command::new("journalctl")
        .args([
            "-u",
            "visaged.service",
            "--since",
            &since.format("%Y-%m-%d %H:%M:%S").to_string(),
            "-o",
            "short-iso-precise",
            "--no-pager",
        ])
        .output()
        .context("failed to run journalctl — visage stats reads the systemd journal")?;
    if !output.status.success() {
        bail!(
            "journalctl failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let calls = parse_journal(&String::from_utf8_lossy(&output.stdout));
    let report = build_report(&calls, days);
    if json {
        println!("{}", serde_json::to_string_pretty(&render_json(&report))?);
    } else {
        print!("{}", render_text(&report));
    }

    if calls.is_empty() {
        eprintln!(
            "\nNothing to count. Either no verify ran in this window, the running visaged \
             is 0.5.0 or older (outcome lines start after 0.5.0), or this account \
             cannot read the system journal: run `sudo visage stats`."
        );
        let hint = String::from_utf8_lossy(&output.stderr);
        if !hint.trim().is_empty() {
            eprintln!("journalctl said: {}", hint.trim());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A journal line as `journalctl -o short-iso-precise` prints it.
    fn line(stamp: &str, outcome: Outcome, ms: u64) -> String {
        format!(
            "{stamp} the-first visaged[642435]: 2026-11-11T21:39:23.031463Z  INFO \
             visaged::dbus_interface: {}",
            outcome::format_line(outcome, Duration::from_millis(ms), Some(0.82))
        )
    }

    fn calls(lines: &[String]) -> Vec<Call> {
        parse_journal(&lines.join("\n"))
    }

    #[test]
    fn parses_outcome_lines_and_skips_everything_else() {
        let text = [
            "-- Boot 1f2e3d4c5b6a --".to_string(),
            "2026-11-11T16:39:20.941833-05:00 the-first visaged[642435]: \
             2026-11-11T21:39:20.941833Z  INFO visaged::dbus_interface: verify requested user=\"cc\""
                .to_string(),
            line("2026-11-11T16:39:23.031463-05:00", Outcome::Matched, 2087),
            "2026-11-11T16:39:23.031500-05:00 the-first visaged[642435]: \u{1b}[2m...\u{1b}[0m \
             visage-auth-outcome v1 outcome=no_face ms=900"
                .to_string(),
            "not a journal line".to_string(),
        ]
        .join("\n");
        let parsed = parse_journal(&text);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].record.outcome, Outcome::Matched);
        assert_eq!(
            parsed[0].at.hour(),
            16,
            "local hour comes from the journal's offset"
        );
        assert_eq!(parsed[1].record.outcome, Outcome::NoFace);
    }

    #[test]
    fn pam_retries_inside_one_budget_are_one_attempt() {
        // no face, retried 200 ms later, then a match: one sudo, one success.
        let c = calls(&[
            line("2026-11-11T10:00:01.000000-05:00", Outcome::NoFace, 1000),
            line("2026-11-11T10:00:02.200000-05:00", Outcome::NoFace, 1000),
            line("2026-11-11T10:00:03.400000-05:00", Outcome::Matched, 1000),
        ]);
        let attempts = group_attempts(&c);
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].calls, 3);
        assert_eq!(attempts[0].outcome, Outcome::Matched);
        assert_eq!(
            (attempts[0].end - attempts[0].start).num_milliseconds(),
            3400,
            "time to unlock runs from the first call's start to the last call's end"
        );
    }

    #[test]
    fn the_slow_retry_after_a_non_match_stays_in_the_attempt() {
        let c = calls(&[
            line(
                "2026-11-11T10:00:02.300000-05:00",
                Outcome::NotMatched,
                2300,
            ),
            // pam_visage sleeps 1.5 s, then calls again.
            line(
                "2026-11-11T10:00:06.000000-05:00",
                Outcome::NotMatched,
                2200,
            ),
        ]);
        let attempts = group_attempts(&c);
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].outcome, Outcome::NotMatched);
    }

    #[test]
    fn the_gap_boundary_is_inclusive_at_three_seconds() {
        let at = |gap_ms: u64| {
            let second_end = 1_000 + gap_ms + 1_000;
            calls(&[
                line("2026-11-11T10:00:01.000000-05:00", Outcome::NoFace, 1000),
                line(
                    &format!(
                        "2026-11-11T10:00:{:02}.{:06}-05:00",
                        second_end / 1000,
                        (second_end % 1000) * 1000
                    ),
                    Outcome::NoFace,
                    1000,
                ),
            ])
        };
        assert_eq!(group_attempts(&at(3_000)).len(), 1);
        assert_eq!(group_attempts(&at(3_001)).len(), 2);
    }

    #[test]
    fn separate_logins_are_separate_attempts() {
        let c = calls(&[
            line(
                "2026-11-11T09:10:00.000000-05:00",
                Outcome::DarkFrames,
                1500,
            ),
            line("2026-11-11T09:12:00.000000-05:00", Outcome::Matched, 900),
        ]);
        let attempts = group_attempts(&c);
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].outcome, Outcome::DarkFrames);
        assert!(attempts[1].succeeded());
    }

    #[test]
    fn the_most_decisive_call_names_a_failed_attempt() {
        let c = calls(&[
            line("2026-11-11T10:00:01.000000-05:00", Outcome::NoFace, 1000),
            line(
                "2026-11-11T10:00:02.200000-05:00",
                Outcome::DarkFrames,
                1000,
            ),
            line("2026-11-11T10:00:03.400000-05:00", Outcome::NoFace, 1000),
        ]);
        assert_eq!(group_attempts(&c)[0].outcome, Outcome::DarkFrames);
    }

    #[test]
    fn calls_refused_before_the_camera_are_counted_apart_and_never_split_an_attempt() {
        let c = calls(&[
            line("2026-11-11T10:00:01.000000-05:00", Outcome::NoFace, 1000),
            line("2026-11-11T10:00:01.100000-05:00", Outcome::RateLimited, 1),
            line("2026-11-11T10:00:02.200000-05:00", Outcome::Matched, 1000),
        ]);
        let report = build_report(&c, 28);
        assert_eq!(report.calls, 3);
        assert_eq!(report.attempts, 1);
        assert_eq!(report.succeeded, 1);
        assert_eq!(report.refused.get(&Outcome::RateLimited), Some(&1));
    }

    #[test]
    fn the_report_counts_attempts_causes_hours_and_unlock_time() {
        let c = calls(&[
            line("2026-11-11T08:30:00.000000-05:00", Outcome::NoFace, 1000),
            line("2026-11-11T08:31:00.000000-05:00", Outcome::Matched, 2000),
            line(
                "2026-11-11T08:40:00.000000-05:00",
                Outcome::DarkFrames,
                1500,
            ),
            // Logged at 15:00:01 after a 1 s verify: it started at 15:00:00.
            line("2026-11-11T15:00:01.000000-05:00", Outcome::Matched, 1000),
            // Logged at 16:00:00 after a 1 s verify: it started at 15:59:59, and
            // an attempt is filed under the hour it started in.
            line("2026-11-11T16:00:00.000000-05:00", Outcome::NoFace, 1000),
        ]);
        let r = build_report(&c, 28);
        assert_eq!((r.attempts, r.succeeded), (5, 2));
        assert_eq!(r.failed.get(&Outcome::NoFace), Some(&2));
        assert_eq!(r.failed.get(&Outcome::DarkFrames), Some(&1));
        assert_eq!(r.by_hour.get(&8), Some(&(3, 1)));
        assert_eq!(r.by_hour.get(&15), Some(&(2, 1)));
        assert_eq!(r.by_hour.get(&16), None);
        assert_eq!(r.unlock_median, Some(Duration::from_millis(1000)));
        assert_eq!(r.unlock_p90, Some(Duration::from_millis(2000)));

        let text = render_text(&r);
        assert!(text.contains("succeeded 2 (40%)"), "{text}");
        assert!(text.contains("no face found"), "{text}");
        assert!(text.contains("08:00"), "{text}");

        let json = render_json(&r);
        assert_eq!(json["format"], 1);
        assert_eq!(json["attempts"], 5);
        assert_eq!(json["success_rate"], 0.4);
        assert_eq!(json["failed_attempts"]["dark_frames"], 1);
        assert_eq!(json["by_hour"]["08"]["attempts"], 3);
        assert_eq!(json["unlock_ms"]["p90"], 2000);
    }

    #[test]
    fn an_empty_journal_says_so_instead_of_reporting_zero_percent() {
        let r = build_report(&[], 28);
        assert_eq!(r.attempts, 0);
        let text = render_text(&r);
        assert!(text.contains("No verify outcomes"), "{text}");
        assert!(!text.contains('%'), "{text}");
        assert_eq!(render_json(&r)["success_rate"], 0.0);
    }

    #[test]
    fn percentile_uses_nearest_rank() {
        let ms = |v: &[u64]| {
            v.iter()
                .map(|&m| Duration::from_millis(m))
                .collect::<Vec<_>>()
        };
        assert_eq!(percentile(&[], 50), None);
        assert_eq!(percentile(&ms(&[7]), 90), Some(Duration::from_millis(7)));
        let ten = ms(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(percentile(&ten, 50), Some(Duration::from_millis(5)));
        assert_eq!(percentile(&ten, 90), Some(Duration::from_millis(9)));
    }
}

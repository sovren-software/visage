//! The one line `visaged` logs when a `Verify` call ends, defined once.
//!
//! `visage stats` reads face-unlock reliability out of the journal, so the line
//! the daemon writes and the line the CLI parses are a contract between two
//! processes. It lives here for the same reason the D-Bus proxy does: a second
//! hand-written copy of a wire format drifts, and it fails silently, as a
//! report that counts nothing.
//!
//! The line is versioned (`v1`). A reader ignores any other version rather
//! than guessing at it, so a future format change cannot be misread as data.
//!
//! It carries no username. The caller chooses the `user` argument, so putting
//! it into a `key=value` line would let a caller write its own fields into the
//! log. Reliability is a property of the machine and its camera, not of a user.

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

/// Marks the line. A reader finds it anywhere in a journal message, so the
/// subscriber's own prefix (timestamp, level, target) does not matter.
pub const TAG: &str = "visage-auth-outcome v1";

/// How one `Verify` call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Outcome {
    /// A face matched an enrolled model.
    Matched,
    /// A face was compared and did not match.
    NotMatched,
    /// A face matched or was compared, and the liveness check rejected it.
    LivenessFailed,
    /// Frames were usable but no face was found in any of them.
    NoFace,
    /// Every frame was too dark to use.
    DarkFrames,
    /// The camera failed.
    CameraError,
    /// Detection, recognition or the engine thread failed.
    EngineError,
    /// The verify ran out of time.
    Timeout,
    /// The rate limiter refused the call before the camera ran.
    RateLimited,
    /// The caller was not allowed to verify this user, or could not be identified.
    Denied,
    /// The user does not exist.
    UnknownUser,
    /// The user has no enrolled models.
    NoModels,
    /// The face store could not be read.
    StoreError,
}

impl Outcome {
    /// Every outcome, in report order.
    pub const ALL: [Outcome; 13] = [
        Outcome::Matched,
        Outcome::NotMatched,
        Outcome::LivenessFailed,
        Outcome::NoFace,
        Outcome::DarkFrames,
        Outcome::CameraError,
        Outcome::EngineError,
        Outcome::Timeout,
        Outcome::RateLimited,
        Outcome::Denied,
        Outcome::UnknownUser,
        Outcome::NoModels,
        Outcome::StoreError,
    ];

    /// The token written into the line.
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Matched => "matched",
            Outcome::NotMatched => "not_matched",
            Outcome::LivenessFailed => "liveness_failed",
            Outcome::NoFace => "no_face",
            Outcome::DarkFrames => "dark_frames",
            Outcome::CameraError => "camera_error",
            Outcome::EngineError => "engine_error",
            Outcome::Timeout => "timeout",
            Outcome::RateLimited => "rate_limited",
            Outcome::Denied => "denied",
            Outcome::UnknownUser => "unknown_user",
            Outcome::NoModels => "no_models",
            Outcome::StoreError => "store_error",
        }
    }

    /// A short phrase for a person reading a report.
    pub fn describe(self) -> &'static str {
        match self {
            Outcome::Matched => "matched",
            Outcome::NotMatched => "face did not match",
            Outcome::LivenessFailed => "liveness check failed",
            Outcome::NoFace => "no face found",
            Outcome::DarkFrames => "only dark frames",
            Outcome::CameraError => "camera error",
            Outcome::EngineError => "engine error",
            Outcome::Timeout => "timed out",
            Outcome::RateLimited => "rate limited",
            Outcome::Denied => "caller denied",
            Outcome::UnknownUser => "unknown user",
            Outcome::NoModels => "no enrolled models",
            Outcome::StoreError => "face store error",
        }
    }

    /// True when the camera ran: the call was a real attempt to recognise a
    /// face, so it counts toward the success rate. The others were refused
    /// before the camera started and say nothing about recognition.
    pub fn is_face_attempt(self) -> bool {
        !matches!(
            self,
            Outcome::RateLimited
                | Outcome::Denied
                | Outcome::UnknownUser
                | Outcome::NoModels
                | Outcome::StoreError
        )
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Outcome {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Outcome::ALL.into_iter().find(|o| o.as_str() == s).ok_or(())
    }
}

/// One parsed outcome line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Record {
    pub outcome: Outcome,
    /// From "verify requested" to the outcome, as the daemon measured it.
    pub elapsed: Duration,
    /// Present only when a face was compared against the gallery.
    pub similarity: Option<f32>,
}

/// The line `visaged` logs for one ended `Verify` call.
///
/// `similarity` is written only for [`Outcome::Matched`] and
/// [`Outcome::NotMatched`]: anywhere else no comparison happened, and a 0.0
/// would read as a real score.
pub fn format_line(outcome: Outcome, elapsed: Duration, similarity: Option<f32>) -> String {
    let mut line = format!("{TAG} outcome={outcome} ms={}", elapsed.as_millis());
    if matches!(outcome, Outcome::Matched | Outcome::NotMatched) {
        if let Some(s) = similarity {
            line.push_str(&format!(" similarity={s:.4}"));
        }
    }
    line
}

/// Parse the outcome line out of a journal message.
///
/// The message may carry a subscriber prefix and ANSI colour codes, so the
/// tag is searched for rather than expected at the start. Returns `None` for
/// any message without a `v1` tag, an unknown outcome, or a missing `ms`.
pub fn parse_line(message: &str) -> Option<Record> {
    let clean = strip_ansi(message);
    let rest = &clean[clean.find(TAG)? + TAG.len()..];
    // The tag must end at a field boundary: "v1" must not match "v10".
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }

    let mut outcome = None;
    let mut elapsed = None;
    let mut similarity = None;
    for field in rest.split_whitespace() {
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        match key {
            "outcome" => outcome = value.parse::<Outcome>().ok(),
            "ms" => elapsed = value.parse::<u64>().ok().map(Duration::from_millis),
            "similarity" => similarity = value.parse::<f32>().ok(),
            _ => {}
        }
    }

    Some(Record {
        outcome: outcome?,
        elapsed: elapsed?,
        similarity,
    })
}

/// Remove ANSI CSI escape sequences (`ESC [ ... letter`).
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_outcome_round_trips_through_the_line() {
        for outcome in Outcome::ALL {
            let line = format_line(outcome, Duration::from_millis(2087), Some(0.8247));
            let record = parse_line(&line).unwrap_or_else(|| panic!("unparsed: {line}"));
            assert_eq!(record.outcome, outcome, "{line}");
            assert_eq!(record.elapsed, Duration::from_millis(2087), "{line}");
        }
    }

    #[test]
    fn similarity_is_written_only_where_a_face_was_compared() {
        for outcome in Outcome::ALL {
            let record =
                parse_line(&format_line(outcome, Duration::from_millis(5), Some(0.5))).unwrap();
            let compared = matches!(outcome, Outcome::Matched | Outcome::NotMatched);
            assert_eq!(record.similarity.is_some(), compared, "{outcome}");
        }
        let record = parse_line(&format_line(
            Outcome::Matched,
            Duration::from_millis(5),
            Some(0.82469),
        ))
        .unwrap();
        assert!((record.similarity.unwrap() - 0.8247).abs() < 1e-6);
    }

    #[test]
    fn the_line_is_found_inside_a_coloured_journal_message() {
        // The shape tracing-subscriber's fmt layer writes to the journal when
        // ANSI is on: timestamp, coloured level and target, then the message.
        let message = "\u{1b}[2m2026-11-11T21:39:23.031463Z\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m \
                       \u{1b}[2mvisaged::dbus_interface\u{1b}[0m\u{1b}[2m:\u{1b}[0m \
                       visage-auth-outcome v1 outcome=no_face ms=1530";
        let record = parse_line(message).unwrap();
        assert_eq!(record.outcome, Outcome::NoFace);
        assert_eq!(record.elapsed, Duration::from_millis(1530));
        assert_eq!(record.similarity, None);
    }

    #[test]
    fn other_lines_and_other_versions_are_ignored() {
        for message in [
            "verify complete user=\"cc\" matched=true similarity=0.82",
            "verify failed error=no face detected in any captured frame",
            "visage-auth-outcome v2 outcome=matched ms=10",
            "visage-auth-outcome v10 outcome=matched ms=10",
            "visage-auth-outcome v1 outcome=sideways ms=10",
            "visage-auth-outcome v1 outcome=matched",
            "visage-auth-outcome v1 outcome=matched ms=soon",
        ] {
            assert_eq!(parse_line(message), None, "{message}");
        }
    }

    #[test]
    fn only_outcomes_where_the_camera_ran_are_face_attempts() {
        let refused: Vec<_> = Outcome::ALL
            .into_iter()
            .filter(|o| !o.is_face_attempt())
            .collect();
        assert_eq!(
            refused,
            [
                Outcome::RateLimited,
                Outcome::Denied,
                Outcome::UnknownUser,
                Outcome::NoModels,
                Outcome::StoreError
            ]
        );
    }

    #[test]
    fn outcome_tokens_are_unique_and_parse_back() {
        let mut seen = std::collections::HashSet::new();
        for outcome in Outcome::ALL {
            assert!(seen.insert(outcome.as_str()), "duplicate {outcome}");
            assert_eq!(outcome.as_str().parse::<Outcome>(), Ok(outcome));
        }
    }
}

//! Contract: every error the PAM module refuses to retry is one visaged still produces.
//!
//! `pam_visage` retries a failed `Verify` until its `timeout=N` budget runs out,
//! except for errors no retry can fix: an active lockout, a missing enrollment,
//! an unknown user, a refused caller. It recognises those by substring, in the
//! `PERMANENT_ERRORS` table in `crates/pam-visage/src/lib.rs`, because D-Bus
//! carries them as free text.
//!
//! The two crates share no constant, so a reworded daemon message would not be a
//! build error. It would silently move that error into the retry path. For a
//! lockout that means calling `Verify` every 200 ms for the whole budget against
//! a counter that is already tripped. This test makes the rewording fail CI.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/visaged should be two levels below the repo root")
        .to_path_buf()
}

/// The first string literal of each `("needle", "reason")` entry in
/// `PERMANENT_ERRORS`.
fn permanent_error_needles() -> Vec<String> {
    let path = repo_root().join("crates/pam-visage/src/lib.rs");
    let src = std::fs::read_to_string(&path).expect("read pam-visage/src/lib.rs");
    let start = src
        .find("const PERMANENT_ERRORS")
        .expect("pam-visage must declare PERMANENT_ERRORS");
    let body = &src[start..];
    let end = body
        .find("];")
        .expect("PERMANENT_ERRORS must end with `];`");
    body[..end]
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("(\""))
        .map(|l| {
            let rest = &l[2..];
            rest[..rest.find('"').expect("closing quote")].to_string()
        })
        .collect()
}

/// All of visaged's own source, concatenated.
fn daemon_source() -> String {
    let dir = repo_root().join("crates/visaged/src");
    let mut all = String::new();
    for entry in std::fs::read_dir(&dir).expect("read crates/visaged/src") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_some_and(|e| e == "rs") {
            all.push_str(&std::fs::read_to_string(&path).expect("read source file"));
        }
    }
    all
}

#[test]
fn the_table_is_found_and_not_empty() {
    // Guards the parser: if it silently returned nothing, the next test would
    // pass with zero assertions.
    let needles = permanent_error_needles();
    assert!(
        needles.len() >= 4,
        "expected at least 4 PERMANENT_ERRORS entries, parsed {needles:?}"
    );
}

#[test]
fn every_permanent_error_is_still_produced_by_the_daemon() {
    let daemon = daemon_source();
    for needle in permanent_error_needles() {
        assert!(
            daemon.contains(&needle),
            "pam-visage treats errors containing {needle:?} as permanent, but no file in \
             crates/visaged/src contains that text any more. Update PERMANENT_ERRORS to \
             the daemon's new wording, or a lockout will be retried until timeout=N."
        );
    }
}

#[test]
fn the_search_can_fail() {
    // Control: the check above must be able to report a missing message.
    assert!(!daemon_source().contains("this text is in no visaged source file"));
}

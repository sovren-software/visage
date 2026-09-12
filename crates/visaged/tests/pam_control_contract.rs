//! Contract: the PAM control string is valid, and identical everywhere we ship it.
//!
//! This is the test for the worst bug in this project's history.
//!
//! From v0.1.0 to v0.3.2 the packaging shipped `[success=end default=ignore]`.
//! `end` is not a valid PAM action. libpam does not reject an unknown keyword —
//! it silently treats it as `ignore`, so **face authentication was a complete
//! no-op on the documented setup path for two minor releases**, and everything
//! looked fine: the module loaded, the daemon ran, `sudo` asked for a password
//! and nobody could tell that was the fallback rather than a failed match.
//!
//! Nothing in the Rust code was wrong. The defect lived entirely in a string in
//! a packaging file, which is why no unit test could reach it.
//!
//! The string is duplicated across three packaging formats with no shared
//! constant, so it can also drift between them — a second failure mode where
//! Debian users and NixOS users get different authentication behaviour from the
//! same release.

use std::path::{Path, PathBuf};

/// Every file that declares the PAM control for `pam_visage.so`.
const DECLARING_FILES: &[&str] = &[
    "packaging/debian/pam-auth-update",
    "packaging/aur/visage.install",
    "packaging/nix/module.nix",
    "packaging/rpm/visage.pam",
];

/// Valid PAM actions, per `pam.conf(5)`. Anything else — including `end` — is
/// silently treated as `ignore` by libpam.
const VALID_ACTIONS: &[&str] = &["ignore", "bad", "die", "ok", "done", "reset"];

/// Valid tokens on the left of `=` in a `[...]` control, per `pam.conf(5)`.
const VALID_TOKENS: &[&str] = &[
    "default",
    "success",
    "open_err",
    "symbol_err",
    "service_err",
    "system_err",
    "buf_err",
    "perm_denied",
    "auth_err",
    "cred_insufficient",
    "authinfo_unavail",
    "user_unknown",
    "maxtries",
    "new_authtok_reqd",
    "acct_expired",
    "session_err",
    "cred_unavail",
    "cred_expired",
    "cred_err",
    "no_module_data",
    "conv_err",
    "authtok_err",
    "authtok_recover_err",
    "authtok_lock_busy",
    "authtok_disable_aging",
    "try_again",
    "ignore",
    "abort",
    "authtok_expired",
    "module_unknown",
    "bad_item",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/visaged should be two levels below the repo root")
        .to_path_buf()
}

/// Every `[...]` control declared for `pam_visage.so` in a file, with its line
/// number for a useful failure message.
fn controls_in(rel: &str) -> Vec<(usize, String)> {
    let path = repo_root().join(rel);
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));

    body.lines()
        .enumerate()
        .filter(|(_, line)| {
            let t = line.trim_start();
            // Comments never configure PAM. `#` covers shell and nix; the deb
            // profile uses no comment syntax on its Auth lines.
            !t.starts_with('#') && (t.contains("pam_visage.so") || t.contains("control ="))
        })
        .filter_map(|(i, line)| {
            let open = line.find('[')?;
            let close = line[open..].find(']')? + open;
            Some((i + 1, line[open..=close].to_string()))
        })
        .collect()
}

/// Split `[a=b c=d]` into its `(token, action)` pairs.
fn parse_control(control: &str) -> Vec<(String, String)> {
    control
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split_whitespace()
        .filter_map(|pair| {
            let (tok, action) = pair.split_once('=')?;
            Some((tok.to_string(), action.to_string()))
        })
        .collect()
}

fn all_controls() -> Vec<(&'static str, usize, String)> {
    DECLARING_FILES
        .iter()
        .flat_map(|f| {
            controls_in(f)
                .into_iter()
                .map(move |(line, c)| (*f, line, c))
        })
        .collect()
}

#[test]
fn every_shipped_pam_control_is_valid() {
    let controls = all_controls();

    // Vacuity guard. If extraction silently found nothing, every assertion
    // below would pass over an empty set and this test would be decorative.
    assert!(
        controls.len() >= DECLARING_FILES.len(),
        "found only {} PAM control declarations across {} packaging files — the extractor \
         is broken, not the packaging. A pass here would mean nothing.\nfound: {controls:#?}",
        controls.len(),
        DECLARING_FILES.len()
    );

    let mut problems = Vec::new();
    for (file, line, control) in &controls {
        let pairs = parse_control(control);
        if pairs.is_empty() {
            problems.push(format!(
                "{file}:{line}: `{control}` parses to no token=action pairs"
            ));
            continue;
        }
        for (tok, action) in pairs {
            if !VALID_TOKENS.contains(&tok.as_str()) {
                problems.push(format!(
                    "{file}:{line}: `{tok}` is not a PAM return code (in `{control}`)"
                ));
            }
            // An action is a keyword or a non-negative jump count.
            let action_ok =
                VALID_ACTIONS.contains(&action.as_str()) || action.parse::<u32>().is_ok();
            if !action_ok {
                problems.push(format!(
                    "{file}:{line}: `{action}` is not a valid PAM action (in `{control}`). \
                     libpam does NOT reject an unknown action — it silently treats it as \
                     `ignore`, which makes face auth a no-op that looks like it works. \
                     This is exactly the `success=end` bug that shipped from v0.1.0 to v0.3.2."
                ));
            }
        }
    }

    assert!(
        problems.is_empty(),
        "invalid PAM control(s):\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn every_shipped_pam_control_is_identical() {
    let controls = all_controls();
    assert!(
        controls.len() >= DECLARING_FILES.len(),
        "extractor found too few controls to compare — see every_shipped_pam_control_is_valid"
    );

    let (first_file, first_line, first) = &controls[0];
    let divergent: Vec<_> = controls
        .iter()
        .filter(|(_, _, c)| c != first)
        .map(|(f, l, c)| format!("{f}:{l}: `{c}`"))
        .collect();

    assert!(
        divergent.is_empty(),
        "packaging formats disagree on the PAM control. Debian, Arch and NixOS users would \
         get different authentication behaviour from the same release, and there is no shared \
         constant to keep them in step.\n  baseline {first_file}:{first_line}: `{first}`\n  {}",
        divergent.join("\n  ")
    );
}

/// Proves the validator would actually reject the historic bug, rather than
/// passing because it accepts everything.
#[test]
fn the_validator_rejects_the_success_end_bug() {
    let bad = parse_control("[success=end default=ignore]");
    let end_action_rejected = bad.iter().any(|(_, a)| {
        a == "end" && !VALID_ACTIONS.contains(&a.as_str()) && a.parse::<u32>().is_err()
    });
    assert!(
        end_action_rejected,
        "the validator accepts `success=end` — it would not have caught the v0.1.0–v0.3.2 bug, \
         so it is not doing the job this file exists for"
    );

    // And it must still accept what we actually ship, or it is merely strict.
    let good = parse_control("[success=done default=ignore]");
    assert!(
        good.iter().all(|(t, a)| VALID_TOKENS.contains(&t.as_str())
            && (VALID_ACTIONS.contains(&a.as_str()) || a.parse::<u32>().is_ok())),
        "the validator rejects the control we ship — it is too strict to be useful"
    );
}

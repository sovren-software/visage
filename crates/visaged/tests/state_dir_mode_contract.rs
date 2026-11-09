//! Contract: every packaging path creates `/var/lib/visage` root-only.
//!
//! `faces.db` lives there. Its embeddings are AES-GCM encrypted under a 0600
//! key, but usernames, labels and enrollment times are stored in clear, so a
//! world-readable state directory tells every local user who enrolled what.
//!
//! Found 2026-11-09. Debian and NixOS were right. Two paths were not:
//! - **AUR**: `install -dm700 "$pkgdir/var/lib/visage/models"`. GNU `install`
//!   applies `-m` to the last component only and creates the parents 0755, so
//!   the package shipped `/var/lib/visage` as 0755. The line reads as if it
//!   were correct, which is why it survived review.
//! - **RPM**: nothing created the directory, so its mode came from whichever
//!   root process made it first, under umask 022.
//!
//! Four packaging formats with no shared source cannot be kept in step by
//! review. This test reads each one.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/visaged should be two levels below the repo root")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// Lines that are not comments.
fn code_lines(body: &str) -> Vec<&str> {
    body.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .collect()
}

#[test]
fn debian_postinst_makes_the_state_dir_0700() {
    let body = read("packaging/debian/postinst");
    assert!(
        code_lines(&body)
            .iter()
            .any(|l| *l == "chmod 700 /var/lib/visage"),
        "packaging/debian/postinst must `chmod 700 /var/lib/visage`"
    );
}

#[test]
fn nixos_module_makes_the_state_dir_0700() {
    let body = read("packaging/nix/module.nix");
    assert!(
        body.contains("\"d /var/lib/visage 0700 root root -\""),
        "packaging/nix/module.nix must declare a 0700 tmpfiles rule for /var/lib/visage"
    );
}

#[test]
fn aur_package_makes_the_state_dir_itself_0700() {
    let body = read("packaging/aur/PKGBUILD");
    // The state directory must be named as the target, not only as a parent.
    assert!(
        code_lines(&body)
            .iter()
            .any(|l| *l == "install -dm700 \"$pkgdir/var/lib/visage\""),
        "packaging/aur/PKGBUILD must `install -dm700 \"$pkgdir/var/lib/visage\"` on its own \
         line; `install -dm700 .../visage/models` leaves /var/lib/visage 0755"
    );
    let hooks = read("packaging/aur/visage.install");
    assert!(
        hooks.contains("chmod 700 /var/lib/visage"),
        "packaging/aur/visage.install must repair the mode on upgrade"
    );
}

#[test]
fn rpm_post_install_makes_the_state_dir_0700() {
    let body = read("crates/visaged/Cargo.toml");
    let line = body
        .lines()
        .find(|l| l.trim_start().starts_with("post_install_script"))
        .expect("crates/visaged/Cargo.toml must set an RPM post_install_script");
    assert!(
        line.contains("install -d -m 0700 /var/lib/visage")
            && line.contains("chmod 0700 /var/lib/visage"),
        "the RPM post_install_script must create /var/lib/visage 0700 and repair an \
         existing one, got: {line}"
    );
}

#[test]
fn the_shared_unit_asks_systemd_for_a_0700_state_dir() {
    let body = read("packaging/systemd/visaged.service");
    let lines = code_lines(&body);
    assert!(
        lines.contains(&"StateDirectory=visage"),
        "missing StateDirectory=visage"
    );
    assert!(
        lines.contains(&"StateDirectoryMode=0700"),
        "missing StateDirectoryMode=0700"
    );
}

/// Control: the AUR check rejects the exact line that shipped the bug.
#[test]
fn the_aur_check_rejects_the_models_only_line() {
    let buggy = "install -dm700 \"$pkgdir/var/lib/visage/models\"\n";
    assert!(!code_lines(buggy)
        .iter()
        .any(|l| *l == "install -dm700 \"$pkgdir/var/lib/visage\""));
}

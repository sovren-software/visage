# Fedora Install Guide

Tested on **Fedora 44 (x86_64), GNOME/Wayland, ASUS Zenbook 14 UM3406KA**
(camera `3277:0055`, IR node `/dev/video2`). The quick path is one script;
PAM wiring stays manual on purpose (see below).

## Quickstart

```bash
sudo dnf install pam-devel clang-devel
./scripts/install-fedora.sh
```

The script is idempotent — safe to re-run after a failure or upgrade. Flags:

| Flag | Effect |
|------|--------|
| `--no-build` | Skip `cargo build`, install from existing `target/release` |
| `--no-enroll` | Skip the onboard reminder at the end (headless/CI) |

It installs `visaged`, `visage`, `visage-enroll` to `/usr/local/bin`, the PAM
module to `/usr/lib64/security/pam_visage.so`, the D-Bus system policy, the
`visage-has-session` GNOME helper, both systemd units, fixes SELinux labels
(`restorecon`), downloads the ONNX models (~182 MB), and starts the daemon.

Verify:

```bash
sudo /usr/local/bin/visage status
sudo /usr/local/bin/visage discover   # expect 3277:0055, no quirk
```

## Wire PAM (manual — do not skip reading this)

Fedora has no `pam-auth-update`, and **authselect owns `system-auth` /
`password-auth` and overwrites hand edits there on the next apply**. So never
put `pam_visage` in `system-auth`. Wire each service individually instead,
starting with `sudo` only. Keep a root shell (`sudo -i`) open in a second
terminal until the new stack is proven.

```bash
sudo cp /etc/pam.d/sudo /etc/pam.d/sudo.bak-visage
```

Add as **line 2** of `/etc/pam.d/sudo` (right after `#%PAM-1.0`):

```text
auth [success=done default=ignore] pam_visage.so
```

Result:

```text
#%PAM-1.0
auth [success=done default=ignore] pam_visage.so
auth       include      system-auth
account    include      system-auth
password   include      system-auth
session    optional     pam_keyinit.so revoke
session    required     pam_limits.so
session    include      system-auth
```

Test:

```bash
sudo -k && sudo true
```

Face match proceeds immediately; anything else falls through to the password
prompt (`PAM_IGNORE`, never lockout). If `sudo` breaks completely, recover
from the spare root shell:

```bash
cp /etc/pam.d/sudo.bak-visage /etc/pam.d/sudo
```

Or without any root shell via polkit (bypasses the sudo stack):

```bash
pkexec bash
cp /etc/pam.d/sudo.bak-visage /etc/pam.d/sudo
```

## Enroll

```bash
sudo /usr/local/bin/visage onboard
sudo -k && sudo true
```

`onboard` downloads models if missing, captures four labelled angles, and
verifies against the daemon before reporting success. It refuses `root` as a
target (face auth bound to root by accident is the exact mistake it prevents).

## GNOME lock screen / login

GNOME uses one PAM service, `gdm-password`, for first login *and* unlock, and
a face cannot unlock the GNOME keyring (that needs the login password). So:

- **Lock screen:** add the same `pam_visage.so` line as line 2 of
  `/etc/pam.d/gdm-password`, but gate it with `visage-has-session` so the
  first login after boot still asks for the password (unlocks the keyring)
  while unlock uses face. The helper is already installed at
  `/usr/local/libexec/visage-has-session` — see
  [`contrib/pam/README.md`](../contrib/pam/README.md) for the two-line stanza.
- **First login:** keep password-only. Do not chase face-login on GNOME; the
  keyring prompt right after defeats the point.

With `authselect ... with-fingerprint`, keep `pam_visage` in `gdm-password`
only — adding it to the fingerprint stack runs two camera verifies against
one device.

## Suspend / resume

`visage-resume.service` restarts the daemon after suspend/hibernate (stale
camera fd otherwise). Enabled by the script. Confirm:

```bash
systemctl status visage-resume.service
journalctl -u visaged --since "5 minutes ago"
```

## Hardware notes (3277:0055)

- IR node is `/dev/video2` (GREY 640×360). The other `/dev/videoN` nodes are
  the same physical camera (RGB / metadata).
- `visage discover` reports `no quirk` — expected. The emitter **strobes by
  firmware default** anyway (lit/unlit alternating every frame), so auth works
  without a quirk. `visage test -n 10` showing ~half dark-skipped frames is
  that strobe, not a defect.
- The daemon default `VISAGE_LIVENESS_MIN_DISPLACEMENT=0.8` rejects ~1 in 6
  genuine logins on this module. The shipped systemd unit already sets `0.1`
  (measured in
  [`hardware-reports/asus-zenbook-um3406ha-3277-0055.md`](hardware-reports/asus-zenbook-um3406ha-3277-0055.md)).
  No measured anti-spoof mechanism exists for this module — the password
  fallback is the control.
- Deriving emitter control bytes for a future `contrib/hw/3277-0055.toml` is
  open work (see the hardware report).

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| `Unable to find libclang` at build | `sudo dnf install clang-devel` |
| `cannot stat ... pam_visage.so` / link error | `sudo dnf install pam-devel` |
| `visage setup` → `Connection reset by peer` | transient HuggingFace reset — re-run `sudo visage setup` (2–3 tries typical) |
| `visaged: not reachable ... ServiceUnknown` | daemon not running: `systemctl status visaged`, `journalctl -u visaged -n 20` (usually missing models — run setup first) |
| `sudo` still asks password | `grep pam_visage /etc/pam.d/sudo`, `systemctl is-active visaged`, `sudo visage list` (empty = enrolled as wrong user — use `onboard`, not bare `enroll` under sudo) |
| SELinux denials after install | `sudo restorecon -v /usr/local/bin/visage* /usr/lib64/security/pam_visage.so`; `sudo ausearch -m avc -ts recent` |

## Uninstall

```bash
sudo systemctl disable --now visaged.service visage-resume.service
sudo rm /usr/local/bin/visaged /usr/local/bin/visage /usr/local/bin/visage-enroll \
  /usr/lib64/security/pam_visage.so /usr/local/libexec/visage-has-session \
  /usr/share/dbus-1/system.d/org.freedesktop.Visage1.conf \
  /etc/systemd/system/visaged.service /etc/systemd/system/visage-resume.service
sudo cp /etc/pam.d/sudo.bak-visage /etc/pam.d/sudo   # remove the pam_visage line
sudo systemctl daemon-reload
# optional: sudo rm -rf /var/lib/visage   # models (~182 MB) + face database
```

# PAM helpers

## `visage-has-session` — password at first login, face for unlock (GNOME)

A face cannot unlock the GNOME keyring; that needs the login password. With `pam_visage`
`sufficient` in `gdm-password`, the first login after boot succeeds by face and the keyring
immediately asks for the password anyway.

GNOME uses the same PAM service, `gdm-password`, for the login screen and the lock screen,
so PAM cannot tell them apart by name. What does differ is that at unlock the user already
owns a login session. This helper checks that via `loginctl`: exit 0 when `$PAM_USER` has a
session of class `user` (unlock, run face auth), exit 1 otherwise (first login, use the
password). Desktops whose lock screen uses its own PAM service do not need it: put
`pam_visage` on the unlock service only. Only GNOME has been tested.

### Install

```bash
sudo install -D -m 755 contrib/pam/visage-has-session /usr/local/libexec/visage-has-session
```

In `/etc/pam.d/gdm-password`, replace the `pam_visage` line with:

```text
auth        [success=ignore default=1]  pam_exec.so quiet /usr/local/libexec/visage-has-session
auth        sufficient    pam_visage.so
```

On success the helper contributes nothing and PAM continues into `pam_visage`; otherwise PAM
skips one module and lands on the password stack. Leave `sudo`, `polkit-1` and other
non-login services as plain `auth sufficient pam_visage.so`.

### Test

```bash
PAM_USER=$USER /usr/local/libexec/visage-has-session; echo $?   # 0 while logged in
PAM_USER=nobody /usr/local/libexec/visage-has-session; echo $?  # 1
```

Then lock the screen and unlock by face; after the next reboot the login screen should ask
for the password.

### Fingerprint readers

With `authselect … with-fingerprint`, GDM runs `gdm-password` and `gdm-fingerprint`
concurrently. Keep `pam_visage` in `gdm-password` only; adding it to the fingerprint stack
would run two camera verifies against one device. A fingerprint touch at first login still
bypasses the keyring unlock, as it always has.

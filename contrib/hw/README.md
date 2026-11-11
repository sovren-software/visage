# Hardware Quirks Database

Camera-specific UVC control bytes for IR emitter activation.

## Format

Each file is a TOML entry named `{vendor_id}-{product_id}.toml` (lowercase hex, no `0x` prefix).

### Single-control cameras

For cameras requiring a single UVC extension unit command:

```toml
[device]
vendor_id  = 0x04F2
product_id = 0xB6D9
name       = "ASUS Zenbook 14 UM3406HA IR Camera"

[emitter]
unit          = 14
selector      = 6
control_bytes = [1, 3, 3, 0, 0, 0, 0, 0, 0]
```

### Multi-control cameras

For cameras requiring a sequence of controls (for example, enabling emitter mode then setting LED power/brightness level), use `[[emitter.controls]]`:

```toml
[device]
vendor_id  = 0x5986
product_id = 0x2174
name       = "Lenovo IdeaPad Pro 5 14IAH10 IR Camera (Bison 5986:2174)"

[emitter]
[[emitter.controls]]
unit          = 7
selector      = 6
control_bytes = [1, 3, 2, 0, 0, 0, 0, 0, 0]
off_bytes     = [1, 3, 1, 0, 0, 0, 0, 0, 0]

[[emitter.controls]]
unit          = 5
selector      = 2
control_bytes = [100]
```

Inline syntax (`controls = [{ unit = ..., selector = ..., control_bytes = [...] }, ...]`) is also accepted by Serde.

A quirk file must specify either the single-control fields or `controls`, never both. Mixing both forms is rejected. `reset_on_close` is set on `[emitter]`.

**Field reference:**

| Section | Field | Type | Description |
|---------|-------|------|-------------|
| `[device]` | `vendor_id` | hex int | USB idVendor (from `lsusb` or `visage discover`) |
| `[device]` | `product_id` | hex int | USB idProduct |
| `[device]` | `name` | string | Human-readable camera name |
| `[emitter]` | `reset_on_close` | bool | Optional. Set `true` for cameras that reset the control when the controlling fd closes and only re-illuminate on a fresh open→set edge; the emitter then holds one fd open for the duration of each capture. Defaults to `false`. |
| `[emitter]` or `[[emitter.controls]]` | `unit` | u8 | UVC extension unit ID |
| `[emitter]` or `[[emitter.controls]]` | `selector` | u8 | UVC control selector |
| `[emitter]` or `[[emitter.controls]]` | `control_bytes` | byte array | Payload to activate the emitter. Zeros of the same length deactivate it unless `off_bytes` is provided. |
| `[emitter]` or `[[emitter.controls]]` | `off_bytes` | byte array | Optional. Explicit payload to deactivate the emitter. Needed for cameras that reject an all-zero "off" payload (e.g. with `ERANGE`). Defaults to zeros of `control_bytes` length when omitted. |

The `control_bytes` values are found via `linux-enable-ir-emitter configure` or UVC descriptor analysis.

## Contributing

1. Run `visage discover` to detect your camera's VID:PID and check for existing quirk support
2. If no quirk exists, use `linux-enable-ir-emitter configure` to find the control bytes
3. Create a TOML file named `{vid}-{pid}.toml` (e.g. `04f2-b6d9.toml`) following the format above
4. **Register it in `crates/visage-hw/src/quirks.rs`** — two lines, and the step that is easy
   to miss:

   ```rust
   const QUIRK_04F2_B6D0: &str = include_str!("../../../contrib/hw/04f2-b6d0.toml");
   //  …then add it to QUIRK_SOURCES:
   ("04f2-b6d0.toml", QUIRK_04F2_B6D0),
   ```

5. **Verify the quirk is doing the work, not another tool.** Disable any external emitter
   activation tool (`linux-enable-ir-emitter` and similar) and **power-cycle the laptop or the
   camera** to clear residual control bytes, then test. A camera left illuminated by something
   else will make a non-working quirk look correct, and the contribution ships inert.

   This is a real negative control, and it is what made [#76](https://github.com/sovren-software/visage/pull/76)
   trustworthy — @rampa3 did exactly this before submitting, then suggested documenting it
   ([#85](https://github.com/sovren-software/visage/issues/85)).

6. Run `cargo test -p visage-hw quirks::` and submit a PR

Quirk files are embedded at compile time via `include_str!` — there is no runtime file loading,
so **dropping a `.toml` into this directory does nothing on its own.** A file that is not
registered in `QUIRK_SOURCES` is never read.

### Why step 4 has tests behind it

`quirk_db()` skips a malformed TOML rather than panicking — the daemon authenticates logins, and
one bad contributed quirk must not stop it starting. The cost is that at runtime a camera with a
broken quirk is **indistinguishable from a camera with no quirk**: no error, no crash, the
emitter simply never fires.

`crates/visage-hw/src/quirks.rs` therefore asserts, in CI:

| Test | Catches |
|---|---|
| `quirk_sources_all_parse` | an entry that is embedded but malformed, and was silently dropped |
| `every_quirk_is_reachable_by_lookup` | an entry that parses but `lookup_quirk` cannot find |
| `no_duplicate_vid_pid` | two entries claiming one device, where listing order silently decides |
| `emitter_payloads_are_coherent` | empty `control_bytes`, or `off_bytes` of a mismatched length |

The parse count is compared against `QUIRK_SOURCES.len()` rather than a hard-coded number, so
the assertion cannot drift as cameras are added.

# Hardware Quirks Database

Camera-specific UVC control bytes for IR emitter activation.

## Format

Each file is a TOML entry named `{vendor_id}-{product_id}.toml` (lowercase hex, no `0x` prefix):

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

**Field reference:**

| Section | Field | Type | Description |
|---------|-------|------|-------------|
| `[device]` | `vendor_id` | hex int | USB idVendor (from `lsusb` or `visage discover`) |
| `[device]` | `product_id` | hex int | USB idProduct |
| `[device]` | `name` | string | Human-readable camera name |
| `[emitter]` | `unit` | u8 | UVC extension unit ID |
| `[emitter]` | `selector` | u8 | UVC control selector |
| `[emitter]` | `control_bytes` | byte array | Payload to activate the emitter. Zeros of the same length deactivate it. |

The `control_bytes` values are found via `linux-enable-ir-emitter configure` or UVC descriptor analysis.

## Contributing

1. Run `visage discover` to detect your camera's VID:PID and check for existing quirk support
2. If no quirk exists, use `linux-enable-ir-emitter configure` to find the control bytes
3. Create a TOML file named `{vid}-{pid}.toml` (e.g. `04f2-b6d9.toml`) following the format above
4. Submit a PR

The quirk file is embedded at compile time via `include_str!` — no runtime file loading required.

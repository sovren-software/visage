# Hardware Quirks Database

Camera-specific UVC control bytes for IR emitter activation.

## Format

Each file is a TOML entry:

```toml
[camera]
vendor_id = 0x04F2
product_id = 0xB6D9
name = "ASUS Zenbook 14 UM3406HA IR Camera"
device = "/dev/video2"

[emitter]
unit = 14
selector = 6
control = [1, 3, 3, 0, 0, 0, 0, 0, 0]
```

## Contributing

1. Run `visage test --discover` to detect your IR camera
2. If auto-detection fails, use `linux-enable-ir-emitter configure` to find control bytes
3. Create a TOML file named `{vendor_id}-{product_id}.toml`
4. Submit a PR

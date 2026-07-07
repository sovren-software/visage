## What problem does this solve?

<!-- The motivation, not the diff — what's broken, missing, or needed, and for whom.
     Link the issue if there is one ("Closes #123"). No clear problem yet? Open an
     issue first (see CONTRIBUTING.md). -->

## Type

<!-- Check one: -->
- [ ] Hardware quirk (`contrib/hw/*.toml`)
- [ ] Bug fix
- [ ] Distribution packaging
- [ ] Documentation
- [ ] Other: <!-- describe -->

## What this changes

<!-- How this PR solves the problem above — the approach, not a line-by-line list. -->

## Testing

<!-- How you verified it works:
     - Quirks: paste `visage discover` output showing the device
     - Bug fixes: the repro, and how this fixes it
     - Packaging: which distro/version you tested on -->

## Checklist

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] Commits are signed off (`git commit -s`) per the DCO
- [ ] I have read [CONTRIBUTING.md](../CONTRIBUTING.md)

## Breaking changes

<!-- Does this change any public API, configuration, CLI behavior, or file format?
     If yes, describe what breaks and how users should migrate. If no, delete this section. -->

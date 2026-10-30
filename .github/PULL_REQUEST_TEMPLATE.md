## What does this change?

<!-- A short description and the motivation. Link the issue: Fixes #123 -->

## How was it tested?

<!-- Commands you ran, devices and platforms you tried. -->

## Checklist

- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass
- [ ] New and changed items are documented; user-facing text is translated (`locales/app.yml`, GUI `I18N`)
- [ ] Protocol changes follow `docs/PROTOCOL.md` (ALPN bumped for incompatible changes)
- [ ] `CHANGELOG.md` has an entry under *Unreleased*

#!/bin/sh
# Cargo runner for `tauri build --runner` (see .github/workflows/release.yml):
# Tauri calls `<runner> build …`, which becomes `cargo auditable build …`, so
# the released GUI binary carries its dependency list (crate names and
# versions) for `cargo audit bin` and SBOM tools. Needs `cargo install cargo-auditable`.
exec cargo auditable "$@"

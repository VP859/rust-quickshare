@rem Cargo runner for `tauri build --runner` on Windows: `cargo auditable build ...`
@rem (see scripts/cargo-auditable.sh and .github/workflows/release.yml).
@cargo auditable %*

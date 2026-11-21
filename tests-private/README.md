# Portal tests

Unit tests for the Portal crates. Each source file under `crates/*/src` loads
its tests from the matching file here with a `#[path]` attribute, so
`cargo test --workspace` runs them.

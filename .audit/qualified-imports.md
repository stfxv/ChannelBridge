# Qualified imports coverage

This document records the import qualification audit of 2026-10-26. The validation results and syntax comparisons below apply to the audit revisions named here. They are historical evidence, not a new verification of the current checkout. The sibling work files named at the end are local audit artifacts, not tracked repository files or source-distribution contents. Current checks are documented in [RUST_STYLE.md](../RUST_STYLE.md).

This change applies these supplied Google Rust guide rules.

- `#qualify-calls-to-top-level-functions-with-a-module-name`.
- `#type-names-should-be-imported-and-used-unqualified`.
- `#prefer-locally-scoped-trait-imports`.

Rust free functions were identified from declarations and import paths. The reviewed mappings cover `finite`, `stereo_queue`, `meter_bank`, `fourcc`, `size_of`, `catch_unwind`, `black_box`, `stereo_output`, `dbfs`, `guard`, and `valid`. References passed to combinators and expressions inside standard assertion macros receive the same qualification as calls. Inline child modules that import `super::*` now call actual parent `ItemFn` declarations through `super::`.

The standalone `qualify_functions` helper uses the existing syn dependency. It runs only when explicitly invoked. It does not infer that lowercase identifiers are functions. It changes parsed expression paths, reviewed type prefixes, and standard expression-list macro arguments. Other macro token languages remain opaque. It rejects matching local bindings or function declarations before writing. Attributed and public function imports, or imports containing comments, require manual review. A module name or alias supplied in a mapping must be reviewed for collisions. The helper is a migration tool, not a name-resolution lint.

Examples run through the isolated Rust wrapper are equivalent to these commands.

```sh
cargo run --locked --manifest-path tools/rust-style/Cargo.toml --bin qualify_functions -- crates/audio/src/resample.rs finite=frame
cargo run --locked --manifest-path tools/rust-style/Cargo.toml --bin qualify_functions -- crates/audio/src/fifo.rs --parent-functions
cargo run --locked --manifest-path tools/rust-style/Cargo.toml --bin qualify_functions -- SOURCE.rs std::cell::Cell=Cell
```

The final form retains these justified cases.

- Disambiguated C and Core Foundation function names remain direct calls. Objective-C `sel!`, `msg_send!`, `ns_string!`, and `define_class!` remain macros.
- `pub use application::run` is an API reexport. The application entry point calls `native::run` with a module qualifier. Changing the reexport would change the API rather than a call's spelling.
- `io::Error`, `io::Result`, and `fmt::Result` distinguish common names from crate error types and the prelude result type. `std::error::Error` stays qualified in modules importing the concrete `devices::Error`.
- Cohesive view, ABI, and Objective-C type groups can retain module qualification under the guide's group and large-dependency exceptions. Repeated ordinary core, time, cell, path, and error-trait types were imported where no name collision exists.
- Traits used across implementations and several functions remain at module scope. Examples include `GlobalAlloc`, `Write` in diagnostics, and Objective-C allocation traits. The sole storage `Write` import moved into `save`. Existing local Unix extension-trait imports remain local.

Validation uses separate targets for this worktree. The normal and production workspace all-target tests pass. Normal and production workspace all-target Clippy pass with `-D warnings`. The standalone helper tests and Clippy pass. The style script passes and still enforces import granularity. The two Loom tests pass with the `model_tests` filter. An unfiltered run with `model-check` also executes ordinary capture tests outside a Loom model and fails there, so it is not the supported model-test invocation.

A temporary syn comparison checked all 37 changed pre-existing Rust files in the isolated qualification branch against its baseline `66eae69`. After removing use items and path prefixes, and normalizing rustfmt's single-expression blocks and optional commas, their parsed token structures match. Literals, leaf names, and other operators are unchanged. An independent reviewer repeated that comparison for the integrated qualification-only delta `064612f..78a3bb7`; all 37 files also matched. Those checks cover the syntax migration. They do not assert token equivalence for the separate `must_use` annotations, explicit report discards, or worker lifecycle fixes. Compiler checks, final tests, and independent review cover the combined changes. The comparison source and original logs are in the sibling work directory under `rust-style-qualified-proof.rs` and `rust-style-qualified-token-proof.log`.

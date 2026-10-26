Run `scripts/check-rust-style.sh check` from a checkout with stable Rust installed.
The check scans every tracked Rust file under `crates`, including tests and examples,
then runs stable rustfmt on the workspace and this helper. Individual source paths
can be supplied after the mode. Missing inputs and parse errors fail with status 2;
style findings fail with status 1. `inventory` prints findings without failing.

`fix-imports` splits nested use groups into one statement per imported item. It
preserves visibility, leading `::`, `self`, aliases and globs. It intentionally
leaves attributed imports and imports containing comments for manual review;
`check` continues to report them. Run workspace rustfmt after the migration.
The edit is idempotent and never rewrites crate source unless explicitly invoked.

The `import-group` finding enforces the guide's requirement that no `::` occurs
inside a use group. Multiple statements importing from the same module are valid.
This does not resolve traits, functions, C FFI symbols or macros and therefore
does not attempt to qualify imported function calls. It does not impose a blanket
glob ban because the guide permits contextual exceptions.

The `unwrap` finding scans token trees recursively, including macro input, while
ignoring strings, comments and docstrings. It inventories the exact token sequence
`.unwrap()`; it cannot infer whether arbitrary macro input executes as Rust.
It deliberately does not invent `expect` reasons. Each reason needs the actual
invariant or prerequisite that should have succeeded. Qualified associated calls
such as `Option::unwrap(value)` and generated macro expansion require Clippy or
review. No checker here guesses the receiver type of `into_inner`, the return type
of `new`, or which types can implement `Copy` or should carry `must_use`.

Use compiler-backed Clippy checks for those semantic questions. The corresponding
built-in lints include `unwrap_used`, `new_ret_no_self`, `must_use_candidate`, and
`missing_const_for_fn`; `missing_copy_implementations` is a rustc lint. Review
findings against the guide before enabling them workspace-wide. A `new` returning
`Self` wrapped in `Result` needs separate API review because `new_ret_no_self`
does not express the guide's `try_new` naming rule exactly.

`poison-into-inner` reports explicit expression paths ending in
`PoisonError::into_inner`. It deliberately ignores unrelated methods named
`into_inner`. Aliases, method receivers, UFCS types and macro expansion still
need type-aware review. The spelling can itself be shadowed by a local type,
so this finding requires review before changing behavior.

Run the behavior fixtures with `cargo test --locked --manifest-path
tools/rust-style/Cargo.toml`. The standalone manifest and lockfile isolate parser
dependencies from every production crate. No nightly rustfmt setting is required.

The `fixture_expects` binary inventories empty `.unwrap()` and `.unwrap_err()`
method calls, including calls inside macro token trees. Comments and string
literals are opaque. Run `cargo run --bin fixture_expects -- SOURCE.rs` to print
one-based line, zero-based column, and method as tab-separated fields. To apply
reviewed explanations, pass a second file whose rows append a fourth field with
the causal panic explanation. The tool validates every token location before
writing and rejects duplicate locations or empty explanations. Recreate the map
after any source edits; positions refer to the exact input source. Whitespace
between method names and parentheses requires a manual edit.

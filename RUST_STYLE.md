# Rust style policy

ChannelBridge uses the [Google Rust Style Guide](https://google.github.io/styleguide/rustguide.html)
as its Rust style policy. The policy covers every first-party Rust source, test,
example, and the style tool.
Dependencies and generated vendor sources are outside that scope.

## Application context

The seven crates form one macOS application and its HAL driver. They are not
independently published libraries. API recommendations are applied to their real
workspace callers, as required by the guide's
[application context](https://google.github.io/styleguide/rustguide.html#apply-api-design-rules-in-context).
Chromium build layout, Chromium task sequences, Fuchsia mutexes, and their test
frameworks do not apply to this Cargo project.

Substantial implementations live in leaf modules. Module indexes declare and
re-export their children. Root type exports remain available, while leaf names
identify a primary abstraction when one exists. Collections such as ABI layouts,
buffer views, configuration models, and meter endpoints remain coherent groups.
`Result` constructors use `try_new`; all callers migrate together.

## Automated checks

Run the normal verification entry point.

```sh
bash scripts/test.sh
```

It runs the style gate, a typed poison-policy positive/negative check, and the
existing audio, lifecycle, concurrency, ABI, and sanitized driver tests.
The standalone style gate and its behavior fixtures can run independently.

```sh
bash scripts/check-rust-style.sh check
cargo test --locked --manifest-path tools/rust-style/Cargo.toml
```

The syntax checker scans source files directly, including untracked files. It
also works in the extracted source distribution without Git. Rustfmt uses stable
options and a 100-column target. Nested import groups are checked by the syntax
tool because stable rustfmt cannot configure the guide's import granularity.
`fix-imports` is opt-in. It preserves aliases and visibility, and leaves imports
with comments or attributes unchanged for manual review.

The standalone gate runs the syntax checker and rustfmt. The normal verification
entry point also runs Clippy. All application crates inherit workspace Clippy
lints that reject `.unwrap()` calls, undocumented unsafe blocks, missing unsafe
API contracts, wildcard imports without an applicable exception, and selected
missing return-value annotations. A typed
`disallowed_methods` policy rejects `PoisonError::into_inner`, `get_ref`, and
`get_mut`, including calls through aliases. The negative fixture proves that
semantic check rather than relying on method-name matching.

Compiler heuristics do not establish full guide conformance. In particular,
private pure functions and some critical returned statuses need manual
`must_use` review. `Option` itself does not diagnose an ignored return in the
supported compiler; applicable constructors and queries carry their own attribute.

## Policy coverage and contextual choices

| Guide area | Application policy and checks |
| --- | --- |
| Public Rust style and API context | Stable rustfmt, Clippy, application-specific contracts, and the existing Cargo test framework. |
| Code organization | Leaf implementations and primary-type file names; root declarations and re-exports; seven responsibility-focused crates. Chromium's flat layout and tiny-crate build rules are not applied. |
| Imports | One module per use group; explicit imports where globs are not permitted; Rust free functions called through modules; types imported directly unless ambiguity or a large ABI/type family makes qualification clearer. Inline `super::*` test imports and root re-exports use the guide's exceptions. Native C symbols and Objective-C macros retain their required spelling. |
| Formatting | Stable 100-column rustfmt policy. No unstable compiler features or unsupported formatting options are introduced. Macro input and long literals are still reviewed where rustfmt cannot rewrite them. |
| Ownership and borrowing | Shared callback, notification, endpoint, and snapshot lifetimes justify `Arc`; deep copies cross actual ownership boundaries. Existing slices, strings, and borrowed payloads remain direct. |
| Inputs and outputs | Slices and owned inventories fit the application access patterns. No speculative `Into` or iterator-generic APIs are added. Named values make boolean purposes clear; boolean persistence fields retain their wire format. |
| Errors | Recoverable platform failures retain `Result` context. Small copyable errors implement `Copy`. Existing `Option` constructors document rejection and reservation conditions where callers reject the prepared configuration as one unit. Callback-time absence remains `Option`. |
| Types and traits | ABI aliases preserve native layout; semantic frame positions remain a newtype. Traits serve real generic backends/handlers. Defaults remain derived where equivalent; configuration keeps its nonzero channel defaults. Local endpoint tuples are immediately destructured. |
| Concurrency | Poisoned guards are never inspected or repaired. Admission refuses further work, snapshot failure reports a blocked fallback, and completion is independent of the poisoned cells. Tests verify backend destruction precedes completion. |
| Macros | Macros remain limited to platform binding, tests, and repeated ABI definitions. Token tools preserve macro input and do not infer symbol identity from capitalization. |
| Naming | Fallible constructor names, primary module names, descriptive borrowed-view lifetimes, and the existing domain terms. HAL, ABI, PCM, FIFO, CF, and SDK are established platform terms. |
| Documentation | Public contracts explain actual bounds, ownership, failure, and panic conditions. Trivial names and self-explanatory enum variants do not gain repetitive comments. |
| Unsafe | Safe audio/control interiors remain separated from native bindings. Local `SAFETY` reasoning covers validity, extent, lifetime, exclusivity, and ABI obligations. Header packing uses safe native-endian operations. |
| API evolution | Workspace callers migrate together. Blanket `non_exhaustive`, generic wrappers, and redundant constructor APIs are not introduced for hypothetical external clients. |
| Return values | Pure queries, critical `Option` results, partial-transfer reports, and consuming builders require explicit use. Intentional warmups and already-accounted HAL transfers explicitly discard reports. Idempotent side-effect operations may expose optional diagnostics. |
| Implementation clarity | Borrowed views, named iteration pipelines, pattern matching, explicit fixture expectations, standard format strings, and contextual assertions. |
| Initialization and constants | No code runs before main; fixed ABI constants keep their intended semantics. Public const functions are not added speculatively. |
| Test framework | Cargo tests, AppKit fake-backend controls, Loom, allocator instrumentation, the SDK C oracle, and the sanitized fake HAL remain the project framework. |

## Native boundary exceptions

The guide permits defensive `catch_unwind` when unwind support exists and panics
are already avoided in normal processing. Four native entry guards use that
exception. Buffer validation, exclusive access, and ownership safety do not depend
on catching a panic. Failure flags prohibit reuse of partially processed state.
Invalid foreign pointers, aborts, and foreign exceptions are not repaired by a
Rust unwind guard. Ordinary errors use explicit results or native status codes.

The supported ABI requires unsafe calls and exact C representations. Tests compare
those representations against the Apple SDK and exercise the actual Rust factory
through a C harness. Those tests do not prove arbitrary foreign pointer validity.
The local safety contracts remain caller obligations.

## Behavioral contract

Poisoned state refuses reuse. Worker completion waits until backend owners are gone
and reports panic even if a final snapshot had already been published. A poisoned
snapshot uses initial display metadata and unavailable telemetry, never recovered
contents. An operation already running can finish before shutdown is observed.

Source distributions include the style configuration, helper sources, locked
parser dependencies through the vendored source set, and style scripts. The
archive verifier rebuilds both workspaces and runs the style gate with an empty
Cargo cache and offline networking.

Physical sleep/reconnect, receiving applications, long calls, and Intel execution
remain separate hardware acceptance checks.

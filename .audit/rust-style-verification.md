# Rust style audit verification

This is the 2026-10-26 audit record. The source revisions, command results, GitHub Actions status, and packaging warnings below describe that audit, not the current checkout or a new test run. The evidence bundle is a local audit artifact and is not tracked in this repository or included in its source distribution. Current verification commands are documented in [RUST_STYLE.md](../RUST_STYLE.md).

The baseline is `945c1ec`. The final application and tooling source is `add8911`.
Subsequent changes contain these audit records only. The captured guide URL and
SHA-256 are recorded in the evidence bundle's `source.json`. The guide was fetched
on 2026-10-26. `RUST_STYLE.md` records rule coverage and contextual application.

The following commands completed with exit status 0 on the final source:

- `bash scripts/test.sh` ran the style and typed poison gates, 13 helper tests,
  default and production suites with 114 passing tests each, AppKit fake lifecycle
  checks, allocation instrumentation, two filtered Loom models, the release response
  grid, Apple SDK layout comparison, and the ASan/UBSan C driver harness.
- `bash scripts/build.sh` built universal app and driver binaries, verified the
  extracted source with an empty Cargo cache offline, and compared package payload
  paths, types, modes, hashes, and signatures. It did not install the package.

Rust 1.85 default/production all-target tests and standalone helper tests also
passed before the final comment labels and model-fixture assertion. The final
source passed the complete current-toolchain script without Rust warnings.

Independent correctness review covered the baseline through the final source.
Separate comment and final delta reviews found no unresolved issues. Review fixed
an overbroad proof description, stale scan documentation, and one Loom-only ignored
return. `qualified-imports.md` defines the syntax comparison's exact scope.

The local evidence bundle contains `verification.json`, final command logs, review
reports, and the canonical decision trail snapshot. The JSON records source scope,
observed exit statuses, and SHA-256 hashes of the logs. Baseline audit notes are
historical findings; the compiler probe supersedes their incorrect Option claim.

GitHub Actions was disabled at handoff, so these results are local verification.
Physical sleep/reconnect, real receiving applications, long calls, and Intel
execution remain hardware acceptance work. The sanitizer harness uses a fake HAL.
The SDK layout oracle does not prove arbitrary foreign-pointer validity.

The package build emits four `write: Permission denied` diagnostics from pkgbuild,
which an isolated replay also reproduced. pkgbuild exits 0. Payload and signature
verification passes. The internal write target remains unidentified; the evidence
bundle retains `package-warning-review.txt` rather than treating stderr as absent.

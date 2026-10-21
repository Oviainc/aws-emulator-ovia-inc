Issue-Severity: LOW
Issue-Gap-Class: generator

# `vendor/build-support` emits an ambiguous `pub use types::*;` glob, so a service whose operation name is also a shape name needs a module-wide lint allow

_Filed: 2026-10-22T00:00:00Z_
_Operation: vendor/build-support (ops module codegen) | Class: generator_

## Description

`vendor/build-support/src/lib.rs` renders each service's `<service>_ops_generated.rs` with
one `pub use types::*;` glob followed by one `pub use <snake_op>::*;` per operation. The
per-operation re-exports carry `#[allow(ambiguous_glob_reexports, unused_imports)]`; the
`types` glob carries only `#[allow(unused_imports)]`.

For a service that models an OPERATION whose name is also a shape name, both globs export
that name, so rustc raises `ambiguous_glob_reexports` — an error under the workspace's
`-D warnings` build. Both re-exports are wanted (the shape and the operation struct are
both reached through `ops`), so the ambiguity is allowed rather than resolved; three
services carry that allow today:

- `service-com-amazonaws-kinesisanalytics-kinesisanalytics-20150814` — `DeleteApplicationOutput`
- `service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523` — `DeleteApplicationOutput`
- `service-com-amazonaws-mediaconnect-mediaconnect` — `UpdateBridgeOutput`

Any newly scaffolded service with an `Update…Output` / `Delete…Output` style operation will
need the same allow.

## Why it is a gap rather than a defect

The allow has to live at the top of each affected service's `src/ops/mod.rs`, because the
colliding re-exports are written into `OUT_DIR` and the only editable file in that chain is
the include site. An inner `#![allow(...)]` there is module-wide, so it also silences any
genuine future ambiguity in that module's hand-written code, and it has to be repeated
per service rather than being carried once by the generator.

## Fix

Add `ambiguous_glob_reexports` to the attribute the generator already emits on the
`pub use types::*;` line, matching what the per-operation re-exports carry. That covers
every service, present and future, scopes the allow to the re-export instead of the whole
`ops` module, and lets the per-service `#![allow(...)]` lines be dropped.

This cannot be done from inside the emulator repo: `vendor/` is read-only here and the
merge gate refuses a change to it, so it needs a change to the vendored core library.

## Evidence

`vendor/build-support/src/lib.rs` — the `pub use types::*;` emission carries
`#[allow(unused_imports)]` while the per-operation `pub use <op>::*;` emission carries
`#[allow(ambiguous_glob_reexports, unused_imports)]`. Building the workspace with
`cargo clippy --all-targets -- -D warnings` without the per-service allows fails with
`error: ambiguous glob re-exports` in
`target/debug/build/service-com-amazonaws-*/out/*_ops_generated.rs` for the three services
listed above.

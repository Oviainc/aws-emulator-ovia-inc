Issue-Severity: LOW
Issue-Gap-Class: codegen

# The ops module generator emits ambiguous glob re-exports when an operation name is also a shape name

_Filed: 2026-10-22T00:00:00Z_
_Operation: scaffold codegen (`<service>/src/ops/mod.rs`) | Class: codegen_

## Description

`build.rs` generates `<service>_ops_generated.rs` as a list of glob re-exports:

```rust
pub use types::*;
...
pub use delete_application_output::*;
```

When a service models an OPERATION whose name is also a generated SHAPE name, that one
name is re-exported twice and `rustc` raises `ambiguous_glob_reexports`, which is an
ERROR under this workspace's `-D warnings` build. Three services hit it today:

- `service-com-amazonaws-kinesisanalytics-kinesisanalytics-20150814` and
  `service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523`: the operation
  `DeleteApplicationOutput` vs the generated output struct of `DeleteApplication`.
- `service-com-amazonaws-mediaconnect-mediaconnect`: the operations
  `UpdateBridgeOutput` / `UpdateFlowOutput` vs the output structs of `UpdateBridge` /
  `UpdateFlow`.

Each of those three `src/ops/mod.rs` carries `#![allow(ambiguous_glob_reexports)]`
so the workspace builds. That suppression is a stopgap, not the fix: with it, `ops::<Name>`
resolves to whichever glob the generator happens to emit first, so the meaning of that
path depends on generator output order.

## Suggested fix

In the ops-module generator (`vendor/build-support`, which an issue fixer cannot edit
from inside the emulator), either

1. emit explicit, non-glob re-exports for the operation structs
   (`pub use delete_application_output::DeleteApplicationOutput as DeleteApplicationOutputOperation;`
   or no re-export at all, since the generated service declaration already names each
   operation by its own module path), or
2. keep the per-item `#[allow(ambiguous_glob_reexports)]` the generated
   `src/ops/types/mod.rs` already emits, so the suppression is generated rather than
   hand-added per service.

Then drop the three hand-added crate-level allows.

## Evidence

`cargo clippy --all-targets -- -D warnings` before the allows were added:

```text
error: ambiguous glob re-exports
  --> .../com_amazonaws_kinesisanalytics_kinesisanalytics_20150814_ops_generated.rs:6:9
   |
 6 | pub use types::*;
   |         ^^^^^^^^ the name `DeleteApplicationOutput` in the type namespace is first re-exported here
...
66 | pub use delete_application_output::*;
   |         ---------------------------- but the name `DeleteApplicationOutput` in the type namespace is also re-exported here
```

## Steps to Reproduce

1. Remove `#![allow(ambiguous_glob_reexports)]` from
   `services/service-com-amazonaws-kinesisanalytics-kinesisanalytics-20150814/src/ops/mod.rs`.
2. Run `cargo clippy -p service-com-amazonaws-kinesisanalytics-kinesisanalytics-20150814 --all-targets -- -D warnings`.
3. The build fails with the error above.
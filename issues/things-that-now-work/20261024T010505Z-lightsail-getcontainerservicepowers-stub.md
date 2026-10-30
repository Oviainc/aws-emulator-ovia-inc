Issue-Severity: HIGH
Issue-Gap-Class: stub
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Implement lightsail GetContainerServicePowers: unimplemented scaffold stub

_Filed: 2026-10-24T01:05:05Z_
_Operation: GetContainerServicePowers | Class: stub_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetContainerServicePowers` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/get_container_service_powers.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

## Evidence

The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/get_container_service_powers.rs`.

## Steps to Reproduce

1. Boot the lightsail emulator
2. Call GetContainerServicePowers and observe HTTP 501 with no state persisted

## Resolution

`GetContainerServicePowers` now reports this service's published container service
power catalog (`CONTAINER_SERVICE_POWERS` in
`services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs`, the same
table a create or an update resolves a requested `power` against), and the scaffold
marker line is gone. The operation's input shape declares no member and its
documentation says it "Returns the list of powers that can be specified for your
Amazon Lightsail container services", so it reads no stored resource and writes
nothing, like this service's other published-catalog readers. Covered by
`tests/all/container_service_powers.rs`.

### Why this fix also touches Cargo.lock

The fix adds no dependency. Merging the current mainline tip into the fix branch left
`Cargo.lock` out of step with the workspace's manifests, so every `--locked` build and
`cargo metadata` refused to run; `cargo update --workspace --offline` brought it back
into step. The only lockfile change is the dependency edges the vendored
`aws-emulator-emulator-telemetry` and `aws-emulator-server` manifests already declare
in mainline (`libc` and `windows-sys 0.61.2` among them, both already resolved and
checksummed in the lockfile before this change). No `[[package]]` entry is added, no
version is moved, and no manifest under `services/` gained a dependency.

Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# The stored-probe -> response projection is implemented four times (GetProbe inline, CreateProbe, UpdateProbe, GetMonitor), so the four copies can drift

_Filed: 2026-10-28T02:32:53Z_
_Operation: GetProbe | Class: maintainability_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The computation that turns a stored `crate::state::Probe` into the wire shape — the same 13 members, with the same `Some(..)` wrapping, the same `DateTime::from` conversions for `createdAt`/`modifiedAt` and the same clone-and-collect of the stored tag map — exists as four independent copies in this crate: inline in `GetProbe::handle`, as `create_probe::output_for`, as `update_probe::output_for`, and as `get_monitor::probe_of`. The output types differ in name only (`GetProbeOutput` / `CreateProbeOutput` / `UpdateProbeOutput` / `Probe`), and every member is projected identically in all four.

That the copies were meant to be one is visible in the code: two of them are declared `pub(crate)` (`create_probe::output_for`, `get_monitor::probe_of`) yet are each used only inside their own module, so the cross-module visibility buys nothing.

The drift risk is concrete: any change to how a probe is reported has to be made four times and in four files. The crate has already had to do this once (`addressFamily` is re-derived and reported in all four), and the open "probe records never carry vpcId" issue will have to touch all four again; a copy missed is a read path that silently reports something different from its siblings, which is precisely the invariant the crate advertises ("what this reports is what `CreateProbe` stored and `GetMonitor` lists").

A single shared projection (one mapping, with the per-operation output type named at the call sites) leaves every response byte-identical and every existing test passing.

## Evidence

Copy 1 — `src/ops/get_probe.rs:207-227` (inline, no helper):
```rust
        Ok(GetProbeOutput {
            probe_id: Some(probe.probe_id.clone()),
            probe_arn: Some(probe.probe_arn.clone()),
            source_arn: probe.source_arn.clone(),
            ...
            created_at: Some(::aws_smithy_types::DateTime::from(probe.created_at)),
            modified_at: Some(::aws_smithy_types::DateTime::from(probe.modified_at)),
            tags: Some(probe.tags.iter().map(|(key, value)| (key.clone(), value.clone())).collect()),
        })
```
Copy 2 — `src/ops/create_probe.rs:161-184`:
```rust
/// This operation's answer, read out of the STORED probe record.
pub(crate) fn output_for(probe: &crate::state::Probe) -> CreateProbeOutput {
    CreateProbeOutput { probe_id: Some(probe.probe_id.clone()), ... }
```
Copy 3 — `src/ops/update_probe.rs:185-208`:
```rust
/// This operation's answer, read out of the STORED probe record.
fn output_for(probe: &crate::state::Probe) -> UpdateProbeOutput {
    UpdateProbeOutput { probe_id: Some(probe.probe_id.clone()), ... }
```
Copy 4 — `src/ops/get_monitor.rs:123-146`:
```rust
/// The modelled `Probe` a stored probe record reports as.
pub(crate) fn probe_of(probe: &crate::state::Probe) -> Probe {
    Probe { probe_id: Some(probe.probe_id.clone()), ... }
```
The unused cross-module visibility (`grep output_for|probe_of` over `src/`): `create_probe::output_for` is referenced only at `create_probe.rs:278` and `:361`; `get_monitor::probe_of` only at `get_monitor.rs:223` — both `pub(crate)` with no external caller.

Tests that pin the four copies' output and must stay unchanged: `tests/all/monitor_and_probe.rs` (CreateProbe response and the `GetMonitor` `probes[]` read-back, e.g. lines 471-652), `tests/all/update_probe.rs` (`UpdateProbe` response vs. `GetProbe`/`GetMonitor`, lines 140-324), `tests/all/resource_tags.rs` (`probes[].tags` and `GetProbe` tag read-back), `tests/all/probe_teardown.rs` (`state`/`modifiedAt` across all read paths).

## Steps to Reproduce

1. grep -n "probe_id: Some(probe.probe_id.clone())" services/service-com-amazonaws-networkmonitor-networkmonitor/src/ops/*.rs — four hits in four files.
2. Read `src/ops/get_probe.rs:207-227`, `src/ops/create_probe.rs:162-184`, `src/ops/update_probe.rs:186-208`, `src/ops/get_monitor.rs:124-146`: the same 13 members, projected identically.
3. grep -n "output_for|probe_of" over `src/`: the two `pub(crate)` copies have no caller outside their own module, so the duplication is not serving a sharing need.
4. Confirm the fix is response-preserving: collapse the four into one shared projection used by all four call sites; `cargo test -p service-com-amazonaws-networkmonitor-networkmonitor` must pass with no test edits.

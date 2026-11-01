Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: dbe80abdd81a79f1a95892fbee40e178463ee847

# The stored-monitor -> response projection is implemented three times (CreateMonitor, UpdateMonitor, ListMonitors' summary), so the copies can drift

_Filed: 2026-10-29T12:46:07Z_
_Operation: UpdateMonitor | Class: maintainability_

> _Note: this issue was identified against the code base at commit `dbe80abdd81a79f1a95892fbee40e178463ee847`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The computation that turns a stored `crate::state::Monitor` into the wire shape — the same five members (`monitorArn`, `monitorName`, `state`, `aggregationPeriod` wrapped in `Some`, and the clone-and-collect of the stored tag map wrapped in `Some`) — exists as three independent, character-for-character identical copies in this crate: `create_monitor::output_for`, `update_monitor::output_for`, and the inline `MonitorSummary` literal inside `ListMonitors::handle`. The output types differ in name only (`CreateMonitorOutput` / `UpdateMonitorOutput` / `MonitorSummary`); `GetMonitor` then projects the same five members a fourth time as part of its superset output.

The drift risk is concrete and this crate has already paid it on the probe side: the identical duplication of the stored-probe projection was filed and fixed by collapsing it into the single `crate::state::probe_report!` macro (`src/state.rs:1033-1060`), whose own doc states the reason — "ONE projection rather than a hand-written copy of the same member moves per operation ... a change to how a probe is reported lands in every read path at once". The monitor-side projection never got the same treatment, so a change to how a monitor is reported (a new member, a different `tags` representation for an empty map) must be made in three places, and a copy missed is a read path that silently reports something different from its siblings.

A single shared projection with the per-operation output type named at the call sites leaves every response byte-identical and every existing test passing.

## Evidence

Copy 1 — `src/ops/create_monitor.rs:174-189`:
```rust
/// This operation's answer, read out of the STORED monitor record.
fn output_for(monitor: &crate::state::Monitor) -> CreateMonitorOutput {
    CreateMonitorOutput {
        monitor_arn: monitor.monitor_arn.clone(),
        monitor_name: monitor.monitor_name.clone(),
        state: monitor.state.clone(),
        aggregation_period: Some(monitor.aggregation_period),
        tags: Some(monitor.tags.iter().map(|(key, value)| (key.clone(), value.clone())).collect()),
    }
}
```
Copy 2 — `src/ops/update_monitor.rs:139-154` (same body, different type name):
```rust
/// This operation's answer, read out of the STORED monitor record.
fn output_for(monitor: &crate::state::Monitor) -> UpdateMonitorOutput {
    UpdateMonitorOutput {
        monitor_arn: monitor.monitor_arn.clone(),
        monitor_name: monitor.monitor_name.clone(),
        state: monitor.state.clone(),
        aggregation_period: Some(monitor.aggregation_period),
        tags: Some(monitor.tags.iter().map(|(key, value)| (key.clone(), value.clone())).collect()),
    }
}
```
Copy 3 — `src/ops/list_monitors.rs:253-268` (inline, no helper):
```rust
        let monitors: Vec<MonitorSummary> = matching[start..end]
            .iter()
            .map(|monitor| MonitorSummary {
                monitor_arn: monitor.monitor_arn.clone(),
                monitor_name: monitor.monitor_name.clone(),
                state: monitor.state.clone(),
                aggregation_period: Some(monitor.aggregation_period),
                tags: Some(monitor.tags.iter().map(|(key, value)| (key.clone(), value.clone())).collect()),
            })
```
And the same five members a fourth time inside `GetMonitor`'s superset output — `src/ops/get_monitor.rs:191-202`.

The rule the crate already adopted for the probe-side twin — `src/state.rs:1026-1032`:
```rust
/// ONE projection rather than a hand-written copy of the same member moves per
/// operation: every member is read out of the record, so what `CreateProbe`
/// answers is what `GetProbe` reads back and what `GetMonitor` lists, and a
/// change to how a probe is reported lands in every read path at once.
```

Tests that pin the three copies' output and must stay unchanged: `tests/all/monitor_and_probe.rs:283-321` (CreateMonitor `monitorArn`/`aggregationPeriod` and the GetMonitor read-back), `tests/all/update_monitor.rs:171-230` (UpdateMonitor response vs. GetMonitor, incl. `monitorArn` at `:181` and `tags` at `:196`), `tests/all/tagging.rs:33-119` (the `ListMonitors` summary `tags`), `tests/all/region_isolation.rs:233-434` (`monitorArn` on create/replay/read).

## Steps to Reproduce

1. grep -n "monitor_arn: monitor.monitor_arn.clone()" services/service-com-amazonaws-networkmonitor-networkmonitor/src/ops/*.rs — four hits in four files (create_monitor, update_monitor, list_monitors, get_monitor).
2. Read `src/ops/create_monitor.rs:174-189`, `src/ops/update_monitor.rs:139-154` and `src/ops/list_monitors.rs:253-268`: the same five members, projected identically, with only the output type's name differing.
3. Compare with the probe-side resolution already in the crate (`src/state.rs:1033-1060`, `probe_report!`), used by `create_probe.rs:168`, `update_probe.rs:192`, `get_probe.rs:209` and `get_monitor.rs:127`.
4. Confirm the fix is response-preserving: collapse the three into one shared projection used by all call sites (GetMonitor reusing it for those five members); `cargo test -p service-com-amazonaws-networkmonitor-networkmonitor` must pass with no test edits.

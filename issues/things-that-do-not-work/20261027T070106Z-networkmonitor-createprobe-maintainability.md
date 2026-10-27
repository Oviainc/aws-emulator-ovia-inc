Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 75491f6dad17b9da11cd051dade27675220440ff

# The per-monitor probe ceiling is implemented inline twice (CreateProbe and CreateMonitor) while its sibling per-subnet ceiling lives in one shared helper

_Filed: 2026-10-27T07:01:06Z_
_Operation: CreateProbe | Class: maintainability_

> _Note: this issue was identified against the code base at commit `75491f6dad17b9da11cd051dade27675220440ff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`MAX_PROBES_PER_MONITOR` is enforced by two separate inline branches, one in each create path, each re-deciding how the count is taken and each re-raising `PROBES_PER_MONITOR_MESSAGE`: `create_probe.rs` counts the probes the monitor already holds and compares with `>=`, `create_monitor.rs` counts the probes the request asks for and compares with `>`. The two copies can drift (they already differ in which population they count and in the comparison operator), and neither combines stored + requested the way the crate's other probe-count rule does.

The crate already has the shape of the fix for exactly this rule: the sibling per-SUBNET ceiling is implemented once in `crate::state::check_probes_per_subnet(existing, requested)` and called from both create paths, with a doc that states the reason for centralising it. Every other rule the two create paths share is likewise a single function in `state.rs` (`check_tag_keys`, `validate_probe_spec`, `resolve_aggregation_period`, `validate_settable_probe_state`). The per-monitor ceiling is the one that is not, so it is the one that can be enforced on one path and not the other.

A `check_probes_per_monitor(existing, requested)` helper in `state.rs`, mirroring `check_probes_per_subnet`, replaces both branches and changes no response: both copies today refuse at the same effective total (100) and both raise the same `ValidationException` message.

## Evidence

Copy 1 — `services/.../src/ops/create_probe.rs:292-299`:
```
        let probe_count = store
            .monitor(&region, &input.monitor_name)
            .map_or(0, |monitor| monitor.probes.len());
        if probe_count >= crate::state::MAX_PROBES_PER_MONITOR {
            return Err(validation(
                crate::state::PROBES_PER_MONITOR_MESSAGE.to_string(),
            ));
        }
```
Copy 2 — `services/.../src/ops/create_monitor.rs:324-328`:
```
        if specs.len() > crate::state::MAX_PROBES_PER_MONITOR {
            return Err(validation(
                crate::state::PROBES_PER_MONITOR_MESSAGE.to_string(),
            ));
        }
```
The sibling ceiling, implemented ONCE with the reason stated — `services/.../src/state.rs:783-790`:
```
/// Shared by every operation that ADDS a probe to a monitor (`CreateProbe`, and
/// `CreateMonitor`'s inline `probes` list), so the limit cannot be enforced on
/// one path and not the other. `Err` carries
/// [`PROBES_PER_SUBNET_MESSAGE`].
pub fn check_probes_per_subnet<'a>(
    existing: Option<&'a Monitor>,
    requested: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
```
called from `create_probe.rs:310-314` and `create_monitor.rs:346-350`.

Test coverage: `tests/all/probes_per_subnet_limit.rs` pins the shared per-subnet helper on both create paths (`create_monitor_bounds_its_inline_probes_per_source_subnet:97`, `create_probe_counts_the_probes_the_monitor_already_holds:186`, `the_bound_is_counted_per_monitor:246`, `a_malformed_probe_outranks_the_count:266`) and `grep PROBES_PER_MONITOR tests/` returns nothing, so the two duplicated branches are the probe-count rule with no test of their own — the refactor must keep both refusals (400 `ValidationException`, message `"Maximum probes per monitor limit exceeded."`) and the existing per-subnet assertions byte-identical.

## Steps to Reproduce

1. Open `services/service-com-amazonaws-networkmonitor-networkmonitor/src/ops/create_probe.rs` at lines 292-299 and `src/ops/create_monitor.rs` at lines 324-328: two inline implementations of the same `MAX_PROBES_PER_MONITOR` ceiling and the same refusal.
2. Open `src/state.rs` lines 773-809: the sibling per-subnet ceiling is a single shared helper whose doc says it is shared 'so the limit cannot be enforced on one path and not the other'.
3. Run `grep -rn "PROBES_PER_MONITOR" services/service-com-amazonaws-networkmonitor-networkmonitor/` and observe the only two call sites are the two inline branches; no test names the message.
4. Verify the behaviour is identical after hoisting the branch into one `state.rs` helper by running the existing `probes_per_subnet_limit.rs` and `monitor_and_probe.rs` suites.

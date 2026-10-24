Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# Deactivating a monitor's probes leaves the monitor reported ACTIVE (monitor state does not follow probe state)

_Filed: 2026-10-24T05:26:27Z_
_Operation: UpdateProbe | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a monitor's state reflects whether any of its probes is actually measuring. Once every probe of a monitor has been deactivated (UpdateProbe state=INACTIVE), the monitor has nothing aggregating and is reported INACTIVE; reactivating a probe moves the monitor back to PENDING and then ACTIVE. AWS's own teardown contract depends on this coupling — DeleteMonitor refuses while any attached probe is not inactive (see open issue 20261023T004257Z, cause 1), which is only a reachable state if probe deactivation is a real, observable monitor-level condition.

Actual (emulator): `UpdateProbe` writes only the probe's `state` field. The containing monitor's `state` is untouched, so a monitor whose single probe is INACTIVE is still reported ACTIVE by GetMonitor and by ListMonitors (including when filtering `state=ACTIVE`). The emulator is internally inconsistent about its own rule: `DeleteProbe` calls `Monitor::clear_activation()` to put a monitor back to INACTIVE when its last probe is REMOVED, and `CreateMonitor` stores a probe-free monitor as INACTIVE ("a monitor aggregates what its probes measure"), but deactivating that same probe instead of deleting it leaves the monitor ACTIVE forever.

Consequence: a client that polls ListMonitors(state="INACTIVE") or GetMonitor to confirm "all probes are off" before deleting a monitor never observes the transition, and the ACTIVE/INACTIVE monitor summary is wrong for any monitor with only deactivated probes.

## Evidence

Spec: MonitorState is `ACTIVE | DELETING | ERROR | INACTIVE | PENDING` and `GetMonitor$state` is @required; `UpdateProbeInput$state` is the documented activate/deactivate toggle (settable values ACTIVE/INACTIVE, per the emulator's own `SETTABLE_PROBE_STATES`). Handler: `src/ops/update_probe.rs` handler lines ~319-327 assign `probe.state = requested_state` and set `probe.modified_at`; nothing in that handler reads or writes `monitor.state` / `monitor.activates_at`. Contrast `src/ops/delete_probe.rs` (~line 178) which calls `monitor.clear_activation()` when the last probe goes away, and `src/state.rs::initial_monitor_state()` which stores a probe-free monitor as INACTIVE. Tests: `tests/all/update_probe.rs::a_probe_can_be_deactivated_and_reactivated` asserts only the PROBE's reported state; `tests/all/monitor_lifecycle_and_front_door.rs::a_probe_added_to_an_idle_monitor_activates_it_and_its_deletion_idles_it_again` covers add/delete, not deactivate. No existing issue covers probe deactivation vs monitor state.

## Steps to Reproduce

1. CreateMonitor(monitorName="m1", aggregationPeriod=60).
2. CreateProbe(monitorName="m1", probe={sourceArn: <subnet ARN>, destination: "203.0.113.10", protocol: "ICMP", packetSize: 56}).
3. Poll GetMonitor(monitorName="m1") until state == "ACTIVE" (the emulator settles PENDING -> ACTIVE after ~500ms).
4. UpdateProbe(monitorName="m1", probeId=<id>, state="INACTIVE") and confirm the probe reports INACTIVE.
5. GetMonitor(monitorName="m1"): observe state still "ACTIVE" even though no probe is active; expected INACTIVE. ListMonitors(state="ACTIVE") still lists the monitor, and ListMonitors(state="INACTIVE") omits it.
6. UpdateProbe(..., state="ACTIVE") again: the monitor state should return to PENDING then ACTIVE.

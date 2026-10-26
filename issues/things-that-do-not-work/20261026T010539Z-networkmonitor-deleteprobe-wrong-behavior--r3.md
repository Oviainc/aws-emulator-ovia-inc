Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9b93cbc54d897386c30bfd66ea5bd8f85f03a356

# DeleteProbe of a monitor's only deactivated probe puts the monitor back to PENDING and then ACTIVE

_Filed: 2026-10-26T01:05:39Z_
_Operation: DeleteProbe | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9b93cbc54d897386c30bfd66ea5bd8f85f03a356`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An accepted DeleteProbe calls `Probe::start_teardown`, which OVERWRITES the probe's stored `state` with `DELETING`. `Probe::is_deactivated` treats only the literal `INACTIVE` as out of service, so the teardown write turns a probe that the caller had switched off back into a probe the monitor counts as "in service" (`Monitor::has_probe_in_service`).

Consequence: a monitor whose only probe had been deactivated is stored `INACTIVE` with nothing due (that is what `UpdateProbe` + `follow_probe_states` leave behind, and what `monitor_state_follows_probe_activation.rs` pins). After a DeleteProbe of that probe, the very next reporting call runs `settle_monitor` -> `follow_probe_states` -> `schedule_activation`, which moves the monitor `INACTIVE` -> `PENDING` and makes the `ACTIVE` transition due; `ACTIVATION_AFTER` (500 ms) later `GetMonitor`/`ListMonitors` report the monitor `ACTIVE` while its only probe reports `DELETING`/`DELETED`. The monitor only falls back to `INACTIVE` once the probe record is forgotten (two `PROBE_TEARDOWN_STAGE` spans after the delete).

So the call that takes the last thing a monitor could aggregate out of the store is the call that ACTIVATES the monitor — the opposite of the rule this crate states for the state. DeleteProbe is also the only probe-mutating handler that does not re-run `Monitor::follow_probe_states` itself after writing the probe, so the monitor-level consequence of its write is decided only by the next caller's settle walk.

## Evidence

The rule the crate states for `INACTIVE`, in `src/state.rs:61-70`:

```
/// `MonitorState` of a monitor that has no probe in service.
///
/// A monitor aggregates what its probes measure, so one with nothing measuring
/// has nothing to activate: it is reported `INACTIVE` straight away and stays
/// there — nothing is due — until a probe of its own is in service again. This
/// is the state a monitor created with an empty (or absent) `probes` list is in,
/// the one a monitor returns to when its last probe is deleted, and the one it
/// returns to when every probe it holds has been deactivated
/// ([`Monitor::follow_probe_states`]).
pub const MONITOR_STATE_INACTIVE: &str = "INACTIVE";
```

and on the write that is made instead, `src/state.rs:911-918`:

```
    pub fn schedule_activation(&mut self, now: SystemTime) {
        if self.activates_at.is_none() {
            self.activates_at = now.checked_add(ACTIVATION_AFTER);
            if self.state == MONITOR_STATE_INACTIVE {
                self.state = MONITOR_STATE_PENDING.to_string();
```
whose doc one line above (state.rs:908-910) justifies it with "A monitor that was `INACTIVE` — nothing to aggregate — is provisioning what it just gained" — but a delete gains nothing.

The code path, `src/state.rs:822-829` (the delete overwrites `INACTIVE`):

```
    pub fn start_teardown(&mut self, now: SystemTime) {
        if self.is_being_torn_down() {
            return;
        }
        self.state = PROBE_STATE_DELETING.to_string();
```

`src/state.rs:856-858` and `:929-931` (a `DELETING` probe therefore counts as in service):

```
    pub fn is_deactivated(&self) -> bool {
        self.state == PROBE_STATE_INACTIVE
    }
...
    pub fn has_probe_in_service(&self) -> bool {
        self.probes.values().any(|probe| !probe.is_deactivated())
    }
```

`src/state.rs:947-961` (`follow_probe_states`, reached from `settle_monitor` at `src/state.rs:1190`, which every reporting handler calls) then schedules the activation and promotes to `ACTIVE`.

The handler that performs the write and does NOT follow the probe states, `src/ops/delete_probe.rs:194-199`:

```
        let already_tearing_down = probe.is_being_torn_down();
        probe.start_teardown(now);
        if !already_tearing_down {
            monitor.modified_at = now;
        }
        Ok(DeleteProbeOutput {})
```
compared with its siblings `src/ops/create_probe.rs:318-319` (`monitor.modified_at = now; monitor.follow_probe_states(now);`) and `src/ops/update_probe.rs:365` (`monitor.follow_probe_states(now);`).

Test coverage: `tests/all/probe_teardown.rs:328-419` (`a_monitor_keeps_aggregating_until_its_last_probe_has_gone`) only deletes probes that are `ACTIVE` at the time of the delete, so the monitor is already `ACTIVE` and the regression is invisible; `tests/all/monitor_state_follows_probe_activation.rs:103-189` pins deactivation WITHOUT a delete (monitor `INACTIVE`). The deactivate-then-delete combination is pinned by no test.

## Steps to Reproduce

1. POST /monitors with body {"monitorName":"m","probes":[{"sourceArn":"<subnet arn>","destination":"10.0.0.1","protocol":"ICMP","packetSize":56}]}
2. Sleep past ACTIVATION_AFTER and GET /monitors/m — the monitor reports ACTIVE and the probe ACTIVE
3. PATCH /monitors/m/probes/{probeId} with {"state":"INACTIVE"}; GET /monitors/m now reports INACTIVE (nothing in service, nothing due)
4. DELETE /monitors/m/probes/{probeId} — 200, the teardown is accepted
5. GET /monitors/m immediately: the monitor reports PENDING, although no probe came into service (expected: still INACTIVE)
6. Sleep 500 ms (ACTIVATION_AFTER) and GET /monitors/m again: the monitor reports ACTIVE while its only probe is listed in DELETING/DELETED, until the probe record is forgotten ~600 ms after the delete

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: fa5ed22240e89dcfd43a22a078f364ee68f69599

# An accepted DeleteProbe moves the probe to DELETING but leaves the probe's own modifiedAt at its old value

_Filed: 2026-10-25T09:16:51Z_
_Operation: DeleteProbe | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `fa5ed22240e89dcfd43a22a078f364ee68f69599`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An accepted `DeleteProbe` changes a stored value every read of the probe reports — `probe.state` becomes `DELETING` and `probe.deletes_at` is set — yet it moves only the MONITOR's `modified_at`. The probe's own `modified_at` is never touched, so `GetProbe`/`GetMonitor` report a probe in `DELETING` (and then `DELETED`) whose `modifiedAt` is still the creation instant, or the instant of the last `UpdateProbe`.

This contradicts the rule the crate applies everywhere else for the same record: `UpdateProbe` moves `probe.modified_at` whenever a stored value — including `state` — actually changed, and a tag write on a probe moves it through `state::mark_modified`. Only the delete path, which performs the largest state change a probe undergoes, leaves the timestamp stale. The modelled member exists to report exactly this (`Probe$modifiedAt`: "The time and date that the probe was last modified"), so a client watching the teardown sees a state transition with no corresponding `modifiedAt` movement.

## Evidence

The delete path — services/service-com-amazonaws-networkmonitor-networkmonitor/src/ops/delete_probe.rs:194-198:
```
        let already_tearing_down = probe.is_being_torn_down();
        probe.start_teardown(now);
        if !already_tearing_down {
            monitor.modified_at = now;
        }
```
and what `start_teardown` changes — src/state.rs:754-760:
```
    pub fn start_teardown(&mut self, now: SystemTime) {
        if self.is_being_torn_down() {
            return;
        }
        self.state = PROBE_STATE_DELETING.to_string();
        self.deletes_at = Some(now.checked_add(PROBE_TEARDOWN_STAGE).unwrap_or(now));
    }
```
(no `self.modified_at = now`).

The sibling that applies the rule to the same field — src/ops/update_probe.rs:323-349:
```
        // `modifiedAt` moves only when a stored value actually changed, so a
        // repeat of the same update (the model marks this operation
        // `@idempotent`) leaves the record as it was.
        ...
        if let Some(requested_state) = input.state {
            if probe.state != requested_state {
                changed = true;
                probe.state = requested_state;
            }
        }
        if changed {
            probe.modified_at = now;
        }
```
and src/state.rs:1129-1135: "Record that a call CHANGED one resolved resource, moving its `modifiedAt` to `now`. ... Called only when the stored tag map actually changed" (`mark_modified`, used by TagResource/UntagResource for a probe target).

Model: `Probe$modifiedAt` — "<p>The time and date that the probe was last modified.</p>" (smithy_query shape=Probe).

Test coverage: tests/all/update_probe.rs:764-824 (`modified_at_moves_only_when_a_stored_value_changed`) pins the rule for the update path; tests/all/probe_teardown.rs asserts the teardown states and the monitor's state but never reads `modifiedAt`, so nothing covers the delete path.

## Steps to Reproduce

1. CreateMonitor(monitorName="m1") -> 200; CreateProbe(monitorName="m1", probe={sourceArn=<subnet arn>, destination="10.0.0.1", protocol="ICMP", packetSize=56}) -> 200; note `createdAt` == `modifiedAt`.
2. Wait past activation and GetProbe -> state ACTIVE, `modifiedAt` unchanged.
3. DeleteProbe(monitorName="m1", probeId=P) -> 200.
4. GetProbe(monitorName="m1", probeId=P) -> 200 with state "DELETING" but `modifiedAt` still equal to `createdAt`.
5. Compare with UpdateProbe(state="INACTIVE") on a fresh probe, which moves `modifiedAt` for the same kind of change (tests/all/update_probe.rs:575-621).

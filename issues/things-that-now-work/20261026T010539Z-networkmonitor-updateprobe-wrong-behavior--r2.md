Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9b93cbc54d897386c30bfd66ea5bd8f85f03a356

# UpdateProbe never moves the containing monitor's modifiedAt, although it rewrites the monitor's own state

_Filed: 2026-10-26T01:05:39Z_
_Operation: UpdateProbe | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9b93cbc54d897386c30bfd66ea5bd8f85f03a356`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateProbe writes the stored probe (destination/port/protocol/packetSize/state) and then re-derives the MONITOR's own stored fields through `Monitor::follow_probe_states`, which can flip `monitor.state` (ACTIVE -> INACTIVE when the last probe is deactivated, INACTIVE -> PENDING when one is switched back on) and set or clear `monitor.activates_at`. It never moves `monitor.modified_at`.

The two sibling probe-mutating handlers of this crate do exactly that write for the same kind of change: CreateProbe sets `monitor.modified_at = now` with the comment "The monitor changed ... and its `modifiedAt` moves", and DeleteProbe sets `monitor.modified_at = now` for an accepted teardown. The result is that `GetMonitor` reports a monitor whose `state` a caller's UpdateProbe just changed, with a `modifiedAt` still pointing at the last create/delete — the crate's own rule for a changed record (`mark_modified`: "Record that a call CHANGED one resolved resource, moving its `modifiedAt` to `now`" ... "part of the resource every read reports, so a call that removes or adds one modified the resource") is applied on three paths and skipped on this one.

## Evidence

The path that skips it, `src/ops/update_probe.rs:353-366`:

```
        if changed {
            probe.modified_at = now;
        }
        // Answered from the STORED probe, ...
        let output = output_for(probe);
        // A monitor aggregates what its probes measure, so switching a probe
        // off or back on changes what the CONTAINING monitor is reporting too:
        // the monitor follows the probes it now holds
        // ([`crate::state::Monitor::follow_probe_states`]) — back to `INACTIVE`
        // once it has nothing in service, and owed `PENDING` -> `ACTIVE` again
        // the moment it has.
        monitor.follow_probe_states(now);
```
(the comment states the monitor's own report changes; no `monitor.modified_at` write follows)

The siblings that perform it — `src/ops/create_probe.rs:313-319`:

```
        monitor.probes.insert(probe_id.clone(), probe);
        // The monitor changed: it now holds a probe in service, so it is owed
        // the `PENDING` -> `ACTIVE` transition and its `modifiedAt` moves. ...
        monitor.modified_at = now;
        monitor.follow_probe_states(now);
```

and `src/ops/delete_probe.rs:194-198`:

```
        let already_tearing_down = probe.is_being_torn_down();
        probe.start_teardown(now);
        if !already_tearing_down {
            monitor.modified_at = now;
        }
```

and the rule both follow, `src/state.rs:1288-1293`:

```
    /// Record that a call CHANGED one resolved resource, moving its
    /// `modifiedAt` to `now`.
    ///
    /// Tags are part of the resource every read reports, so a call that removes
    /// or adds one modified the resource. Called only when the stored tag map
    /// actually changed: a tagging call that is a no-op modified nothing.
```
(used by TagResource `src/ops/tag_resource.rs:234-236` and UntagResource `src/ops/untag_resource.rs:203-205`).

Test coverage: `tests/all/update_probe.rs` asserts the PROBE's `modifiedAt` (lines 768-830, 579-630) and `tests/all/probe_teardown.rs` asserts the probe's `modifiedAt` after a delete, but no test reads the MONITOR's `modifiedAt` after an UpdateProbe, so nothing pins the current value.

## Steps to Reproduce

1. POST /monitors {"monitorName":"m","probes":[{"sourceArn":"<subnet arn>","destination":"10.0.0.1","protocol":"ICMP","packetSize":56}]}
2. Sleep past ACTIVATION_AFTER; GET /monitors/m and record `state` (ACTIVE) and `modifiedAt`
3. PATCH /monitors/m/probes/{probeId} with {"state":"INACTIVE"} — 200
4. GET /monitors/m: `state` is now INACTIVE (the monitor record was rewritten) but `modifiedAt` is unchanged from step 2
5. Contrast: DELETE /monitors/m/probes/{probeId} (or a CreateProbe) does move the monitor's `modifiedAt` for the same class of change

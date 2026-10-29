Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e39df06442d4f2a86a0fa0e3a1f1c22c399555b8

# A tag write to a PROBE ARN leaves the containing monitor's modifiedAt untouched, although the crate's rule is that any call rewriting a probe changed the monitor

_Filed: 2026-10-28T15:55:48Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e39df06442d4f2a86a0fa0e3a1f1c22c399555b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` (and `UntagResource`) resolve `resourceArn` to either a monitor or a probe. When the target is a PROBE, the accepted write changes that probe's tag map and calls `state::mark_modified`, which moves only `probe.modified_at`. The MONITOR that holds the probe keeps its old `modifiedAt`, even though `GetMonitor` reports the probe with its tags, so the monitor's reported content changed while its `modifiedAt` says nothing changed.

That contradicts the rule this crate states and applies on every other probe-writing path: `Monitor::follow_probe_write`'s doc says the probes are part of the monitor record every read reports, "so a call that adds a probe, takes one away or rewrites one CHANGED the monitor — the same rule a changed tag map follows (mark_modified)", and `CreateProbe`, `UpdateProbe` and `DeleteProbe` all call `monitor.follow_probe_write(changed, now)` so the monitor's `modifiedAt` moves with the probe's. The two tagging handlers are the only probe-writing paths that call `mark_modified` alone and never `follow_probe_write`, so the helper the crate already has for exactly this consequence is skipped.

## Evidence

The rule, services/service-com-amazonaws-networkmonitor-networkmonitor/src/state.rs:1102-1107 (`Monitor::follow_probe_write` doc):
```
/// The probes are part of the monitor record every read reports
/// (`GetMonitor` lists them with their own members), so a call that adds a
/// probe, takes one away or rewrites one CHANGED the monitor — the same rule
/// a changed tag map follows
/// ([`ComAmazonawsNetworkmonitorNetworkmonitorState::mark_modified`]).
```

The sibling that applies it, src/ops/update_probe.rs:363-371:
```
// The write landed in a probe the MONITOR holds and every read of the
// monitor reports, so an update that stored something new changed the
// monitor too and its `modifiedAt` moves with the probe's; ...
monitor.follow_probe_write(changed, now);
```
(`CreateProbe` does the same at src/ops/create_probe.rs:338, `DeleteProbe` at src/ops/delete_probe.rs:213.)

The path that skips it, src/ops/tag_resource.rs:261-266:
```
// A write that assigned every key the value it already had modified
// nothing, so it does not move `modifiedAt` — the same rule
// `UntagResource` applies to a removal that removed nothing.
if changed {
    store.mark_modified(&target, now);
}
```
and the Probe branch of that helper, src/state.rs:1497-1508:
```
TaggedResource::Probe { region, monitor_name, probe_id } => {
    if let Some(probe) = self.monitor_mut(region, monitor_name)
        .and_then(|monitor| monitor.probes.get_mut(probe_id))
    { probe.modified_at = now; }
}
```
— nothing touches `monitor.modified_at`. `UntagResource` has the identical gap at src/ops/untag_resource.rs:242-244.

That the probe's tags ARE part of what a read of the monitor reports: src/ops/get_monitor.rs:138-144 puts `probe.tags` into each `Probe` of `GetMonitorOutput.probes`, and get_monitor.rs:221-223 `// The stored probe map IS this list`.

Test coverage: tests/all/monitor_modified_at_follows_probe_writes.rs:112-116 states the rule — `/// Every call that writes a probe of a stored monitor moves the MONITOR's modifiedAt to the instant of that call` — but its row table (lines 118-180) contains only CreateProbe/UpdateProbe/DeleteProbe/GetMonitor, so no test exercises a tag write against a probe ARN; tests/all/resource_tags.rs:1206-1339 asserts the `modifiedAt` rule for tag writes against a MONITOR ARN only. No comment or test states that a probe tag write should leave the monitor's `modifiedAt` alone.

## Steps to Reproduce

1. POST /monitors with `{"monitorName":"probe-tagged","probes":[{"sourceArn":"arn:aws:ec2:us-east-1:<account>:subnet/subnet-0123456789abcdef0","destination":"10.0.0.1","protocol":"ICMP","packetSize":56}]}`
2. GET /monitors/probe-tagged and record the monitor's `modifiedAt` and the `probeArn` of its probe
3. Wait long enough for the wire timestamp to differ, then POST /tags/{probeArn} with `{"tags":{"owner":"net"}}` (204)
4. GET /monitors/probe-tagged: the probe inside `probes` now reports the new tag and a moved `modifiedAt`, but the monitor's own `modifiedAt` is unchanged — while the same observation after `PATCH /monitors/probe-tagged/probes/{probeId}` with `{"packetSize":1500}` shows the monitor's `modifiedAt` moving with the probe's
5. Repeat with DELETE /tags/{probeArn}?tagKeys=owner for the UntagResource half

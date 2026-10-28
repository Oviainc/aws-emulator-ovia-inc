Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# TagResource/UntagResource mutate a probe whose teardown has been accepted, which the crate's own rule says no call may change

_Filed: 2026-10-28T05:28:36Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

After an accepted `DeleteProbe`, the probe record stays readable while it drains (`DELETING` -> `DELETED` -> forgotten). `UpdateProbe` explicitly refuses any call against such a probe with `ResourceNotFoundException`, on the stated ground that the teardown is irreversible and the probe "is no longer a target for an update", and the teardown suite states the rule as "no call puts it back into service or changes what it still reports".

The tagging mutations break that rule. `resolve_tagged_resource` walks every probe of every monitor with no teardown filter, so a draining probe's ARN still resolves, and `TagResource` (and `UntagResource`) then write the probe's stored tag map and move its `modifiedAt` via `mark_modified`, answering 204. Both are values the drain-phase reads report: `GetProbe`/`GetMonitor` report the probe's `tags` and `modifiedAt`, and the teardown suite pins `modifiedAt` as being left exactly where the accepted delete put it by "every later call that changes nothing — a repeat of the delete, a read".

So the same resource is simultaneously (a) unmutable through `UpdateProbe` (404) and (b) mutable through the tagging route (204) — one crate, two answers for the same draining probe. Worse, the tagging write is a success that persists nothing a caller can keep: one `PROBE_TEARDOWN_STAGE` later `settle_monitor` drops the record (`monitor.probes.retain(|_, probe| !probe.is_forgotten_at(now))`) and the just-written tags vanish, so the 204 claims a durable write the service already knows it is discarding.

## Evidence

The rule, as the crate states it — services/service-com-amazonaws-networkmonitor-networkmonitor/src/ops/update_probe.rs:290-305:
```
        // A probe whose teardown has been accepted is on its way out and its
        // teardown is irreversible, so it is no longer a target for an update:
        // the id names a probe this monitor is giving up, which is what the
        // not-found answer tells the caller. Reads still report it, in the
        // teardown state it has reached, until the clock has carried it through.
        if probe.is_being_torn_down() {
            return Err(UpdateProbeErrorModeled::ResourceNotFoundException(...));
        }
```
and the suite that pins it — tests/all/probe_teardown.rs:421-423:
```
/// The teardown is IRREVERSIBLE: a probe on its way out is no longer a target
/// for an update, so no call puts it back into service or changes what it still
/// reports.
```
plus tests/all/probe_teardown.rs:563-566:
```
/// An accepted delete MODIFIES the probe, so the probe's own `modifiedAt` moves
/// to the instant of that call, and every later call that changes nothing — a
/// repeat of the delete, a read — leaves it exactly there.
```

The path that ignores it — services/service-com-amazonaws-networkmonitor-networkmonitor/src/ops/tag_resource.rs:202-251 (same shape in untag_resource.rs:199-231):
```
        store.settle_monitors_in(&region, now);
        let target = store
            .resolve_tagged_resource(&region, &input.resource_arn)
            .ok_or_else(|| not_found(&input.resource_arn))?;
        let tags = store.tags_mut(&target)...
        for (key, value) in input.tags { changed |= ...; tags.insert(key, value); }
        if changed { store.mark_modified(&target, now); }
```
and the resolver it uses, which has no teardown check — src/state.rs:1361-1369:
```
            for probe in monitor.probes.values() {
                if probe.probe_arn == resource_arn {
                    return Some(TaggedResource::Probe { ... });
```
while `Probe::is_being_torn_down` (src/state.rs:937-939) is the predicate the crate already has for exactly this question. The write is then discarded by src/state.rs:1313: `monitor.probes.retain(|_, probe| !probe.is_forgotten_at(now));`.

Test coverage: tests/all/resource_tags.rs and tests/all/tagging.rs exercise tagging only against live monitors and probes (grep for `DELETING`/`teardown` in the tagging suites returns nothing), so no test pins either answer for a draining probe.

## Steps to Reproduce

1. POST /monitors with {"monitorName":"m","aggregationPeriod":60}; POST /monitors/m/probes with probe {"sourceArn":"arn:aws:ec2:us-west-2:000000000000:subnet/subnet-12345678","destination":"10.0.2.1","protocol":"ICMP","packetSize":56}; note probeId and probeArn.
2. DELETE /monitors/m/probes/{probeId} -> 200; GET /monitors/m/probes/{probeId} now reports state DELETING and a modifiedAt moved to the delete instant.
3. Within the teardown window, PATCH /monitors/m/probes/{probeId} with {"packetSize":1500} -> 404 ResourceNotFoundException (the crate's rule: a draining probe is not a target for mutation).
4. Within the same window, POST /tags/<probeArn> with {"tags":{"k":"v"}} -> 204, and GET /monitors/m/probes/{probeId} now reports the new tag and a modifiedAt moved past the delete instant — the draining probe was mutated after all, by a different route.
5. Wait past the teardown: GET /monitors/m/probes/{probeId} -> 404 and the tag write is gone, so the 204 persisted nothing a caller can read back.

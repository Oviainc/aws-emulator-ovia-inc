Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7cfd583752b909e6f589468ac42594db7887f86f

# StartUpdateSignalMap skips the "monitor deployment in flight" guard its three sibling signal-map mutations apply, so a forced rediscovery replaces the graph under a running deployment

_Filed: 2026-10-28T19:03:06Z_
_Operation: StartUpdateSignalMap | Class: validation_

> _Note: this issue was identified against the code base at commit `7cfd583752b909e6f589468ac42594db7887f86f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The signal-map slice has one helper for "a monitor deployment of this signal map is already running", `SignalMapRecord::monitor_deployment_in_progress`. Three of the four mutations of a signal map resolve against it before writing: `DeleteSignalMap` (through `state::delete_signal_map`), `StartMonitorDeployment` and `StartDeleteMonitorDeployment`. `StartUpdateSignalMap` is the one that does not — `state::start_signal_map_update` checks only `map.lifecycle_in_progress()` and `SignalMapUpdateRefusal` has no variant for a running deployment at all.

So while a deployment is `DEPLOYMENT_IN_PROGRESS`, a `StartUpdateSignalMap` carrying `forceRediscovery: true` (or a changed `discoveryEntryPointArn`, or new `cloudWatchAlarmTemplateGroupIdentifiers` / `eventBridgeRuleTemplateGroupIdentifiers`) is accepted: it rewrites `cloud_watch_alarm_template_group_ids`, `event_bridge_rule_template_group_ids` and the pending `media_resource_map` — i.e. exactly the monitor contents the in-flight deployment is deploying — and when the deployment's own instant comes due it still settles to `DEPLOYMENT_COMPLETE` and is recorded as `LastSuccessfulMonitorDeployment`, now describing a monitor that was replaced underneath it. The handler's own preamble comment says the deployment outcome is materialised "before deciding what this call may do", but nothing in the call then reads it.

## Evidence

The helper and its stated scope — services/service-com-amazonaws-medialive-medialive/src/state.rs:4895-4900:
```
/// Whether a monitor deployment of this signal map is already running, so a
/// second one — of any kind — cannot be started on top of it.
pub fn monitor_deployment_in_progress(&self) -> bool {
    self.monitor_deployment
        .as_ref()
        .is_some_and(MonitorDeploymentRecord::in_progress)
```

The sibling MUTATION that does apply it — services/service-com-amazonaws-medialive-medialive/src/state.rs:3849-3853 and 3875-3886:
```
/// `DeleteSignalMap` "deletes the specified signal map" and documents no
/// precondition, so the only refusals are an identifier that resolves to no
/// signal map and a signal map with work of its own still in flight: its own
/// lifecycle action, or a monitor deployment of it.
...
if map.lifecycle_in_progress() {
    return Err(SignalMapDeleteRefusal::LifecycleRunning { ... });
}
if map.monitor_deployment_in_progress() {
    return Err(SignalMapDeleteRefusal::DeploymentRunning {
        id,
        status: map.monitor_deployment_status().to_string(),
    });
}
```
and services/service-com-amazonaws-medialive-medialive/src/ops/start_monitor_deployment.rs:216-225 / start_delete_monitor_deployment.rs:219-230, both of which branch on `signal_map.monitor_deployment_in_progress()`.

The path that skips it — services/service-com-amazonaws-medialive-medialive/src/state.rs:3930-3937 (the only state check in `start_signal_map_update`):
```
// A lifecycle action of the signal map is asynchronous, so one still in
// flight is already changing the resource this call would change.
if map.lifecycle_in_progress() {
    return Err(SignalMapUpdateRefusal::LifecycleRunning {
        id,
        status: map.status().to_string(),
    });
}
```
and it then writes the monitor's own inputs at services/service-com-amazonaws-medialive-medialive/src/state.rs:4023-4029:
```
if let Some(ids) = alarm_group_ids {
    map.cloud_watch_alarm_template_group_ids = ids;
}
if let Some(ids) = rule_group_ids {
    map.event_bridge_rule_template_group_ids = ids;
}
map.begin_update(discovered, now);
```

The handler comment that names the deployment as an input to the decision — services/service-com-amazonaws-medialive-medialive/src/ops/start_update_signal_map.rs:188-192:
```
// Materialise what the clock had already made due — an earlier
// discovery or update, and a monitor deployment's outcome — before
// deciding what this call may do, so the decision is made against the
// state a reader would already have been shown.
state.settle(now);
```
`SignalMapUpdateRefusal` (state.rs) declares `NotFound`, `LifecycleRunning`, `NameTaken`, `EntryPointMalformed`, `EntryPointNotFound`, `AlarmGroupNotFound`, `RuleGroupNotFound` — no deployment-in-flight variant, and `ops/start_update_signal_map.rs:235-289` maps exactly those seven.

## Steps to Reproduce

1. CreateSignalMap with a discoveryEntryPointArn that resolves, and wait for status CREATE_COMPLETE.
2. StartMonitorDeployment for that signal map — it answers 202 with monitorDeployment.status = DEPLOYMENT_IN_PROGRESS.
3. Immediately StartUpdateSignalMap for the same identifier with forceRediscovery: true (and/or a different cloudWatchAlarmTemplateGroupIdentifiers list).
4. Observed: 202 Accepted; the signal map's template group ids and pending mediaResourceMap are replaced while the deployment is still DEPLOYMENT_IN_PROGRESS, and the deployment later settles to DEPLOYMENT_COMPLETE / lastSuccessfulMonitorDeployment.
5. Expected, by the rule the crate's own `monitor_deployment_in_progress()` helper states and that DeleteSignalMap, StartMonitorDeployment and StartDeleteMonitorDeployment all enforce: the mutation is refused with the modelled ConflictException (409) and nothing is written.
6. Contrast: DeleteSignalMap in the same window is refused with a ConflictException naming the deployment status.

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0c701d5f8ae664c771f5c5cdfd2b2b982b891619

# Idempotent clientToken replay reports a stale state: the replay paths of CreateMonitor/CreateProbe skip settle_monitor

_Filed: 2026-10-25T04:06:39Z_
_Operation: CreateMonitor | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0c701d5f8ae664c771f5c5cdfd2b2b982b891619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every path in this crate that REPORTS a monitor's or probe's `state` first materialises the activation the clock has already made due, by calling `crate::state::ComAmazonawsNetworkmonitorNetworkmonitorState::settle_monitor` (GetMonitor, GetProbe, UpdateMonitor, UpdateProbe) or `settle_monitors_in` (ListMonitors) — except the two idempotency-replay paths.

When `CreateMonitor` is retried with the same `clientToken`, the handler looks the token up and returns `output_for(monitor)` straight from the stored record WITHOUT settling it, so the `state` it reports is the state the monitor was stored in rather than the state it is in at the instant of the call. A monitor created with probes is stored `PENDING` and is owed `ACTIVE` after `ACTIVATION_AFTER`; a retry sent after that instant still answers `state: "PENDING"` while a `GetMonitor`/`ListMonitors` at the very same instant answers `ACTIVE`. Two reads of the same resource at the same moment disagree, and the stale `PENDING` is also left in the store until some other operation settles it.

`CreateProbe`'s replay path has exactly the same defect from the same cause: it returns `output_for(probe)` for the probe the token already minted without calling `settle_monitor`, so a retry reports `state: "PENDING"` for a probe that `GetProbe` reports as `ACTIVE`. One fix (settle before replaying) covers both.

No existing test pins the stale value: the only replay test retries a probe-LESS monitor (stored `INACTIVE`, nothing ever due) and, for the probe retry, compares only `probeId`.

## Evidence

The rule the crate states for itself — `services/service-com-amazonaws-networkmonitor-networkmonitor/src/state.rs:896-903`:
```
    /// Materialise the transition the clock has already made due, for ONE
    /// monitor and its probes.
    ///
    /// This is the ONLY write a reporting operation performs: a monitor whose
    /// `activates_at` has passed was owed its activation at that instant whether
    /// or not anybody called, so a read settles it before reporting.
    pub fn settle_monitor(&mut self, region: &str, monitor_name: &str, now: SystemTime) {
```
Every other reporting path obeys it: `src/ops/get_monitor.rs:200` `store.settle_monitor(&region, &input.monitor_name, now);`, `src/ops/get_probe.rs:190` (same call), `src/ops/update_monitor.rs:222` (same call, comment: "the same write `GetMonitor` performs, owed at its due instant whether or not anybody called"), `src/ops/update_probe.rs:268` (same call), `src/ops/list_monitors.rs:195` `store.settle_monitors_in(&region, now);`.

The replay paths do not — `src/ops/create_monitor.rs:248-258`:
```
        if let Some(token) = input.client_token.as_deref() {
            let replayed = store
                .client_token_record(&region, token)
                ...
            if let Some(monitor) = replayed.and_then(|name| store.monitor(&region, &name)) {
                return Ok(output_for(monitor));
            }
        }
```
and `output_for` reports the stored state verbatim (`src/ops/create_monitor.rs:158`: `state: monitor.state.clone(),`). The same in `src/ops/create_probe.rs:244-255`, ending `return Ok(output_for(probe));` (line 254), with `src/ops/create_probe.rs:162`: `state: Some(probe.state.clone()),`.

That the transition belongs to the clock and not to the read is what the suite itself pins — `tests/all/monitor_and_probe.rs:835-838`: "The `PENDING` -> `ACTIVE` transition is owed to the CLOCK: a monitor that has a probe is `PENDING` straight after the call and `ACTIVE` once its due instant has passed".

Test coverage of the replay: `tests/all/monitor_and_probe.rs:681-692` retries `{"monitorName":"retried","clientToken":"token-1","aggregationPeriod":30}` — a monitor with NO probes, so `activates_at` is `None` and settling is a no-op — and `tests/all/monitor_and_probe.rs:702-711` compares only `probeId` on the probe retry. Settling in the replay paths therefore leaves both assertions as they are.

## Steps to Reproduce

1. CreateMonitor with body {"monitorName":"m1","aggregationPeriod":30,"clientToken":"t1","probes":[{"sourceArn":"arn:aws:ec2:us-east-1:<emulator account>:subnet/subnet-0123456789abcdef0","destination":"10.0.0.1","protocol":"ICMP","packetSize":56}]} — the response reports state "PENDING" (correct, the activation is not due yet).
2. Wait longer than ACTIVATION_AFTER (500 ms).
3. GetMonitor(monitorName="m1") — reports state "ACTIVE".
4. Repeat the identical CreateMonitor call (same clientToken "t1") — it replays, but reports state "PENDING" again, contradicting the GetMonitor taken at the same instant.
5. Same for probes: CreateProbe(monitorName="m1", clientToken="t2", probe={...}); wait >500 ms; GetProbe reports "ACTIVE" while a retry of the identical CreateProbe replays with state "PENDING".

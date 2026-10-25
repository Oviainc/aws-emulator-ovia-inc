Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# DeleteProbe removes the probe synchronously, so the modeled DELETING/DELETED probe states are never observable

_Filed: 2026-10-24T15:31:05Z_
_Operation: DeleteProbe | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: probe deletion on AWS is asynchronous teardown of a network resource. `DeleteProbe` is accepted and the probe stays readable for a window with its state moved into the teardown part of the lifecycle — the modeled `ProbeState` enum carries `DELETING` and `DELETED` exactly for that window (PENDING | ACTIVE | INACTIVE | ERROR | DELETING | DELETED) — and only afterwards does `GetProbe` start answering ResourceNotFoundException. The same window is why the recorded AWS behavior for this path has to poll GetProbe until it 404s instead of asserting the 404 on the first read, and why `GetMonitor` still lists the probe (in DELETING) for a moment after the delete returns.

Actual: the handler removes the probe from the monitor's `probes` map inside the same request (`monitor.probes.remove(&input.probe_id)`), so the very next `GetProbe` answers 404 and `GetMonitor` never lists it again. The store has no notion of a probe being torn down: `crate::state::Probe` tracks only `activates_at`, and the two constants the crate defines for probe states are PENDING/ACTIVE/INACTIVE (`SETTABLE_PROBE_STATES`, `PROBE_STATE_*`). Consequently two of the six modeled `ProbeState` values can never be produced by any response of this emulator, and a client that calls DeleteProbe and then reads the probe to watch it drain (or that treats a 404 as "the probe never existed" rather than "teardown finished") sees a transition AWS does not perform.

Note: this is NOT the refusal guard already filed under the open issue "networkmonitor accepts calls AWS refuses…" (that one is about DeleteProbe being accepted while the probe is still PENDING). This is the post-acceptance lifecycle of an accepted delete.

## Evidence

Spec: `ProbeState` enum in com-amazonaws-networkmonitor-networkmonitor.json declares DELETING and DELETED alongside PENDING/ACTIVE/INACTIVE/ERROR, and `Probe$state`/`GetProbeOutput$state`/`UpdateProbeOutput$state` all target it — values a response can only carry if a deleted probe remains readable while it is being torn down. Well-established AWS behavior: NetworkMonitor probe deletion is asynchronous (the recorded AWS check for this path polls GetProbe until it reports not-found rather than expecting an immediate 404).
Handler: services/service-com-amazonaws-networkmonitor-networkmonitor/src/ops/delete_probe.rs lines ~163-180 — `monitor.probes.remove(&input.probe_id)` then `monitor.clear_activation()`; src/state.rs `Probe` has no teardown/`deletes_at` field and `settle_monitor` only materialises PENDING->ACTIVE.
Tests: tests/all/monitor_and_probe.rs asserts the probe is gone immediately after DeleteProbe (an_unstored_resource_is_reported_as_not_found / the lifecycle round-trip); no test asserts a DELETING window, so nothing covers this.

## Steps to Reproduce

1. CreateMonitor(monitorName="m1", aggregationPeriod=30) -> 200.
2. CreateProbe(monitorName="m1", probe={sourceArn=<subnet arn>, destination="10.0.0.1", protocol="ICMP", packetSize=56}) -> 200; poll GetProbe until state is ACTIVE.
3. DeleteProbe(monitorName="m1", probeId=P) -> 200.
4. Immediately call GetProbe(monitorName="m1", probeId=P). Expected: 200 with state DELETING (and GetMonitor("m1") still listing P) for a short window, converging to 404 ResourceNotFoundException afterwards. Actual: 404 ResourceNotFoundException on the first read and GetMonitor already lists no probes.
5. Confirm no emulator response can ever carry ProbeState DELETING or DELETED.

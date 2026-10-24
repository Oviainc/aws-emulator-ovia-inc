Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad

# The monitor store is not partitioned by Region, so a monitor created through one Region's endpoint is read, listed, updated and deleted through every other

_Filed: 2026-10-24T09:06:18Z_
_Operation: GetMonitor | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: CloudWatch Network Monitor is a regional service. A monitor created through the us-east-1 endpoint exists only in us-east-1: GetMonitor for that monitorName signed for us-west-2 answers ResourceNotFoundException (404), ListMonitors in us-west-2 does not list it, UpdateMonitor/DeleteMonitor/CreateProbe in us-west-2 cannot reach it, and the same monitorName may be created independently in both Regions (each with its own ARN carrying its own Region).

Actual (emulator): the store is a single flat `monitors: BTreeMap<String, Monitor>` keyed by monitorName alone, with no Region dimension, and every read/mutate handler ignores the request Region entirely (`let _ = ctx;`). Consequences observable over HTTP:

* GetMonitor signed for us-west-2 returns a monitor created with us-east-1 credentials — and returns a monitorArn that names us-east-1, i.e. an ARN from a Region the caller did not address, which AWS never does.
* ListMonitors in any Region lists every Region's monitors (same for the probes GetMonitor/GetProbe report).
* UpdateMonitor / DeleteMonitor / CreateProbe / DeleteProbe / UpdateProbe in Region B mutate or destroy a Region A monitor.
* CreateMonitor in Region B for a monitorName already used in Region A is refused with ConflictException, although the two monitors are different resources in AWS.

The Region is already available to every handler (`crate::state::request_region(ctx)`, used today only to mint ARNs in CreateMonitor/CreateProbe); it just is not part of the key or of any lookup.

## Evidence

Spec/well-established semantics: the service is sigv4-signed per Region with per-Region endpoints, and every modeled ARN (MonitorArn `arn:aws:networkmonitor:<region>:<account>:monitor/<name>`, probeArn) carries the Region, so resources are Region-scoped by construction; monitorName uniqueness is per account per Region. Handler/state: `src/state.rs` declares `pub monitors: BTreeMap<String, Monitor>` keyed by monitorName with no region member on `Monitor`; `src/ops/get_monitor.rs` line ~184 `let _ = ctx;` then looks up `store.monitors.get(&input.monitor_name)` (line ~197); `src/ops/list_monitors.rs` line ~160 `let _ = ctx;` and walks `store.monitors.values()`; `src/ops/update_monitor.rs` (~193), `src/ops/delete_monitor.rs` (~141), `src/ops/delete_probe.rs` (~146), `src/ops/get_probe.rs` (~175), `src/ops/update_probe.rs` (~240) all discard `ctx` the same way; `src/ops/create_monitor.rs` line ~214 calls `request_region(ctx)` but only to build the ARN (line ~303), never to key the record. Tests: `tests/all/*` always drive one Region (the fixtures sign a single region, e.g. `tests/all/resource_tags.rs::monitor_arn_in(region, ...)` is used only to build a NON-matching ARN); no test performs a cross-Region read, so this is uncovered. Prior art for this gap class in this repo: the same region-partitioning issue has been filed for keyspaces, mediapackage, managedblockchain, polly, rbin and others.

## Steps to Reproduce

1. With credentials signed for us-east-1, CreateMonitor(monitorName="m1", aggregationPeriod=60) -> 200 with monitorArn arn:aws:networkmonitor:us-east-1:<account>:monitor/m1.
2. With credentials signed for us-west-2, GetMonitor(monitorName="m1"): observe HTTP 200 reporting the monitor, with monitorArn still naming us-east-1; expected HTTP 404 ResourceNotFoundException.
3. With us-west-2 credentials, ListMonitors: observe m1 listed; expected an empty monitors list.
4. With us-west-2 credentials, CreateMonitor(monitorName="m1"): observe HTTP 409 ConflictException; expected HTTP 200 creating an independent us-west-2 monitor whose ARN names us-west-2.
5. With us-west-2 credentials, DeleteMonitor(monitorName="m1") -> 200, then GetMonitor in us-east-1: observe 404, i.e. one Region's endpoint destroyed another Region's monitor.

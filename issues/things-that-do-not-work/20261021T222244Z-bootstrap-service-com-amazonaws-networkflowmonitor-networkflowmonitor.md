Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: aa53e03837fde54d44dc5b021f911ec63548de2f

# Bootstrap the networkflowmonitor emulator

_Filed: 2026-10-21T22:22:44Z_
_Operation: service-com-amazonaws-networkflowmonitor-networkflowmonitor | Class: missing_

> _Note: this issue was identified against the code base at commit `aa53e03837fde54d44dc5b021f911ec63548de2f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 25 operations of NetworkFlowMonitor (restJson1, sigv4 name `networkflowmonitor`) are unmodified scaffolds answering HTTP 501. Implement a FIRST minimal, stateful happy-path slice covering the scope + monitor lifecycle, in this order:

1. **CreateScope** — `POST /scopes`. Input: `targets` (list<TargetResource>, required), `clientToken` (@idempotencyToken), `tags`. Must mint a `scopeId` (uuid) and `scopeArn` (`arn:aws:networkflowmonitor:<region>:<account>:scope/<scopeId>`), persist `targets`/`tags`, and return `status` = `SUCCEEDED` (valid ScopeStatus). Repeating the same `clientToken` must return the same scope, not a second one.
2. **GetScope** — `GET /scopes/{scopeId}`. Must return the stored `scopeId`, `scopeArn`, `status`, `targets` and `tags` exactly as created; unknown id -> `ResourceNotFoundException` (HTTP 404).
3. **CreateMonitor** — `POST /monitors`. Input: `monitorName` (required, pattern `^[a-zA-Z0-9_.-]+$`, 1..255), `localResources` (required), `remoteResources`, `scopeArn` (required), `clientToken`, `tags`. Must reject a `scopeArn` that is not a stored scope (`ValidationException` 400), mint `monitorArn` (`arn:aws:networkflowmonitor:<region>:<account>:monitor/<monitorName>`), store `localResources`/`remoteResources`/`tags`/`scopeArn`, set `createdAt`/`modifiedAt` to now and `monitorStatus` = `ACTIVE`, and return a duplicate name as `ConflictException` (409) unless the `clientToken` matches the prior create.
4. **GetMonitor** — `GET /monitors/{monitorName}`. Must return the persisted monitor (arn, name, status, localResources, remoteResources, createdAt, modifiedAt, tags) — same values CreateMonitor returned; unknown name -> `ResourceNotFoundException` (404).
5. **ListMonitors** — `GET /monitors`. Must list monitors actually in state as `MonitorSummary` items, honour the `monitorStatus` query filter, cap by `maxResults` (1..25) and round-trip an opaque `nextToken` so paging through yields each monitor exactly once.
6. **DeleteMonitor** — `DELETE /monitors/{monitorName}`. Must remove the monitor from state; afterwards GetMonitor -> 404 and ListMonitors no longer includes it. Deleting an unknown name -> `ResourceNotFoundException` (404).

Every one of these must compute its response from persisted state — no canned bodies, no echoing the request, no minting an id that is never stored. A create followed by a read must return what was created, and a delete must be observable by the next reader. Timestamps must be the real stored creation/modification times (UpdateMonitor, when later implemented, bumps `modifiedAt`).

## State Model

In `src/state.rs` (`ComAmazonawsNetworkflowmonitorNetworkflowmonitorState`, reached via `crate::state::service_state(&*self.engine)` and locked):

- `scopes: HashMap<String /*scopeId*/, Scope>` where `Scope { scope_id: String, scope_arn: String, status: ScopeStatus, targets: Vec<TargetResource>, tags: HashMap<String,String>, created_at: SystemTime }`
- `monitors: HashMap<String /*monitorName*/, Monitor>` where `Monitor { monitor_arn: String, monitor_name: String, monitor_status: MonitorStatus, scope_arn: String, local_resources: Vec<MonitorLocalResource>, remote_resources: Vec<MonitorRemoteResource>, tags: HashMap<String,String>, created_at: SystemTime, modified_at: SystemTime }`
- `idempotency: HashMap<String /*clientToken*/, String /*created resource id or name*/>` so repeated `@idempotencyToken` values replay the first result
- `arn_index: HashMap<String /*arn*/, ResourceRef>` mapping monitor/scope ARNs back to their key, so CreateMonitor can validate `scopeArn` (and the tagging ops can later resolve `resourceArn`)

All maps live in the single shared engine so every handler in the crate sees the same store.

## Out of scope for this slice

The query operations (`StartQueryMonitorTopContributors`, `GetQueryStatus*`, `GetQueryResults*`, `StopQuery*`), `UpdateMonitor`, `UpdateScope`, `DeleteScope`, `ListScopes` and the tagging operations stay 501 and will be filed as individual gap issues.

## Done

An integration test that runs the six calls above against one running emulator passes with correct, stateful bodies — not merely 2xx.

## Evidence

`services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/OPERATIONS.md` lists all 25 operations with the input/output members cited above (CreateScope lines 120-126, GetScope 128-134, CreateMonitor 49-55, GetMonitor 57-63, ListMonitors 81-87 incl. @paginated(inputToken=nextToken, outputToken=nextToken, items=monitors) and maxResults range 1..25, DeleteMonitor 73-79) and the per-operation errors (`ResourceNotFoundException` 404, `ConflictException` 409, `ValidationException` 400). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 25 files in `src/ops/` (create_scope.rs, get_scope.rs, create_monitor.rs, get_monitor.rs, list_monitors.rs, delete_monitor.rs among them), so no operation holds any state today. README.md documents the shared-engine state API (`crate::state::service_state`) the slice must use.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `networkflowmonitor` at it.
2. CreateScope with targets=[{targetIdentifier:{targetId:{accountId:"123456789012"},targetType:"ACCOUNT"},region:"us-east-1"}] — expect 200 with a scopeId, a scope ARN and status SUCCEEDED (today: 501).
3. GetScope with the returned scopeId — expect the same scopeId/scopeArn/status and the targets exactly as submitted.
4. CreateMonitor with monitorName="demo-monitor", scopeArn=<arn from step 2>, localResources=[{type:"AWS::EC2::VPC",identifier:"vpc-123"}] — expect 200 with monitorArn, monitorStatus ACTIVE, createdAt/modifiedAt set.
5. GetMonitor monitorName="demo-monitor" — expect exactly the monitor created in step 4 (localResources preserved).
6. ListMonitors with maxResults=1 — expect demo-monitor in `monitors`; page with the returned nextToken and expect no duplicates and eventual exhaustion.
7. DeleteMonitor monitorName="demo-monitor" — expect 200, then GetMonitor returns ResourceNotFoundException (404) and ListMonitors no longer lists it.
8. CreateMonitor with scopeArn="arn:aws:networkflowmonitor:us-east-1:123456789012:scope/does-not-exist" — expect ValidationException (400), not success.

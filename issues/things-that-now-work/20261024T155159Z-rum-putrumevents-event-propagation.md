Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# Telemetry is never copied into the vended CloudWatch Logs group when CwLogEnabled is true

_Filed: 2026-10-24T15:51:59Z_
_Operation: PutRumEvents | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CwLogEnabled` is documented in the model as "This parameter specifies whether RUM sends a copy of this telemetry data to Amazon CloudWatch Logs in your account", and `DataStorage.CwLog.CwLogGroup` is "The name of the log group where the copies are stored". The emulator honours HALF of that contract: `CreateAppMonitor`/`UpdateAppMonitor` check `logs:PutResourcePolicy` and CREATE the vended log group `/aws/vendedlogs/RUMService_<name>` through the CloudWatch Logs emulator (`src/state/cw_log_delivery.rs::provision_log_group`), and `GetAppMonitor` advertises its name. But `PutRumEvents` only appends the batch to RUM's own in-memory buffer (`record_events` in `src/state.rs`) and never delivers anything to that log group.

Expected: after `PutRumEvents` against a monitor whose `CwLogEnabled` is true, a CloudWatch Logs reader (`logs:DescribeLogStreams`, `logs:GetLogEvents` / `logs:FilterLogEvents`) on the advertised `CwLogGroup` finds a copy of the delivered RUM events.

Actual: the log group stays empty forever — it has no log streams and no events, no matter how many batches are delivered. Since `GetAppMonitorData` is still a scaffold, the CloudWatch Logs copy is the ONLY observable path to delivered telemetry, so a workflow of "create monitor with CwLogEnabled -> PutRumEvents -> read the events out of CloudWatch Logs" cannot be reproduced at all.

## Evidence

Spec: `CreateAppMonitorRequest$CwLogEnabled` / `UpdateAppMonitorRequest$CwLogEnabled` and `CwLog$CwLogGroup` in the model (see the doc comment block in `src/ops/create_app_monitor.rs`). Handler: `src/ops/put_rum_events.rs` `handle` resolves the monitor, authorizes, then only calls `record.record_events(...)`; it never touches the logs client, and `cw_log_delivery.rs` exposes only `caller_may_enable_delivery` and `provision_log_group` (a `CreateLogGroup` call) — no `CreateLogStream`/`PutLogEvents`. Test coverage: `tests/all/cw_log_delivery.rs` asserts only that the GROUP exists (`logs:DescribeLogGroups` finds it); nothing asserts that a delivered batch appears in it. The cross-service seam already exists (the logs SDK client built from `ctx` in `cw_log_delivery::logs_client`), so the propagation is simply unwired.

## Steps to Reproduce

1. CreateAppMonitor(Name='shop', Domain='example.com', CwLogEnabled=true) with a caller holding logs:PutResourcePolicy -> 200.
2. GetAppMonitor(Name='shop') -> note DataStorage.CwLog.CwLogGroup, e.g. '/aws/vendedlogs/RUMService_shop...'.
3. PutRumEvents(Id=<the minted Id>, BatchId=<uuid>, AppMonitorDetails={...}, UserDetails={...}, RumEvents=[{Id, Timestamp, Type, Details:'{}'}]) -> 200.
4. With a CloudWatch Logs client: DescribeLogStreams(logGroupName=<that group>) -> emulator reports no streams, and FilterLogEvents/GetLogEvents return no events; AWS stores a copy of the delivered telemetry in that group.

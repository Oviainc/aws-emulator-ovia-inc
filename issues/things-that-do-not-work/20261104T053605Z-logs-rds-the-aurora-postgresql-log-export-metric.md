Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__rds.obs-rds-3-aurora-postgresql-log-export-error-metric-filter-counts-only-error-lines
Issue-Key: logs__rds.obs-rds-3-aurora-postgresql-log-export-error-metric-filter-counts-only-error-lines|_poll_until#1|Timeout|return last
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# logs__rds: the Aurora PostgreSQL log-export metric-filter flow never completes — no SQL can be run against the instance, so no error lines are ever exported to CloudWatch Logs

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
ONE root cause (Cause 1 of 1).

AWS: a cluster created with EnableCloudwatchLogsExports=['postgresql'] (Smithy: CreateDBClusterMessage$EnableCloudwatchLogsExports, LogTypeList) streams the engine's postgresql log to the log group /aws/rds/cluster/<cluster id>/postgresql. The lines in it are produced by the engine as statements are executed against the published endpoint, and a CloudWatch Logs metric filter over that group counts only the ERROR lines.

Emulator: the check never gets that far — it ends with 'Error: Check timed out after 60.0s'. The step it is waiting on is a session against the cluster endpoint: nothing listens on the published address:port (the same absence reported for the rds service), so no statement can be executed, no engine log line can be produced, and nothing is ever delivered to the log group for the metric filter to count. The export configuration is accepted but has no producer behind it.

Why: a client cannot observe any RDS-sourced log data at all — log export, log-based metric filters and alarms built on them are silently empty, which reads as 'no errors occurred' rather than 'no logs exist'.

Fix: the RDS engine data plane that must serve the published cluster endpoint and emit engine log records, and the delivery path from it into the CloudWatch Logs group named by EnableCloudwatchLogsExports, in the rds crate's log-export component.

Traces: logs__rds.obs-rds-3-aurora-postgresql-log-export-error-metric-filter-counts-only-error-lines.

## Evidence
Expected: after running statements that raise errors against the cluster endpoint, logs.filter_log_events on /aws/rds/cluster/<id>/postgresql returns the engine's ERROR lines and the metric filter's datapoint counts exactly those.
Actual: `Error: Check timed out after 60.0s` — the check is still waiting at the step that needs a SQL session against the published endpoint; no log events are ever delivered.

## Steps to Reproduce
1. rds.create_db_cluster(Engine='aurora-postgresql', EnableCloudwatchLogsExports=['postgresql']) and create a writer instance; wait for 'available'.
2. Connect to the cluster endpoint and execute statements that log ERROR lines — the connection never succeeds.
3. logs.describe_log_groups / filter_log_events for /aws/rds/cluster/<id>/postgresql — nothing is delivered, so the metric filter never produces a datapoint.

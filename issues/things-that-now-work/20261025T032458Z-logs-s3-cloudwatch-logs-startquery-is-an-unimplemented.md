Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__s3.xsvc-logs-3-scheduled-query-s3-object-rows-equal-seeded-events logs__s3.xsvc-logs-3-scheduled-query-s3-put-denied-records-destination-error
Issue-Key: logs__s3|StartQuery|NotImplemented
Issue-Commit: 271fde8cc1a42675cd9f2105c7c7bd008dd933ab

# logs__s3: CloudWatch Logs StartQuery is an unimplemented stub, so no scheduled-query result can be written to or denied on S3 (1 cause)

> _Note: this issue was identified against the code base at commit `271fde8cc1a42675cd9f2105c7c7bd008dd933ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQuery

## Description
One root cause.

Cause 1 of 1: CloudWatch Logs StartQuery answers 501 NotImplemented, so neither the success path (query results written to the destination S3 object) nor the refusal path (the destination records a delivery error when the S3 PUT is denied) can run.
AWS: logs#StartQuery accepts a query string over one or more log groups and returns a queryId whose results are then retrievable/deliverable; a scheduled query writes its rows to the configured S3 destination, and when the destination PUT is denied the destination records that error rather than silently succeeding.
Emulator: `An error occurred (NotImplemented) when calling the StartQuery operation: operation StartQuery is scaffolded but not implemented in this emulator`.
Why: every Logs Insights workflow and every scheduled-query-to-S3 delivery is blocked at its first call, so neither the row contents nor the denied-destination error behavior can be observed.
Fix: the StartQuery handler in the cloudwatch-logs service and the scheduled-query delivery path that consumes it. An open issue already names this stub for the control-plane surface — 20261023T113357Z-logs-startquery-stub.md — but it does not carry these two behavioral replay keys, which additionally grade the S3-destination delivery and the denied-destination error.
Traces: logs__s3.xsvc-logs-3-scheduled-query-s3-object-rows-equal-seeded-events, logs__s3.xsvc-logs-3-scheduled-query-s3-put-denied-records-destination-error

## Evidence
`botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartQuery operation: operation StartQuery is scaffolded but not implemented in this emulator` in both checks. Expected: 200 with a queryId, then query rows delivered to the configured S3 object; and, on a denied destination PUT, a recorded destination error.

## Steps to Reproduce
Boot the emulator and replay the suite, re-running logs__s3.xsvc-logs-3-scheduled-query-s3-object-rows-equal-seeded-events and logs__s3.xsvc-logs-3-scheduled-query-s3-put-denied-records-destination-error. Manual: CreateLogGroup, PutLogEvents a few records, then StartQuery over that group.

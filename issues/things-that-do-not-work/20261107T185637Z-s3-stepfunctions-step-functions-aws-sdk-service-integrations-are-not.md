Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__stepfunctions.sfn-redrive-of-a-partly-failed-map-run-reruns-only-the-failed-items
Issue-Key: s3__stepfunctions.sfn-redrive-of-a-partly-failed-map-run-reruns-only-the-failed-items|run#14|AssertionError|assert (first_counters["succeeded"], first_counters["failed"]) == (
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# Step Functions AWS-SDK service integrations are not interpreted: every aws-sdk:s3 Task fails States.Runtime, so a Distributed Map's children all fail

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
One root cause, grounded in a failing check this report does NOT mark ALREADY FILED or OWNED. Open issue 20261106T214711Z-s3-stepfunctions-distributed-map-is-unusable-resultwriter-is.md owns other checks of this service and does not carry this replay key.

Cause 1 of 1: a Task state whose Resource is an AWS-SDK integration ARN is never executed.
AWS: a Task state with Resource "arn:aws:states:::aws-sdk:s3:getObject" calls S3 GetObject with the state's parameters under the execution role, and the state succeeds with the API's output (or fails with the service's own error, e.g. S3.NoSuchKey / S3.AccessDenied). Inside a Distributed Map, each child execution runs its own branch, so a map over N items where some items are expected to fail yields a MapRunStatus with a real succeeded/failed split, and a redrive of that map run re-runs only the previously failed items.
Emulator: the integration is not dispatched. Every child fails with error 'States.Runtime' and cause "state 'Gate': 'arn:aws:states:::aws-sdk:s3:getObject' names an integration with 's3', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it" — an emulator-authored message. All 9 of 9 items therefore fail, the map run counts {succeeded: 0, failed: 9, resultsWritten: 0} where 6 succeeded / 3 failed was required, and the execution ends 'States.ExceedToleratedFailureThreshold' ("9 of 9 items failed, more than this 'Map' state tolerates"). With no partially-failed map run there is nothing for redrive to re-run selectively.
Why: the AWS-SDK integration is the primary way a state machine touches other services, so essentially every real workflow fails with an error AWS never emits; a client's error handling, failure-tolerance thresholds and redrive semantics are all exercised against a fault of the emulator rather than of the workflow.
Fix: the Step Functions Task-state integration dispatcher — resolve arn:aws:states:::aws-sdk:<service>:<action> to the emulator's service handler, call it under the execution role, and surface the service's own error as the state's error/cause.
Traces: s3__stepfunctions.sfn-redrive-of-a-partly-failed-map-run-reruns-only-the-failed-items.

## Evidence
Expected a map run counting 6 succeeded and 3 failed, then a redrive re-running only the 3. Actual: {'succeeded': 0, 'failed': 9, 'total': 9, 'resultsWritten': 0}; execution error 'States.ExceedToleratedFailureThreshold'; each non-gate child failed with error 'States.Runtime' and cause "state 'Gate': 'arn:aws:states:::aws-sdk:s3:getObject' names an integration with 's3', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it".

## Steps to Reproduce
1) Put several objects in a bucket, omitting a few of the keys the map will address.
2) Create a state machine with a Distributed Map whose child branch has a Task with Resource 'arn:aws:states:::aws-sdk:s3:getObject' and a tolerated-failure threshold.
3) StartExecution, then DescribeMapRun and RedriveExecution.

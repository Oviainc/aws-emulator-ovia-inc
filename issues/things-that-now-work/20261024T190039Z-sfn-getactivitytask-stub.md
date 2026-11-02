Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__stepfunctions.metrics-states-stream-activity-metrics-attribute-worker-outcomes-to-the-activity-arn-series cloudwatch__stepfunctions.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves cloudwatch__stepfunctions.obs-sfn-1-redrive-to-succeeded-counts-once-and-second-redrive-rejected stepfunctions.activity-deleted-mid-flight-orphans-its-outstanding-task stepfunctions.activity-heartbeat-timeout-closes-task-before-task-timeout stepfunctions.activity-stopped-execution-refuses-its-outstanding-token stepfunctions.activity-token-consumed-by-failure-refuses-a-later-success stepfunctions.activity-two-queued-tasks-do-not-cross-executions stepfunctions.activity-worker-input-and-reply-round-trip stepfunctions.deep-sfn-2-sfn-jsonata-context-redrive-count-joins-describe-execution stepfunctions.e2e-activity-worker-heartbeat-success-pipeline stepfunctions.error-invalid-output-send-task-success stepfunctions.get-activity-task-long-poll-returns-empty-on-timeout stepfunctions.redrive-execution-preserves-original-history-order stepfunctions.redrive-execution-resumes-from-failed-state stepfunctions.redrive-execution-resumes-from-failure-point stepfunctions.redrive-succeeded-execution-rejected stepfunctions.reject-delete-nonempty-activity-in-use stepfunctions.reject-redrive-nonexistent-execution stepfunctions.send-task-failure-after-timeout-token-invalid stepfunctions.send-task-failure-kms-access-denied stepfunctions.send-task-heartbeat-task-does-not-exist stepfunctions.send-task-success-token-reuse-fails stepfunctions.task-heartbeat-keeps-execution-alive stepfunctions.task-io-base64-blob-round-trip stepfunctions.task-timed-out-error-on-heartbeat-after-window
Issue-Key: sfn|GetActivityTask|NotImplemented
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# Implement sfn GetActivityTask: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetActivityTask

## Description
`GetActivityTask` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/get_activity_task.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

26 recorded unit(s) fail on their `GetActivityTask` call, so each one unblocks only when `GetActivityTask` works: `cloudwatch__sfn.metrics-states-stream-activity-metrics-attribute-worker-outcomes-to-the-activity-arn-series`, `cloudwatch__sfn.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves`, `cloudwatch__sfn.obs-sfn-1-redrive-to-succeeded-counts-once-and-second-redrive-rejected`, `sfn.activity-deleted-mid-flight-orphans-its-outstanding-task`, `sfn.activity-heartbeat-timeout-closes-task-before-task-timeout`, `sfn.activity-stopped-execution-refuses-its-outstanding-token`, `sfn.activity-token-consumed-by-failure-refuses-a-later-success`, `sfn.activity-two-queued-tasks-do-not-cross-executions`, `sfn.activity-worker-input-and-reply-round-trip`, `sfn.deep-sfn-2-sfn-jsonata-context-redrive-count-joins-describe-execution`, `sfn.e2e-activity-worker-heartbeat-success-pipeline`, `sfn.error-invalid-output-send-task-success`, `sfn.get-activity-task-long-poll-returns-empty-on-timeout`, `sfn.redrive-execution-preserves-original-history-order`, `sfn.redrive-execution-resumes-from-failed-state`, `sfn.redrive-execution-resumes-from-failure-point`, `sfn.redrive-succeeded-execution-rejected`, `sfn.reject-delete-nonempty-activity-in-use`, `sfn.reject-redrive-nonexistent-execution`, `sfn.send-task-failure-after-timeout-token-invalid` (+6 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/get_activity_task.rs`. Replaying the corpus, the emulator refused the `GetActivityTask` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetActivityTask operation: operation GetActivityTask is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sfn emulator
Call GetActivityTask and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

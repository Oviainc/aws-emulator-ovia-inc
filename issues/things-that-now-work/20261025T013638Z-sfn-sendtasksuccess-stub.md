Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__stepfunctions.metrics-states-stream-activity-metrics-attribute-worker-outcomes-to-the-activity-arn-series cloudwatch__stepfunctions.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves stepfunctions.activity-deleted-mid-flight-orphans-its-outstanding-task stepfunctions.activity-stopped-execution-refuses-its-outstanding-token stepfunctions.activity-token-consumed-by-failure-refuses-a-later-success stepfunctions.activity-two-queued-tasks-do-not-cross-executions stepfunctions.activity-worker-input-and-reply-round-trip stepfunctions.get-activity-task-blocks-until-execution-schedules-task stepfunctions.reject-delete-nonempty-activity-in-use stepfunctions.send-task-success-token-reuse-fails stepfunctions.task-io-base64-blob-round-trip
Issue-Key: sfn|SendTaskSuccess|NotImplemented
Issue-Commit: e5b04c00ada8052fad6ddec319a50738ff12651d

# Implement sfn SendTaskSuccess: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `e5b04c00ada8052fad6ddec319a50738ff12651d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendTaskSuccess

## Description
`SendTaskSuccess` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/send_task_success.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

11 recorded unit(s) fail on their `SendTaskSuccess` call, so each one unblocks only when `SendTaskSuccess` works: `cloudwatch__sfn.metrics-states-stream-activity-metrics-attribute-worker-outcomes-to-the-activity-arn-series`, `cloudwatch__sfn.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves`, `sfn.activity-deleted-mid-flight-orphans-its-outstanding-task`, `sfn.activity-stopped-execution-refuses-its-outstanding-token`, `sfn.activity-token-consumed-by-failure-refuses-a-later-success`, `sfn.activity-two-queued-tasks-do-not-cross-executions`, `sfn.activity-worker-input-and-reply-round-trip`, `sfn.get-activity-task-blocks-until-execution-schedules-task`, `sfn.reject-delete-nonempty-activity-in-use`, `sfn.send-task-success-token-reuse-fails`, `sfn.task-io-base64-blob-round-trip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/send_task_success.rs`. Replaying the corpus, the emulator refused the `SendTaskSuccess` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the SendTaskSuccess operation: operation SendTaskSuccess is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sfn emulator
Call SendTaskSuccess and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

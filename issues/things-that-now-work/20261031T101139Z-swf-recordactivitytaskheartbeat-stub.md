Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: swf.e2e-workflow-activity-full-lifecycle swf.e2e-workflow-execution-lifecycle swf.record-heartbeat-unknown-task-token-error
Issue-Shape-Id: com.amazonaws.swf#SimpleWorkflowService
Issue-Key: swf|RecordActivityTaskHeartbeat|NotImplemented
Issue-Commit: 3227a483a6739d1d495ce0f6668dabbc01e8fd5a

# Implement swf RecordActivityTaskHeartbeat: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `3227a483a6739d1d495ce0f6668dabbc01e8fd5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RecordActivityTaskHeartbeat

## Description
`RecordActivityTaskHeartbeat` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/record_activity_task_heartbeat.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `RecordActivityTaskHeartbeat` call, so each one unblocks only when `RecordActivityTaskHeartbeat` works: `swf.e2e-workflow-activity-full-lifecycle`, `swf.e2e-workflow-execution-lifecycle`, `swf.record-heartbeat-unknown-task-token-error`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/record_activity_task_heartbeat.rs`. Replaying the corpus, the emulator refused the `RecordActivityTaskHeartbeat` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the RecordActivityTaskHeartbeat operation: operation RecordActivityTaskHeartbeat is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the swf emulator
Call RecordActivityTaskHeartbeat and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

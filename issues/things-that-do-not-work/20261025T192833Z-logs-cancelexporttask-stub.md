Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.cancel-export-task-then-describe-shows-cancelled logs.create-export-task-lifecycle logs.export-task-cancel-ordering logs.export-task-cancel-transitions-to-cancelled logs.export-task-unknown-id-semantics
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: cloudwatch-logs|CancelExportTask|NotImplemented
Issue-Key: logs.create-export-task-lifecycle|run#26|AssertionError|assert cancel_err == ("InvalidOperationException", "The specified export task has already finished"), (
Issue-Commit: 8c118d1d43e9fad6226f410894186130d2b1ac53

# Implement logs CancelExportTask: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `8c118d1d43e9fad6226f410894186130d2b1ac53`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelExportTask

## Description
`CancelExportTask` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/cancel_export_task.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `CancelExportTask` call, so each one unblocks only when `CancelExportTask` works: `logs.cancel-export-task-then-describe-shows-cancelled`, `logs.create-export-task-lifecycle`, `logs.export-task-cancel-ordering`, `logs.export-task-cancel-transitions-to-cancelled`, `logs.export-task-unknown-id-semantics`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/cancel_export_task.rs`. Replaying the corpus, the emulator refused the `CancelExportTask` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CancelExportTask operation: operation CancelExportTask is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the logs emulator
Call CancelExportTask and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.cancel-completed-export-task-fails logs.cancel-export-task-already-completed-fails logs.cancel-export-task-terminal-state-rejected logs.cancel-export-task-then-describe-shows-cancelled logs.create-export-task-lifecycle logs.create-export-task-nonexistent-group-fails logs.create-export-task-overlapping-window-conflict-rejected logs.e2e-export-task-produces-s3-object-with-log-content logs.export-task-cancel-lifecycle logs.export-task-cancel-ordering logs.export-task-cancel-terminal-state-rejected logs.export-task-cancel-transitions-to-cancelled logs.export-task-honors-time-window-boundaries logs.export-task-server-managed-completion logs.export-task-time-range-window-respected logs.export-task-unknown-id-semantics
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: cloudwatch-logs|CreateExportTask|NotImplemented
Issue-Key: logs.create-export-task-nonexistent-group-fails|run#14|AssertionError|assert code == ("ResourceNotFoundException", "The specified log group does not exist."), f"expected ResourceNotFoundException, got {code}"
Issue-Key: logs.create-export-task-overlapping-window-conflict-rejected|run#17|AssertionError|assert code == expected, f"expected {expected!r}, got {code!r}"
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement logs CreateExportTask: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExportTask

## Description
`CreateExportTask` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/create_export_task.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

16 recorded unit(s) fail on their `CreateExportTask` call, so each one unblocks only when `CreateExportTask` works: `logs.cancel-completed-export-task-fails`, `logs.cancel-export-task-already-completed-fails`, `logs.cancel-export-task-terminal-state-rejected`, `logs.cancel-export-task-then-describe-shows-cancelled`, `logs.create-export-task-lifecycle`, `logs.create-export-task-nonexistent-group-fails`, `logs.create-export-task-overlapping-window-conflict-rejected`, `logs.e2e-export-task-produces-s3-object-with-log-content`, `logs.export-task-cancel-lifecycle`, `logs.export-task-cancel-ordering`, `logs.export-task-cancel-terminal-state-rejected`, `logs.export-task-cancel-transitions-to-cancelled`, `logs.export-task-honors-time-window-boundaries`, `logs.export-task-server-managed-completion`, `logs.export-task-time-range-window-respected`, `logs.export-task-unknown-id-semantics`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/create_export_task.rs`. Replaying the corpus, the emulator refused the `CreateExportTask` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateExportTask operation: operation CreateExportTask is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the logs emulator
Call CreateExportTask and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

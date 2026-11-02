Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__stepfunctions.obs-sfn-1-redrive-to-succeeded-counts-once-and-second-redrive-rejected stepfunctions.redrive-execution-preserves-original-history-order stepfunctions.redrive-execution-resumes-from-failed-state stepfunctions.redrive-execution-resumes-from-failure-point stepfunctions.redrive-succeeded-execution-rejected stepfunctions.reject-redrive-nonexistent-execution stepfunctions.send-task-failure-after-timeout-token-invalid
Issue-Key: sfn|SendTaskFailure|NotImplemented
Issue-Commit: e5b04c00ada8052fad6ddec319a50738ff12651d

# Implement sfn SendTaskFailure: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `e5b04c00ada8052fad6ddec319a50738ff12651d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendTaskFailure

## Description
`SendTaskFailure` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/send_task_failure.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `SendTaskFailure` call, so each one unblocks only when `SendTaskFailure` works: `cloudwatch__sfn.obs-sfn-1-redrive-to-succeeded-counts-once-and-second-redrive-rejected`, `sfn.redrive-execution-preserves-original-history-order`, `sfn.redrive-execution-resumes-from-failed-state`, `sfn.redrive-execution-resumes-from-failure-point`, `sfn.redrive-succeeded-execution-rejected`, `sfn.reject-redrive-nonexistent-execution`, `sfn.send-task-failure-after-timeout-token-invalid`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/send_task_failure.rs`. Replaying the corpus, the emulator refused the `SendTaskFailure` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the SendTaskFailure operation: operation SendTaskFailure is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sfn emulator
Call SendTaskFailure and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

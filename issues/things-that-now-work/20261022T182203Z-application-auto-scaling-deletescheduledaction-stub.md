Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling.deleted-scheduled-action-never-fires application-autoscaling.scheduled-action-lifecycle
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-auto-scaling|DeleteScheduledAction|NotImplemented
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Implement application-auto-scaling DeleteScheduledAction: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteScheduledAction

## Description
`DeleteScheduledAction` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/delete_scheduled_action.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `DeleteScheduledAction` call, so each one unblocks only when `DeleteScheduledAction` works: `application-auto-scaling.deleted-scheduled-action-never-fires`, `application-auto-scaling.scheduled-action-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/delete_scheduled_action.rs`. Replaying the corpus, the emulator refused the `DeleteScheduledAction` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteScheduledAction operation: operation DeleteScheduledAction is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the application-auto-scaling emulator
Call DeleteScheduledAction and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

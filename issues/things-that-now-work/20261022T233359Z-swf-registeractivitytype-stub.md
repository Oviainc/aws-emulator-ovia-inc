Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: swf.activity-heartbeat-timeout-triggers-timeout-event swf.activity-result-binary-blob-round-trip swf.activity-result-payload-round-trip swf.activity-task-double-completion-rejected swf.activity-task-heartbeat-timeout-reaping swf.concurrent-activity-pollers-no-duplicate-delivery swf.concurrent-activity-task-terminal-response-race swf.decision-token-used-for-activity-response-rejected swf.delete-non-deprecated-activity-type-rejected swf.deprecate-already-deprecated-activity-type-rejected swf.describe-activity-type-cross-domain-rejected swf.e2e-workflow-activity-full-lifecycle swf.e2e-workflow-execution-lifecycle swf.history-event-order-matches-schedule-order swf.record-heartbeat-unknown-task-token-error swf.respond-activity-completed-after-task-timeout-rejected swf.respond-activity-failed-wrong-token-rejected swf.respond-activity-task-canceled-unknown-token-error swf.respond-activity-task-completed-twice-rejected swf.respond-activity-task-failed-invalid-token-rejected swf.undeprecate-active-activity-type-rejected swf.workflow-history-event-ordering-monotonic
Issue-Shape-Id: com.amazonaws.swf#SimpleWorkflowService
Issue-Key: swf|RegisterActivityType|NotImplemented
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Implement swf RegisterActivityType: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterActivityType

## Description
`RegisterActivityType` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/register_activity_type.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

22 recorded unit(s) fail on their `RegisterActivityType` call, so each one unblocks only when `RegisterActivityType` works: `swf.activity-heartbeat-timeout-triggers-timeout-event`, `swf.activity-result-binary-blob-round-trip`, `swf.activity-result-payload-round-trip`, `swf.activity-task-double-completion-rejected`, `swf.activity-task-heartbeat-timeout-reaping`, `swf.concurrent-activity-pollers-no-duplicate-delivery`, `swf.concurrent-activity-task-terminal-response-race`, `swf.decision-token-used-for-activity-response-rejected`, `swf.delete-non-deprecated-activity-type-rejected`, `swf.deprecate-already-deprecated-activity-type-rejected`, `swf.describe-activity-type-cross-domain-rejected`, `swf.e2e-workflow-activity-full-lifecycle`, `swf.e2e-workflow-execution-lifecycle`, `swf.history-event-order-matches-schedule-order`, `swf.record-heartbeat-unknown-task-token-error`, `swf.respond-activity-completed-after-task-timeout-rejected`, `swf.respond-activity-failed-wrong-token-rejected`, `swf.respond-activity-task-canceled-unknown-token-error`, `swf.respond-activity-task-completed-twice-rejected`, `swf.respond-activity-task-failed-invalid-token-rejected` (+2 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/register_activity_type.rs`. Replaying the corpus, the emulator refused the `RegisterActivityType` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the RegisterActivityType operation: operation RegisterActivityType is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the swf emulator
Call RegisterActivityType and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling.at-action-missed-while-suspended-is-not-replayed-on-resume application-autoscaling.ceiling-only-scheduled-action-scales-the-table-in application-autoscaling.concurrent-scheduled-action-puts-on-one-target-are-all-stored application-autoscaling.deleted-scheduled-action-never-fires application-autoscaling.deregistered-target-scheduled-enforcement-blocked application-autoscaling.describe-dimension-filter-without-a-resource-id-rejected application-autoscaling.describe-filter-lists-above-fifty-entries-rejected application-autoscaling.describe-scheduled-actions-names-resource-and-dimension-intersect application-autoscaling.partial-scheduled-action-update-keeps-schedule-and-replaces-action application-autoscaling.put-scheduled-action-idempotent-upsert application-autoscaling.put-scheduled-action-unknown-rate-unit-rejected application-autoscaling.rate-action-stops-firing-after-its-end-time application-autoscaling.rate-action-waits-for-its-start-time-then-repeats application-autoscaling.scheduled-action-above-registered-maximum-rejected-at-dispatch application-autoscaling.scheduled-action-lifecycle application-autoscaling.scheduled-action-name-is-scoped-to-its-dimension application-autoscaling.suspended-state-upsert-changes-only-the-flags-it-names
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-auto-scaling|PutScheduledAction|NotImplemented
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# Implement application-auto-scaling PutScheduledAction: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScheduledAction

## Description
`PutScheduledAction` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/put_scheduled_action.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

17 recorded unit(s) fail on their `PutScheduledAction` call, so each one unblocks only when `PutScheduledAction` works: `application-auto-scaling.at-action-missed-while-suspended-is-not-replayed-on-resume`, `application-auto-scaling.ceiling-only-scheduled-action-scales-the-table-in`, `application-auto-scaling.concurrent-scheduled-action-puts-on-one-target-are-all-stored`, `application-auto-scaling.deleted-scheduled-action-never-fires`, `application-auto-scaling.deregistered-target-scheduled-enforcement-blocked`, `application-auto-scaling.describe-dimension-filter-without-a-resource-id-rejected`, `application-auto-scaling.describe-filter-lists-above-fifty-entries-rejected`, `application-auto-scaling.describe-scheduled-actions-names-resource-and-dimension-intersect`, `application-auto-scaling.partial-scheduled-action-update-keeps-schedule-and-replaces-action`, `application-auto-scaling.put-scheduled-action-idempotent-upsert`, `application-auto-scaling.put-scheduled-action-unknown-rate-unit-rejected`, `application-auto-scaling.rate-action-stops-firing-after-its-end-time`, `application-auto-scaling.rate-action-waits-for-its-start-time-then-repeats`, `application-auto-scaling.scheduled-action-above-registered-maximum-rejected-at-dispatch`, `application-auto-scaling.scheduled-action-lifecycle`, `application-auto-scaling.scheduled-action-name-is-scoped-to-its-dimension`, `application-auto-scaling.suspended-state-upsert-changes-only-the-flags-it-names`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/put_scheduled_action.rs`. Replaying the corpus, the emulator refused the `PutScheduledAction` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutScheduledAction operation: operation PutScheduledAction is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the application-auto-scaling emulator
Call PutScheduledAction and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

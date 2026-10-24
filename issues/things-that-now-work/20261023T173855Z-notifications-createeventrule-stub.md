Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: notifications.cloudwatch-alarm-state-change-delivers-notification-event notifications.create-event-rule-nonexistent-configuration-not-found notifications.delete-event-rule-removes-it notifications.event-rule-creating-transitions-to-active notifications.event-rule-update-regions-persisted notifications.list-event-rules-reflects-created-rule
Issue-Shape-Id: com.amazonaws.notifications#Notifications
Issue-Key: notifications|CreateEventRule|NotImplemented
Issue-Commit: a91eb31cf888327ed8ad916c03309e3893f84f70

# Implement notifications CreateEventRule: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a91eb31cf888327ed8ad916c03309e3893f84f70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventRule

## Description
`CreateEventRule` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-notifications-notifications/src/ops/create_event_rule.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `CreateEventRule` call, so each one unblocks only when `CreateEventRule` works: `notifications.cloudwatch-alarm-state-change-delivers-notification-event`, `notifications.create-event-rule-nonexistent-configuration-not-found`, `notifications.delete-event-rule-removes-it`, `notifications.event-rule-creating-transitions-to-active`, `notifications.event-rule-update-regions-persisted`, `notifications.list-event-rules-reflects-created-rule`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-notifications-notifications/src/ops/create_event_rule.rs`. Replaying the corpus, the emulator refused the `CreateEventRule` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateEventRule operation: operation CreateEventRule is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the notifications emulator
Call CreateEventRule and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

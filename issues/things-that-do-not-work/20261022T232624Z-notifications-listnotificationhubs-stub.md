Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: notifications.associate-channel-duplicate-association-is-idempotent notifications.associate-channel-invalid-arn-pattern-rejected notifications.associate-channel-then-list-channels-shows-association notifications.cloudwatch-alarm-state-change-delivers-notification-event notifications.concurrent-associate-channel-exactly-one-wins notifications.concurrent-event-rule-creation-yields-distinct-rules notifications.create-event-rule-invalid-source-pattern-rejected notifications.create-event-rule-nonexistent-configuration-not-found notifications.create-notification-configuration-invalid-name-pattern-rejected notifications.delete-event-rule-removes-it notifications.disassociate-channel-not-associated-rejected notifications.disassociate-channel-removes-association-and-allows-reassociation notifications.event-rule-creating-transitions-to-active notifications.event-rule-update-regions-persisted notifications.get-event-rule-not-found notifications.get-notification-configuration-invalid-arn-validation notifications.list-event-rules-reflects-created-rule notifications.list-notification-configurations-filters-by-status notifications.notification-configuration-lifecycle-create-active-delete notifications.notification-hub-register-active-deregister-removes-it notifications.tag-resource-exceeds-max-tags-rejected notifications.tag-resource-list-tags-untag-roundtrip notifications.update-notification-configuration-changes-description
Issue-Shape-Id: com.amazonaws.notifications#Notifications
Issue-Key: notifications|ListNotificationHubs|NotImplemented
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Implement notifications ListNotificationHubs: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListNotificationHubs

## Description
`ListNotificationHubs` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-notifications-notifications/src/ops/list_notification_hubs.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

23 recorded unit(s) fail on their `ListNotificationHubs` call, so each one unblocks only when `ListNotificationHubs` works: `notifications.associate-channel-duplicate-association-is-idempotent`, `notifications.associate-channel-invalid-arn-pattern-rejected`, `notifications.associate-channel-then-list-channels-shows-association`, `notifications.cloudwatch-alarm-state-change-delivers-notification-event`, `notifications.concurrent-associate-channel-exactly-one-wins`, `notifications.concurrent-event-rule-creation-yields-distinct-rules`, `notifications.create-event-rule-invalid-source-pattern-rejected`, `notifications.create-event-rule-nonexistent-configuration-not-found`, `notifications.create-notification-configuration-invalid-name-pattern-rejected`, `notifications.delete-event-rule-removes-it`, `notifications.disassociate-channel-not-associated-rejected`, `notifications.disassociate-channel-removes-association-and-allows-reassociation`, `notifications.event-rule-creating-transitions-to-active`, `notifications.event-rule-update-regions-persisted`, `notifications.get-event-rule-not-found`, `notifications.get-notification-configuration-invalid-arn-validation`, `notifications.list-event-rules-reflects-created-rule`, `notifications.list-notification-configurations-filters-by-status`, `notifications.notification-configuration-lifecycle-create-active-delete`, `notifications.notification-hub-register-active-deregister-removes-it` (+3 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-notifications-notifications/src/ops/list_notification_hubs.rs`. Replaying the corpus, the emulator refused the `ListNotificationHubs` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListNotificationHubs operation: operation ListNotificationHubs is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the notifications emulator
Call ListNotificationHubs and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

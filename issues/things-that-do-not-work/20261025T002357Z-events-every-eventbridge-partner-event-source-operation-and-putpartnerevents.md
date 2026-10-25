Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.activate-deactivate-event-source-state-transition events.describe-event-source-nonexistent-not-found events.activate-event-source-nonexistent-not-found events.deactivate-event-source-nonexistent-not-found events.concurrent-activate-event-source-conflict-detected events.e2e-partner-event-source-activation-to-ingestion-flow events.partner-event-source-lifecycle-create-rejected-non-partner events.delete-partner-event-source-disabled-operation-blocked events.describe-partner-event-source-nonexistent-not-found events.list-partner-event-sources-disabled-account-denied events.list-partner-event-source-accounts-unknown-source-not-found events.put-partner-events-non-partner-account-denied
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.activate-deactivate-event-source-state-transition|run#7|AssertionError|assert describe_err == not_described, (
Issue-Key: events.describe-event-source-nonexistent-not-found|run#9|AssertionError|assert err == ("ResourceNotFoundException", f"Event source {bogus} does not exist."), (
Issue-Key: events.activate-event-source-nonexistent-not-found|run#10|AssertionError|assert err == expected, (
Issue-Key: events.deactivate-event-source-nonexistent-not-found|run#10|AssertionError|assert err == expected, (
Issue-Key: events.concurrent-activate-event-source-conflict-detected|run#12|AssertionError|assert create_err == expected_create, (
Issue-Key: events|CreatePartnerEventSource|NotImplemented
Issue-Key: events.partner-event-source-lifecycle-create-rejected-non-partner|run#8|AssertionError|assert create_err == refused, f"expected {refused!r}, got {create_err!r}"
Issue-Key: events.delete-partner-event-source-disabled-operation-blocked|run#10|AssertionError|assert err == expected, f"expected {expected}, got {err}"
Issue-Key: events.describe-partner-event-source-nonexistent-not-found|run#4|AssertionError|assert (code, message) == ("ValidationException", _not_allowlisted(bogus)), repr((code, message))
Issue-Key: events.list-partner-event-sources-disabled-account-denied|run#9|AssertionError|assert e == ("ValidationException", _not_allowlisted(prefix)), e
Issue-Key: events.list-partner-event-source-accounts-unknown-source-not-found|run#9|AssertionError|assert err == ("ValidationException", _not_allowlisted(bogus)), err
Issue-Key: events|PutPartnerEvents|NotImplemented
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# Every EventBridge partner-event-source operation and PutPartnerEvents is an unimplemented scaffold stub (9 causes)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ActivateEventSource

## Description
The whole partner-event-source surface of EventBridge answers `NotImplemented` ("operation <Op> is scaffolded but not implemented in this emulator"). Nine distinct operation handlers are involved, so there are nine causes; each check below is refused on a different one.

Cause 1 of 9 — DescribeEventSource. AWS: Smithy `DescribeEventSource` returns Arn/CreatedBy/CreationTime/ExpirationTime/Name/State for a shared partner source and answers ResourceNotFoundException/400 for a source that was never shared. Emulator: NotImplemented. Traces: events.activate-deactivate-event-source-state-transition.

Cause 2 of 9 — ActivateEventSource. AWS: Smithy `ActivateEventSource` takes a Name matching `^aws\.partner(/[\.\-_A-Za-z0-9]+){2,}$` and returns Unit; its modeled errors are ConcurrentModificationException, InternalException, InvalidStateException, OperationDisabledException, ResourceNotFoundException — a never-shared source is ResourceNotFoundException/400. Emulator: NotImplemented. Traces: events.activate-event-source-nonexistent-not-found.

Cause 3 of 9 — DeactivateEventSource. AWS: same shape and error set as ActivateEventSource; a never-shared source is ResourceNotFoundException/400. Emulator: NotImplemented. Traces: events.deactivate-event-source-nonexistent-not-found.

Cause 4 of 9 — CreatePartnerEventSource. AWS: Smithy `CreatePartnerEventSource` creates the partner source and, for a caller not entitled to the namespace, refuses with a validation error. Emulator: NotImplemented. Traces: events.concurrent-activate-event-source-conflict-detected.

Cause 5 of 9 — DeletePartnerEventSource. AWS: modeled with OperationDisabledException for an account that is not an APN partner. Emulator: NotImplemented. Traces: events.delete-partner-event-source-disabled-operation-blocked.

Cause 6 of 9 — DescribePartnerEventSource. AWS: returns Arn/Name, ResourceNotFoundException/400 for an unknown source. Emulator: NotImplemented. Traces: events.describe-partner-event-source-nonexistent-not-found.

Cause 7 of 9 — ListPartnerEventSources. AWS: returns PartnerEventSources + NextToken, and OperationDisabledException for a non-partner account. Emulator: NotImplemented. Traces: events.list-partner-event-sources-disabled-account-denied.

Cause 8 of 9 — ListPartnerEventSourceAccounts. AWS: returns PartnerEventSourceAccounts + NextToken, ResourceNotFoundException/400 for an unknown source. Emulator: NotImplemented. Traces: events.list-partner-event-source-accounts-unknown-source-not-found.

Cause 9 of 9 — PutPartnerEvents. AWS: Smithy `PutPartnerEvents` takes Entries (1-20) and returns FailedEntryCount + Entries; its only client error is OperationDisabledException/400, which is what a non-partner account must get. Emulator: NotImplemented. Traces: events.put-partner-events-non-partner-account-denied.

Why (all nine): a client integrating a partner source cannot create, share, describe, list, activate, deactivate or publish to one, and cannot distinguish "not entitled" (OperationDisabledException/ValidationException) from "does not exist" (ResourceNotFoundException) — every one of those outcomes collapses to the same non-modeled `NotImplemented`.

Fix: the nine operation handlers under services/service-com-amazonaws-eventbridge-awsevents/src/ops/ (describe_event_source.rs, activate_event_source.rs, deactivate_event_source.rs, create_partner_event_source.rs, delete_partner_event_source.rs, describe_partner_event_source.rs, list_partner_event_sources.rs, list_partner_event_source_accounts.rs, put_partner_events.rs) and the partner-source registry they share.

## Evidence
Replayed, all HTTP 400 code `NotImplemented`:
- "expected ('ResourceNotFoundException', 'Event source aws.partner/<ns>/testnamespace/testevent does not exist.') describing an unshared source, got ('NotImplemented', 'operation DescribeEventSource is scaffolded but not implemented in this emulator')"
- "expected ('ResourceNotFoundException', 'Event source aws.partner/sim2real.test/<n> or event bus aws.partner/sim2real.test/<n> does not exist.') activating a never-shared partner source, got ('NotImplemented', 'operation ActivateEventSource is scaffolded but not implemented in this emulator')"
- same for DeactivateEventSource.
- "expected the APN-partner-namespace rejection ('ValidationException', 'Caller is not whitelisted for the event source partner namespace aws.partner/<ns>'), got ('NotImplemented', 'operation CreatePartnerEventSource is scaffolded but not implemented in this emulator')"
- DeletePartnerEventSource, DescribePartnerEventSource, ListPartnerEventSources, ListPartnerEventSourceAccounts, PutPartnerEvents: "operation <Op> is scaffolded but not implemented in this emulator".

Expected: the modeled output shape with HTTP 200, or 400 with the operation's own modeled error code (ResourceNotFoundException / OperationDisabledException / ValidationException). `NotImplemented` appears in no partner-source operation's error list.

## Steps to Reproduce
1. ActivateEventSource(Name="aws.partner/example.com/test/src") -> 400 NotImplemented instead of 400 ResourceNotFoundException.
2. DeactivateEventSource with the same name -> 400 NotImplemented.
3. DescribeEventSource / DescribePartnerEventSource with the same name -> 400 NotImplemented.
4. CreatePartnerEventSource(Name="aws.partner/example.com/test/src", Account=<acct>) -> 400 NotImplemented instead of the not-entitled validation refusal.
5. DeletePartnerEventSource / ListPartnerEventSources / ListPartnerEventSourceAccounts -> 400 NotImplemented.
6. PutPartnerEvents(Entries=[{Source:"aws.partner/example.com/test/src", DetailType:"t", Detail:"{}"}]) -> 400 NotImplemented instead of 400 OperationDisabledException.

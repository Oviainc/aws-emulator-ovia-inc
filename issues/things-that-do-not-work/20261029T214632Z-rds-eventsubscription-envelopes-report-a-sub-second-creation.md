Issue-Source: trace-sweep-divergence
Issue-Trace: rds-event-subscription-mutation-envelopes
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-event-subscription-mutation-envelopes
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|CreateEventSubscription|2xx->2xx|$.CreateEventSubscriptionResponse.CreateEventSubscriptionResult.EventSubscription.SubscriptionCreationTime
Issue-Key: rds|AddSourceIdentifierToSubscription|2xx->2xx|$.AddSourceIdentifierToSubscriptionResponse.AddSourceIdentifierToSubscriptionResult.EventSubscription.SubscriptionCreationTime
Issue-Key: rds|ModifyEventSubscription|2xx->2xx|$.ModifyEventSubscriptionResponse.ModifyEventSubscriptionResult.EventSubscription.Status
Issue-Key: rds|ModifyEventSubscription|2xx->2xx|$.ModifyEventSubscriptionResponse.ModifyEventSubscriptionResult.EventSubscription.SubscriptionCreationTime
Issue-Key: rds|RemoveSourceIdentifierFromSubscription|2xx->2xx|$.RemoveSourceIdentifierFromSubscriptionResponse.RemoveSourceIdentifierFromSubscriptionResult.EventSubscription.Status
Issue-Key: rds|RemoveSourceIdentifierFromSubscription|2xx->2xx|$.RemoveSourceIdentifierFromSubscriptionResponse.RemoveSourceIdentifierFromSubscriptionResult.EventSubscription.SubscriptionCreationTime
Issue-Key: rds|DeleteEventSubscription|2xx->2xx|$.DeleteEventSubscriptionResponse.DeleteEventSubscriptionResult.EventSubscription.SubscriptionCreationTime
Issue-Commit: 9919ffd222ff0dfbfa1095fff748926e9311733c

# RDS EventSubscription envelopes report a sub-second creation time and a transient status

> _Note: this issue was identified against the code base at commit `9919ffd222ff0dfbfa1095fff748926e9311733c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSubscription

## Description
Every operation that serializes an EventSubscription emits SubscriptionCreationTime in a form the recorded shape rejects, and the mutating operations report the subscription's transient status instead of the settled one AWS returns. 5 causes.

Shared AWS behavior: recorded behavior only — SubscriptionCreationTime is a String member whose recorded value matches the RDS event-subscription timestamp shape (the recording asserts the shape, not the instant); and Modify/RemoveSourceIdentifier responses report Status 'active'.
Emulator: SubscriptionCreationTime is an RFC3339 instant with microsecond precision, which fails the recorded shape assertion; and the mutating responses report Status 'modifying'.
Why: a client parsing SubscriptionCreationTime with AWS's format fails, and a client that polls on Status sees a value AWS never returns from these calls, so it either loops or takes the wrong branch. The failing comparison is on the value's SHAPE, not on the particular instant — a differing clock reading alone would not be a finding.
Fix: the EventSubscription serializer's timestamp formatting, and the Modify/AddSourceIdentifier/RemoveSourceIdentifier handlers' status transition (settle before responding).

Cause 1 of 5: CreateEventSubscription — SubscriptionCreationTime fails the recorded shape. Traces: rds-event-subscription-mutation-envelopes#task#3.
Cause 2 of 5: AddSourceIdentifierToSubscription — same timestamp divergence. Traces: ...#task#7.
Cause 3 of 5: ModifyEventSubscription — same timestamp divergence plus Status 'modifying' where the recording has 'active'. Traces: ...#task#10.
Cause 4 of 5: RemoveSourceIdentifierFromSubscription — same timestamp divergence plus Status 'modifying' where the recording has 'active'. Traces: ...#task#11.
Cause 5 of 5: DeleteEventSubscription — same timestamp divergence in the deleted-subscription envelope. Traces: ...#task#12.

## Evidence
Replay cluster details (status 200 matched in all five):
1. ...#task#3 CreateEventSubscription: EventSubscription.SubscriptionCreationTime value_mismatch — expected <shaped-not-asserted> (i.e. the recorded shape), got '2026-10-29T21:33:43.917359Z'.
2. ...#task#7 AddSourceIdentifierToSubscription: same SubscriptionCreationTime shape failure.
3. ...#task#10 ModifyEventSubscription: same, plus EventSubscription.Status expected 'active' got 'modifying'.
4. ...#task#11 RemoveSourceIdentifierFromSubscription: same, plus Status expected 'active' got 'modifying'.
5. ...#task#12 DeleteEventSubscription: same SubscriptionCreationTime shape failure.
(The instant shown is this run's clock; the divergence the replay reports is that the value does not match the recorded SubscriptionCreationTime shape.)

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Re-run rds-event-subscription-mutation-envelopes#task#3 and the steps in trace_ids; the fixture creates a subscription, adds/removes a source identifier, modifies and deletes it, and each response's EventSubscription envelope is diffed.

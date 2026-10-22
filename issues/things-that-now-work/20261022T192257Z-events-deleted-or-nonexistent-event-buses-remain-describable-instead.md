Issue-Source: trace-sweep-divergence
Issue-Trace: events-custom-event-bus-config-update
Issue-Step: teardown#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: events-custom-event-bus-config-update events-event-bus-resource-policy-cross-account-permission events-eventbus-create-describe-update-delete events-eventbus-lifecycle events-event-bus-update-lifecycle events-eventbus-logconfig-toggle events-eventbus-multi-isolation events-update-event-bus-description-lifecycle events-update-event-bus-errors
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|DescribeEventBus|4xx->2xx|$
Issue-Key: events|DescribeEventBus|4xx->2xx|$.__type
Issue-Key: events|DescribeEventBus|4xx->2xx|$.message
Issue-Key: events|DescribeEventBus|4xx->2xx|status
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# Deleted or nonexistent event buses remain describable instead of ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEventBus

## Description
DeleteEventBus reports success but the bus remains readable, and DescribeEventBus on a name that was never created also answers 200, so the emulator has no not-found path for event buses at all. Two causes; a fixer must close BOTH.

Cause 1 of 2 - DescribeEventBus after DeleteEventBus returns 200 with the deleted bus.
AWS: Smithy DescribeEventBus models ResourceNotFoundException (client error, httpError 400). After DeleteEventBus succeeds, DescribeEventBus for that name fails with HTTP 400 and __type ResourceNotFoundException (recorded message example: 'Event bus <name> does not exist.'). The fixture polls for this transition, so eventual consistency is already tolerated.
Emulator: HTTP 200 with the full bus record ($.Name, $.Arn, $.CreationTime, $.LastModifiedTime) - no error code at all - and it never converges: the poll gave up after 15 attempts.
Why: teardown/recreate cycles break. A client that deletes a bus and recreates it sees the stale object; a client that uses DescribeEventBus as an existence probe concludes a deleted bus is live, and nothing ever observes the delete taking effect. Because the delete does not take effect, downstream steps in the same fixture also run against state that should be gone.
Fix: DeleteEventBus must remove the bus from the events state store, and DescribeEventBus must return 400 / ResourceNotFoundException for an absent name.
Traces: events-custom-event-bus-config-update#teardown#7; also events-event-bus-resource-policy-cross-account-permission#teardown#10, events-eventbus-create-describe-update-delete#teardown#6, events-eventbus-lifecycle#teardown#6.

Cause 2 of 2 - DescribeEventBus on an absent/never-created bus returns 200.
AWS: same modeled error - DescribeEventBus on a name with no bus returns HTTP 400 with __type ResourceNotFoundException.
Emulator: HTTP 200, $.__type absent and $.message absent; no error is produced. This cluster also covers an error-path fixture step (events-update-event-bus-errors#task#7) whose whole purpose is the not-found response, which shows the gap is in the lookup, not only in delete.
Why: negative tests cannot be written against the emulator, and client code that branches on ResourceNotFoundException to decide 'create vs reuse' takes the wrong branch every time.
Fix: event-bus lookup in the DescribeEventBus handler must raise ResourceNotFoundException (400) on a miss rather than fabricating a record.
Traces: events-event-bus-update-lifecycle#teardown#5; also events-eventbus-logconfig-toggle#teardown#6, events-eventbus-multi-isolation#teardown#9, events-eventbus-multi-isolation#teardown#11, events-update-event-bus-description-lifecycle#teardown#8, events-update-event-bus-errors#task#7.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator.

Cause 1 - events-custom-event-bus-config-update#teardown#7 DescribeEventBus: status expected 400, got 200; $.__type missing (expected 'ResourceNotFoundException', got None); $.message missing (expected 'Event bus <bus-name> does not exist.', got None); poll: error code None not in ['ResourceNotFoundException'] after 15 attempt(s) - actual body {'Name': '<bus-name>', 'Arn': 'arn:aws:events:us-west-2:<acct>:event-bus/<bus-name>', 'CreationTime': ..., 'LastModifiedTime': ...}. 4 steps affected.
Cause 2 - events-event-bus-update-lifecycle#teardown#5 DescribeEventBus: status expected 400, got 200; $.__type missing (expected 'ResourceNotFoundException'); $.message missing (expected 'Event bus <bus-name> does not exist.'). 6 steps affected, including events-update-event-bus-errors#task#7.

Model evidence: Smithy DescribeEventBus errors = [InternalException (server, 500), ResourceNotFoundException (client, 400)]. The 400 status and the ResourceNotFoundException code are the requirement; the quoted message text is the recorded example (bus names are environment-specific).

## Steps to Reproduce
1. Clone the repo, boot the emulator, replay the wire corpus (replay_wire_corpus).
2. CreateEventBus a custom bus, then DeleteEventBus it (succeeds).
3. DescribeEventBus the deleted name: the emulator answers 200 with the full bus record instead of 400 / ResourceNotFoundException, and keeps doing so on repeated polls.
4. DescribeEventBus a name that was never created: again 200 instead of 400 / ResourceNotFoundException.
5. Re-replay events-custom-event-bus-config-update#teardown#7 and events-event-bus-update-lifecycle#teardown#5 to confirm both causes are closed.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.put-events-endpoint-id-delivers-to-primary-and-replicates-to-secondary events.put-events-endpoint-id-with-unrelated-event-bus-name-rejected events.put-targets-fifo-sqs-dead-letter-config-rejected events.put-targets-sns-fifo-topic-target-rejected events.sqs-target-message-group-id-json-path-resolves-per-event events.test-event-pattern-anything-but-mixed-string-number-list-rejected events.create-api-destination-invalid-endpoint-pattern-rejected
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|PutEvents|UnknownService
Issue-Key: events.put-targets-fifo-sqs-dead-letter-config-rejected|_error#20|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: events.put-targets-sns-fifo-topic-target-rejected|_error#16|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: events.sqs-target-message-group-id-json-path-resolves-per-event|run#20|AssertionError|assert first == "sim2real-group-alpha", (
Issue-Key: events.test-event-pattern-anything-but-mixed-string-number-list-rejected|_error#4|AssertionError|raise AssertionError(f"expected the pattern to be refused; it SUCCEEDED: {result!r}")
Issue-Key: events.create-api-destination-invalid-endpoint-pattern-rejected|run#8|AssertionError|assert (code, message) == expected, (
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# EventBridge: 7 net-new causes — global-endpoint PutEvents unroutable, FIFO/SNS-FIFO target refusals missing, SqsParameters.MessageGroupId JSON path not resolved, anything-but mixed-list accepted, api-destination endpoint pattern unanchored

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Seven independent EventBridge defects, all grounded in failing behavioral checks under the `events` heading. Numbered so each can be closed separately.

Cause 1 of 7: PutEvents with an EndpointId cannot be dispatched at all.
AWS: PutEvents accepts an `EndpointId` (Smithy: PutEventsRequest.EndpointId, used with a global endpoint) and delivers the entry to the endpoint's primary Region's bus, replicating to the secondary; when the named bus is unrelated to the endpoint it refuses the request. Recorded behavior shows the SDK resolving a global-endpoint host for PutEvents.
Emulator: the call never reaches an EventBridge handler. botocore raises `ClientError (UnknownService) when calling the PutEvents operation: no emulated service is registered for 'awsevents'` — the emulator's service registry is keyed so the global-endpoint PutEvents route resolves to the sdkId token `awsevents`, for which nothing is registered. Status/code: the client sees an `UnknownService` error instead of either a 200 PutEventsResponse or EventBridge's own modelled refusal.
Why: a client using a global endpoint for multi-Region failover gets a non-AWS error code (`UnknownService`) from the SDK, so neither the happy path nor the documented rejection can be exercised; failover never works.
Fix: the emulator's endpoint/service registration for EventBridge (the `awsevents` sdkId alias must map to the same handler set as `events`/`eventbridge`), plus the PutEvents handler's EndpointId routing.
Traces: events.put-events-endpoint-id-delivers-to-primary-and-replicates-to-secondary; events.put-events-endpoint-id-with-unrelated-event-bus-name-rejected. The SAME root cause also breaks cloudwatch__events__route53__sqs.cloudwatch-alarm-breach-fails-the-global-endpoint-over-to-secondary, filed separately because it is listed under another service heading.

Cause 2 of 7: PutTargets accepts a FIFO SQS queue as a target's dead-letter queue.
AWS: Smithy Target.DeadLetterConfig -> DeadLetterConfig.Arn is documented as 'The ARN of the SQS queue specified as the target for the dead-letter queue'; AWS refuses a FIFO queue there with HTTP 400 `ValidationException` and the message `SQS FIFO is not supported by Dead Letter Queues.` (entirely fixed text — no request values), and leaves the rule's target list unchanged.
Emulator: returns HTTP 200 with `{"FailedEntryCount": 0, "FailedEntries": []}` and registers the target.
Why: a client that mis-configures a FIFO DLQ gets no feedback and silently loses undeliverable events.
Fix: PutTargets' input validation (DeadLetterConfig.Arn must be a standard queue).
Traces: events.put-targets-fifo-sqs-dead-letter-config-rejected.

Cause 3 of 7: PutTargets accepts a FIFO SNS topic as a target.
AWS: refuses a `*.fifo` SNS topic ARN in Target.Arn with HTTP 400 `ValidationException` and the message `SNS FIFO is not a supported service for a target.` (entirely fixed text), leaving the rule's targets unchanged; a standard topic is accepted.
Emulator: returns HTTP 200 with `FailedEntryCount: 0` and registers the FIFO topic target.
Why: the rule looks wired but EventBridge cannot publish to a FIFO topic, so matched events vanish with no rejection at configuration time.
Fix: PutTargets' target-ARN validation.
Traces: events.put-targets-sns-fifo-topic-target-rejected.

Cause 4 of 7: SqsParameters.MessageGroupId holding a JSON path is not evaluated per event.
AWS: Smithy SqsParameters.MessageGroupId (@length 0..100) is 'The ID of the message group to use as the target'; when it holds a JSON path such as `$.detail.group` EventBridge resolves it against each matched event, so the delivered FIFO message's MessageGroupId is that event's own `detail.group` value.
Emulator: delivers the message with MessageGroupId set to the literal template text `$.detail.group` instead of the resolved value (recorded: expected the event's own detail.group value, got the literal path).
Why: every event lands in one message group named `$.detail.group`, so FIFO ordering/parallelism keyed on an event field collapses to a single serialized group.
Fix: the SQS target delivery path's SqsParameters resolution (resolve a JSON-path MessageGroupId against the event before SendMessage).
Traces: events.sqs-target-message-group-id-json-path-resolves-per-event.

Cause 5 of 7: TestEventPattern accepts an `anything-but` list that mixes strings and numbers.
AWS: refuses the pattern with HTTP 400 `InvalidEventPatternException` and the message `Event pattern is not valid. Reason: Inside anything but list, either all values are number or string, mixed type is not supported` followed by ` at [Source: (String)"<the pattern body>"; line: 1, column: <offset>]`. Fixed text is the 'Event pattern is not valid. Reason: ...' sentence and the ' at [Source: (String)"..."; line: 1, column: N]' frame; the variable parts are the caller's pattern body and the column offset of the offending token. All-string and all-number lists are accepted.
Emulator: returns HTTP 200 `{"Result": false}` — it treats the malformed pattern as a valid non-match.
Why: a client gets a silent `false` instead of a validation error, so a pattern that can never match is deployed on a rule and the rule never fires.
Fix: the event-pattern parser/validator shared by TestEventPattern and PutRule (anything-but element type homogeneity).
Traces: events.test-event-pattern-anything-but-mixed-string-number-list-rejected.

Cause 6 of 7: CreateApiDestination's invocationEndpoint pattern refusal omits the pattern's anchors.
AWS: refuses a malformed endpoint with `ValidationException` and the message `1 validation error detected: Value '<endpoint>' at 'invocationEndpoint' failed to satisfy constraint: Member must satisfy regular expression pattern: ^((%[0-9A-Fa-f]{2}|[-()_.!~*';/?:@\x26=+$,A-Za-z0-9])+)([).!';/?:,])?$`. The fixed text is the `1 validation error detected: Value '...' at 'invocationEndpoint' failed to satisfy constraint: Member must satisfy regular expression pattern: ` frame plus the modelled pattern itself INCLUDING its leading `^` and trailing `$`; the only variable part is the rejected endpoint value.
Emulator: emits the same message but with the anchors stripped: `... pattern: ((%[0-9A-Fa-f]{2}|[-()_.!~*';/?:@\x26=+$,A-Za-z0-9])+)([).!';/?:,])?` (no `^`, no `$`).
Why: the message is a contract — clients and test suites match on it, and a client re-applying the quoted regex locally gets an unanchored, substring-matching regex that accepts endpoints AWS rejects.
Fix: the validation-error serializer that renders a member's `@pattern` trait into the message (it must emit the trait value verbatim, anchors included).
Traces: events.create-api-destination-invalid-endpoint-pattern-rejected.

Cause 7 of 7 (bundled under cause 1's registry defect only if the registry is the cause; otherwise independent): PutEvents' global-endpoint rejection path is unreachable. AWS refuses a PutEvents whose `EndpointId` names a global endpoint unrelated to the given `EventBusName` with a 400 ValidationException naming the endpoint and the bus; the emulator cannot reach that decision because the request fails earlier with the `UnknownService` error of cause 1. This is recorded separately so a fixer verifies the refusal, not just the happy path.
Emulator: `UnknownService` / `no emulated service is registered for 'awsevents'`.
Why: the negative class is completely unimplemented.
Fix: PutEvents handler validation of EndpointId against EventBusName, once routing is fixed.
Traces: events.put-events-endpoint-id-with-unrelated-event-bus-name-rejected.

Note: open issues 20261025T002357Z-events-eventbridge-accepts-requests-aws-refuses-reserved-bus-names.md and 20261029T182228Z-events-sqs-eventbridge-puttargets-accepts-conflicting-invalid-input-forms-the.md cover other PutTargets/PutRule acceptance gaps; none of them carries the replay keys above.

## Evidence
Cause 1/7: `botocore.exceptions.ClientError: An error occurred (UnknownService) when calling the PutEvents operation: no emulated service is registered for 'awsevents'`. Expected: HTTP 200 PutEventsResponse (primary delivery + secondary replication).
Cause 2: expected `('ValidationException', 'SQS FIFO is not supported by Dead Letter Queues.')`; got HTTP 200 `{'FailedEntryCount': 0, 'FailedEntries': []}`.
Cause 3: expected `('ValidationException', 'SNS FIFO is not a supported service for a target.')`; got HTTP 200 `{'FailedEntryCount': 0, 'FailedEntries': []}`.
Cause 4: `first message's MessageGroupId was '$.detail.group', expected the event's own detail.group value 'sim2real-group-alpha'`.
Cause 5: expected `InvalidEventPatternException` + 'Event pattern is not valid. Reason: Inside anything but list, either all values are number or string, mixed type is not supported'; got HTTP 200 `{'Result': False}`.
Cause 6: expected pattern `^((%[0-9A-Fa-f]{2}|[-()_.!~*';/?:@\x26=+$,A-Za-z0-9])+)([).!';/?:,])?$`; got `((%[0-9A-Fa-f]{2}|[-()_.!~*';/?:@\x26=+$,A-Za-z0-9])+)([).!';/?:,])?` (anchors missing).
Cause 7: same UnknownService failure as cause 1 on the rejection path.

## Steps to Reproduce
1. Replay the checks listed in trace_ids against the booted emulator.
2. Cause 1/7: `events.put_events(Entries=[{...}], EndpointId='<endpoint-id>')` -> UnknownService 'awsevents'.
3. Cause 2: create a standard target queue and a `.fifo` DLQ, then `put_targets(Rule=R, EventBusName=B, Targets=[{Id:'t', Arn:<std-queue-arn>, DeadLetterConfig:{Arn:<fifo-queue-arn>}}])` -> 200 instead of ValidationException.
4. Cause 3: `put_targets(..., Targets=[{Id:'t', Arn:'arn:aws:sns:<r>:<a>:<name>.fifo'}])` -> 200 instead of ValidationException.
5. Cause 4: put a target on a FIFO queue with `SqsParameters={'MessageGroupId':'$.detail.group'}`, PutEvents with `detail.group='sim2real-group-alpha'`, receive the message and read MessageGroupId.
6. Cause 5: `test_event_pattern(EventPattern='{"detail":{"state":[{"anything-but":["initializing",1]}]}}', Event=<valid envelope>)` -> 200 Result=false instead of InvalidEventPatternException.
7. Cause 6: `create_api_destination(..., InvocationEndpoint='https://sim2real.invalid/a b', ...)` and compare the quoted regex in the message against the model's pattern.

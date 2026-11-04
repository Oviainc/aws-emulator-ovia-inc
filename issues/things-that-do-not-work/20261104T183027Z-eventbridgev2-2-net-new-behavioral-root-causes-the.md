Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eventbridgev2.create-event-bus-name-collision-different-token-rejected eventbridgev2.put-events-partial-failure-reports-per-entry-error
Issue-Shape-Id: com.amazonaws.eventbridgev2#AWSEventsV2
Issue-Key: eventbridgev2.create-event-bus-name-collision-different-token-rejected|run#11|AssertionError|assert message == "Resource already exists", f"unexpected message {message!r}"
Issue-Key: eventbridgev2.put-events-partial-failure-reports-per-entry-error|run#8|AssertionError|assert put["FailedEntryCount"] == 1, (
Issue-Commit: e5b2e36fc94a41937ab22fe33196324420972c76

# eventbridgev2: 2 net-new behavioral root causes — the ResourceAlreadyExistsException message is emulator-authored, and PutEvents never refuses an entry whose Detail is not valid JSON

> _Note: this issue was identified against the code base at commit `e5b2e36fc94a41937ab22fe33196324420972c76`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventBus

## Description
Two distinct, net-new root causes in the EventBridgeV2 (AWSEventsV2, rpcv2Cbor, apiVersion 2025-05-15) emulator. BOTH must be fixed and re-replayed before this issue is closed. Neither is covered by the one open eventbridgev2 issue, 20261104T173733Z-eventbridgev2-error-responses-omit-modeled-exception-type-and.md: that issue covers CreateEventBus input validation (missing ClientToken, name-pattern InvalidInputException wire type/message) and the DescribeEventBus not-found message. Cause 1 below is a DIFFERENT error class on CreateEventBus (ResourceAlreadyExistsException, the name-collision path, which is reached only after validation passes and which that issue never exercises), and Cause 2 is in PutEvents, an operation that issue does not mention at all.

Cause 1 of 2 — CreateEventBus name collision returns an emulator-authored ResourceAlreadyExistsException message.
AWS: Smithy: CreateEventBus declares ResourceAlreadyExistsException (com.amazonaws.eventbridgev2#ResourceAlreadyExistsException, @error client, @httpError 409, single member `Message`, documented "A resource with the same name already exists."); Name is @required, target EventBusName, @length 1-256, @pattern ^[A-Za-z0-9][\.\-_A-Za-z0-9]*$; ClientToken is @idempotencyToken, so a SECOND, different token against a name already held by an ACTIVE bus is a name collision and not an idempotent replay. Recorded behavior: AWS refuses that second create with error code ResourceAlreadyExistsException and `Message` exactly "Resource already exists". That message is entirely FIXED text — AWS interpolates nothing from the request or from state into it: no bus name, no ARN, no id. The original bus survives untouched (same ARN, State ACTIVE) and ListEventBuses still shows exactly one bus with that name.
Emulator: the status and the error code are right, but the message is a sentence the emulator composes from the request and from its own state, of the form "event bus <Name> already exists as <existing bus ARN>" — the replay shows 'event bus sim2real-behav-ebv2-bus-09340d799a47 already exists as arn:aws:events:us-west-2:000000000000:event-busv2/sim2real-behav-ebv2-bus-09340d799a47/0f323xyz81ko603xfo3js6669'. In that actual value the name, the account, the region and the 25-char bus id are this run's request and state filling the emulator's own template; the defect is the wording and the interpolation, not those particular values.
Why: the error message is part of the contract. A client or test that asserts on, logs or parses AWS's constant "Resource already exists" sees different text from the emulator and fails, and conversely the emulator leaks the colliding bus's ARN into a message AWS deliberately keeps constant — so code that learns to read the ARN out of the message from the emulator has nothing to read against AWS.
Fix: the CreateEventBus handler's name-collision path (and the shared ResourceAlreadyExistsException construction it uses) must emit the constant message AWS words for this error class instead of formatting the requested name and the existing ARN into it. Same wording defect family as the DescribeEventBus message in the open issue above, but a different error class on a different code path, so it is not closed by that fix.
Traces: eventbridgev2.create-event-bus-name-collision-different-token-rejected.

Cause 2 of 2 — PutEvents does not validate per-entry Detail as JSON, so a malformed entry is published instead of refused in its own position.
AWS: Smithy: PutEventsRequestEntry$Detail targets EventDetail and its own documentation states "The event payload, as a valid JSON string." PutEventsResultEntry carries EventId, SequenceNumber, SuccessCode, ErrorCode (free-form string, @length 1-256) and ErrorMessage (@length 1-2048), and is documented as aligning 1:1 with the request entry at the same position; PutEventsResponse$FailedEntryCount is documented as "The number of entries in the batch that failed to be published." Recorded behavior: a batch mixing one well-formed entry with one whose Detail is not JSON is accepted as a whole (HTTP 200, no top-level error) and reports PARTIAL failure — the good entry gets an EventId and SuccessCode PUBLISHED, while the bad entry carries NO EventId and instead ErrorCode "MalformedDetail" with ErrorMessage "Detail is not valid JSON", and FailedEntryCount is 1. Both of those strings are fixed text for this entry-level error class — no part is interpolated from the request.
Emulator: every entry is published. The replay shows FailedEntryCount=0 and two result entries both carrying an EventId, a SequenceNumber and SuccessCode 'PUBLISHED', with no ErrorCode or ErrorMessage anywhere, for a batch whose second entry's Detail was the non-JSON string "not-json". The handler's per-entry loop only refuses a Source with the service-reserved "aws." prefix and an unrepresentable Time; it copies Detail into the stored event without parsing it, so no MalformedDetail refusal exists.
Why: a client relying on AWS's behavior reads FailedEntryCount and the per-entry ErrorCode/ErrorMessage to find and retry or quarantine the bad entries of a batch. Against the emulator that loop never fires: malformed payloads are silently accepted and handed an EventId, so a producer that emits bad JSON passes every emulator test and then starts reporting failed entries (or has its events dropped) the first time it runs against AWS. The emulator also stores and will later deliver an event whose Detail AWS would never have admitted to the bus.
Fix: PutEvents per-entry validation in the operation's handler — parse each entry's Detail as JSON before storing it and, when it does not parse, push a result entry at that position with no EventId and no SequenceNumber, ErrorCode and ErrorMessage set for this class, leaving the remaining entries published and FailedEntryCount counted from the results (the handler already counts, so only the refusal branch is missing).
Traces: eventbridgev2.put-events-partial-failure-reports-per-entry-error.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator: 2/10 passed, eventbridgev2 2/10 (8 failed). Six of the failures are the four ALREADY FILED unimplemented-stub clusters (CreateSubscriber, CreateEventSource, PutResourcePolicy, ListTagsForResource); the two below are the net-new, reportable ones.

Cause 1 — [mismatch] eventbridgev2.create-event-bus-name-collision-different-token-rejected:
  CreateEventBus(Name=<N>, ClientToken=<tok1>) -> bus ACTIVE; CreateEventBus(Name=<N>, ClientToken=<tok2>) is correctly refused with Error.Code = ResourceAlreadyExistsException (HTTP 409), but the check fails on the message:
    AssertionError: unexpected message 'event bus sim2real-behav-ebv2-bus-09340d799a47 already exists as arn:aws:events:us-west-2:000000000000:event-busv2/sim2real-behav-ebv2-bus-09340d799a47/0f323xyz81ko603xfo3js6669'
  expected Error.Message == 'Resource already exists' (constant), actual = the emulator-composed "event bus <name> already exists as <arn>". The name/ARN in the actual value are this run's input and state, not part of the requirement.
  Emulator source: services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/create_event_bus.rs lines 240-250 — `format!("event bus {} already exists as {}", input.name, existing.arn)`.

Cause 2 — [mismatch] eventbridgev2.put-events-partial-failure-reports-per-entry-error:
  PutEvents(EventBusArn=<ACTIVE bus>, Entries=[{Source:'sim2real.ebv2.partial', DetailType:'ok', Detail:'{"a": 1}'}, {Source:'sim2real.ebv2.partial', DetailType:'bad', Detail:'not-json'}]):
    expected FailedEntryCount 1, entry[0] {EventId:<id>, SuccessCode:'PUBLISHED'}, entry[1] {ErrorCode:'MalformedDetail', ErrorMessage:'Detail is not valid JSON', no EventId}
    actual FailedEntryCount 0, Entries = [{'EventId': '60cfbede1ace8a39975835de1c9756ce', 'SequenceNumber': '1', 'SuccessCode': 'PUBLISHED'}, {'EventId': '8be1948accd4dc451d0b14e4db018fed', 'SequenceNumber': '2', 'SuccessCode': 'PUBLISHED'}]
    AssertionError: expected exactly one failed entry, got FailedEntryCount=0
  Emulator source: services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/put_events.rs lines 200-299 — the per-entry loop refuses only a reserved `aws.` Source (lines 205-217) and an unrepresentable Time (lines 251-268); `entry.detail` is cloned into the stored event at line 275 with no JSON parse, so no MalformedDetail branch exists. FailedEntryCount is counted correctly at lines 308-311, so it reports 0 only because nothing was refused.

Model references (services/service-com-amazonaws-eventbridgev2-awseventsv2/smithy-model/com-amazonaws-eventbridgev2-awseventsv2.json): service AWSEventsV2 v2025-05-15, rpcv2Cbor, sdkId EventBridgeV2, endpointPrefix eventsv2. CreateEventBus errors include ResourceAlreadyExistsException (409, doc "A resource with the same name already exists.", single `Message` member). PutEventsRequestEntry$Detail doc: "The event payload, as a valid JSON string." PutEventsResultEntry doc: "Result entry for a published event. The position of each entry in the response list aligns 1:1 with the corresponding request entry." PutEventsResponse$FailedEntryCount doc: "The number of entries in the batch that failed to be published."

## Steps to Reproduce
1. git_clone the repo and run the behavioral suite (run_behavioral_suite), which builds and boots the emulator.

Cause 1 (check eventbridgev2.create-event-bus-name-collision-different-token-rejected):
  a. CreateEventBus(Name=<unique name>, ClientToken=<token1>); poll DescribeEventBus until State == ACTIVE.
  b. CreateEventBus(Name=<same name>, ClientToken=<token2>) — a DIFFERENT token, so this is a name collision and not an idempotent replay.
  c. The call is refused 409 ResourceAlreadyExistsException (correct). Read Error.Message: AWS returns the constant 'Resource already exists'; the emulator returns 'event bus <name> already exists as <existing bus ARN>'.

Cause 2 (check eventbridgev2.put-events-partial-failure-reports-per-entry-error):
  a. CreateEventBus(...) and wait for ACTIVE.
  b. PutEvents(EventBusArn=<that bus>, Entries=[{Source:'sim2real.ebv2.partial', DetailType:'ok', Detail:'{"a": 1}'}, {Source:'sim2real.ebv2.partial', DetailType:'bad', Detail:'not-json'}]).
  c. AWS answers 200 with FailedEntryCount 1, the first entry PUBLISHED with an EventId, and the second carrying ErrorCode 'MalformedDetail' / ErrorMessage 'Detail is not valid JSON' and NO EventId. The emulator answers 200 with FailedEntryCount 0 and both entries PUBLISHED with EventIds.

Issue-Source: trace-sweep-divergence
Issue-Trace: eventbridgev2-subscriber-delivers-published-event-to-sqs
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eventbridgev2-subscriber-delivers-published-event-to-sqs
Issue-Shape-Id: com.amazonaws.eventbridgev2#AWSEventsV2
Issue-Key: eventbridgev2|CreateEventBus|2xx->2xx|$.CreationTime
Issue-Key: eventbridgev2|CreateEventBus|2xx->2xx|$.StorageConfiguration
Issue-Key: eventbridgev2|DescribeEventBus|2xx->2xx|$.CreationTime
Issue-Key: eventbridgev2|DescribeEventBus|2xx->2xx|$.Description
Issue-Key: eventbridgev2|DescribeEventBus|2xx->2xx|$.LastModifiedTime
Issue-Key: eventbridgev2|DescribeEventBus|2xx->2xx|$.StorageConfiguration
Issue-Key: eventbridgev2|PutEvents|2xx->2xx|$.Entries[*].EventId
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# eventbridgev2: event-bus reads omit StorageConfiguration and Description, and timestamps and EventId are serialized in non-AWS shapes

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventBus

## Description
Four net-new causes on the EventBridgeV2 (sdkId EventBridgeV2, protocol rpcv2Cbor) SUCCESS paths: every 200 response for an event bus under-populates its modeled output, and two service-minted values are serialized in a shape AWS does not use. All four are on the same bootstrapped service (eventbridgev2) and all four must be closed before this issue is resolved.

Cause 1 of 4 — CreateEventBus/DescribeEventBus never report StorageConfiguration (fixtures: eventbridgev2-subscriber-delivers-published-event-to-sqs#task#4 and #task#5).
AWS: Smithy: CreateEventBusResponse.StorageConfiguration and DescribeEventBusResponse.StorageConfiguration both target com.amazonaws.eventbridgev2#StorageConfigurationOutput {RetentionPeriodInDays @range 1..365, RetentionWindowStartTime (timestamp, doc: "The earliest point in time from which stored events are available")}; the shape's doc words it "Event storage configuration of an event bus, as returned on reads". Recorded behavior: both the create response and the subsequent describe response carry StorageConfiguration with a RetentionPeriodInDays and a RetentionWindowStartTime, i.e. a bus always reports an effective storage configuration on a read.
Emulator: the member is absent from both responses ($.StorageConfiguration: missing, expected {RetentionPeriodInDays, RetentionWindowStartTime}).
Why: a client that reads a bus to learn its retention window (or waits on it before publishing) sees no storage configuration at all and cannot distinguish "no retention" from "not reported"; any code that dereferences StorageConfiguration.RetentionPeriodInDays fails.
Fix: the CreateEventBus and DescribeEventBus handlers' response projection and the bus state that backs it — the projection only emits StorageConfigurationOutput when the create request itself carried a StorageConfiguration, so a bus created without one reports nothing instead of its effective (defaulted) configuration. Hypothesis: AWS defaults an unspecified retention period on the bus rather than leaving the member unset.
Traces: eventbridgev2-subscriber-delivers-published-event-to-sqs#task#4 (CreateEventBus), #task#5 (DescribeEventBus). Code: services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/create_event_bus.rs:340 and src/state.rs:197-203.

Cause 2 of 4 — DescribeEventBus omits Description for a bus that has none (fixture: eventbridgev2-subscriber-delivers-published-event-to-sqs#task#5).
AWS: Smithy: DescribeEventBusResponse.Description targets Description (@length 0..512), i.e. the empty string is a legal value. Recorded behavior only: the recorded DescribeEventBus response carries Description as the empty string "" for a bus created without a description (the recorded CreateEventBus response for the same bus does not, so this is a read-path requirement, not a create-path one).
Emulator: $.Description: missing (expected '', got None) — the member is left unset and dropped from the CBOR map.
Why: a client reading a bus's description gets "absent" where AWS gives "", so round-tripping a describe into an update, or rendering the field, diverges between emulator and AWS.
Fix: the DescribeEventBus handler's response projection (default the member to the empty string when the stored bus has no description) in services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/describe_event_bus.rs.
Traces: eventbridgev2-subscriber-delivers-published-event-to-sqs#task#5.

Cause 3 of 4 — CreationTime/LastModifiedTime are serialized in a different value shape from the recorded timestamps (fixtures: eventbridgev2-subscriber-delivers-published-event-to-sqs#task#4 and #task#5).
AWS: Smithy: CreateEventBusResponse.CreationTime, DescribeEventBusResponse.CreationTime and DescribeEventBusResponse.LastModifiedTime all target com.amazonaws.eventbridgev2#Timestamp (type timestamp); the service's single protocol is rpcv2Cbor, whose timestamps are epoch-second values carried as a tagged CBOR date-time. The recorded responses' timestamp values are shape-asserted by the harness (expected '<shaped-not-asserted>'), i.e. they match the timestamp shape of the recording.
Emulator: the values decode as bare fractional numbers and FAIL that shape assertion — on the create response $.CreationTime came back as a plain number with sub-millisecond fractional precision, and on the describe response both $.CreationTime and $.LastModifiedTime did (the latter one lifecycle span later). Hypothesis: the emulator carries the full nanosecond sub-second component into the serialized value and/or emits it untagged, where AWS reports a tagged epoch timestamp at millisecond precision.
Why: an SDK client decodes the member as a number rather than an instant (or loses the type entirely), so a date comparison, a waiter or any time arithmetic on CreationTime/LastModifiedTime behaves differently against the emulator than against AWS. The concrete epoch values in the recording are per-run and are NOT the requirement — only the value's shape/precision is.
Fix: the timestamp serialization used by these handlers' response projection — services/service-com-amazonaws-eventbridgev2-awseventsv2/src/state.rs:505 (to_date_time, which preserves subsec_nanos) and the rpcv2Cbor timestamp encoding it feeds.
Traces: eventbridgev2-subscriber-delivers-published-event-to-sqs#task#4, #task#5.

Cause 4 of 4 — PutEvents mints an EventId in a form AWS does not use (fixture: eventbridgev2-subscriber-delivers-published-event-to-sqs#task#6).
AWS: Smithy: PutEventsResultEntry.EventId targets EventId (string, @length 1..128, doc "Unique event ID assigned by the service"). Recorded behavior only: the recorded PutEvents response's Entries[0].EventId is shape-asserted, and the emulator's value fails that assertion — EventBridge assigns event ids in hyphenated UUID form (8-4-4-4-12 lowercase hex, 36 characters).
Emulator: $.Entries[0].EventId is 32 undelimited lowercase hex digits (one recorded example: 'd053b063dfb320db975835de1c9756ce'); the id itself is per-run and is not the requirement, its FORM is.
Why: a client that parses, logs-correlates or validates the event id as a UUID (or matches it against an id surfaced on a delivered event) rejects the emulator's value, and nothing can be keyed on the id's shape.
Fix: the id minting the PutEvents handler uses — services/service-com-amazonaws-eventbridgev2-awseventsv2/src/state.rs:450 (new_event_id, which formats the 128 bits as {:032x}); render the same 128 bits in UUID form instead.
Traces: eventbridgev2-subscriber-delivers-published-event-to-sqs#task#6.

Related open issues that do NOT cover these causes (they are all about error-path status/type/message text, not success-path payloads): 20261104T173733Z-eventbridgev2-error-responses-omit-modeled-exception-type-and.md, 20261107T005809Z-eventbridgev2-event-source-error-paths-return-the-wrong.md, 20261104T183027Z-eventbridgev2-2-net-new-behavioral-root-causes-the.md.

## Evidence
Replay of trace set simparity-wire-aws-v1 (fixture eventbridgev2-subscriber-delivers-published-event-to-sqs), all steps answered HTTP 200 as expected; the divergences are in the body:

task#4 CreateEventBus (expected 200, got 200):
  $.CreationTime: value_mismatch (expected '<shaped-not-asserted>', got 1794239124.399352)
  $.StorageConfiguration: missing (expected {'RetentionPeriodInDays': 1, 'RetentionWindowStartTime': '<shaped-not-asserted>'}, got None)

task#5 DescribeEventBus (expected 200, got 200):
  $.CreationTime: value_mismatch (expected '<shaped-not-asserted>', got 1794239124.399352)
  $.Description: missing (expected '', got None)
  $.LastModifiedTime: value_mismatch (expected '<shaped-not-asserted>', got 1794239124.899352)
  $.StorageConfiguration: missing (expected {'RetentionPeriodInDays': 1, 'RetentionWindowStartTime': '<shaped-not-asserted>'}, got None)

task#6 PutEvents (expected 200, got 200):
  $.Entries[0].EventId: value_mismatch (expected '<shaped-not-asserted>', got 'd053b063dfb320db975835de1c9756ce')

Expected per the model: StorageConfigurationOutput{RetentionPeriodInDays @range 1..365, RetentionWindowStartTime timestamp} on both the create and the describe response; Description (@length 0..512) present as "" on the describe; CreationTime/LastModifiedTime as rpcv2Cbor timestamps; EventId (@length 1..128) in hyphenated UUID form. The literal epoch numbers, the bus id and the event id above are per-run values, not requirements.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay wire fixture eventbridgev2-subscriber-delivers-published-event-to-sqs from trace set simparity-wire-aws-v1.
2. Step task#4: CreateEventBus for a bus — inspect the 200 response body: StorageConfiguration is absent and CreationTime is a bare fractional epoch number instead of a protocol timestamp.
3. Step task#5: DescribeEventBus on the ARN just returned — StorageConfiguration and Description are absent, CreationTime and LastModifiedTime are bare numbers.
4. Step task#6: PutEvents one entry onto that bus — Entries[0].EventId is 32 undelimited hex digits rather than a hyphenated UUID.
5. Compare each against the recorded AWS response for the same step.

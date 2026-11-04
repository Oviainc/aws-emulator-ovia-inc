Issue-Source: trace-sweep-divergence
Issue-Trace: eventbridgev2-create-event-bus-validation-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eventbridgev2-create-event-bus-validation-errors eventbridgev2-describe-and-policy-against-missing-resources
Issue-Shape-Id: com.amazonaws.eventbridgev2#AWSEventsV2
Issue-Key: eventbridgev2|CreateEventBus|4xx->2xx|$.Message
Issue-Key: eventbridgev2|CreateEventBus|4xx->2xx|$.__type
Issue-Key: eventbridgev2|CreateEventBus|4xx->2xx|status
Issue-Key: eventbridgev2|CreateEventBus|4xx->4xx|$.Message
Issue-Key: eventbridgev2|CreateEventBus|4xx->4xx|$.__type
Issue-Key: eventbridgev2|DescribeEventBus|4xx->4xx|$.Message
Issue-Commit: 499424707e34574cc6d85e5e4758775b0381202c

# EventBridgeV2 error responses omit modeled exception type and AWS message text

> _Note: this issue was identified against the code base at commit `499424707e34574cc6d85e5e4758775b0381202c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventBus

## Description
The emulator's EventBridgeV2 (AWSEventsV2, rpcv2Cbor, apiVersion 2025-05-15) failure path does not produce the service's modeled exceptions: required-member validation is skipped entirely, the error discriminator is not the modeled shape id, and the `Message` member is either absent or carries emulator-invented prose instead of AWS's message template. THREE distinct causes are bundled here; all three must be fixed and re-replayed.

Cause 1 of 3 - CreateEventBus does not enforce the ClientToken requirement (fixture eventbridgev2-create-event-bus-validation-errors, step task#1).
AWS: recorded behavior only for the requiredness - Smithy marks CreateEventBusRequest.ClientToken as @idempotencyToken with @length min 1 max 64 and pattern ^[\u0021-\u007E]+$, and does not mark it @required; the recording shows the live service nonetheless rejecting a CreateEventBus request that omits ClientToken with HTTP 400, error `com.amazon.nexus.v20250515.exceptions#InvalidInputException` (the modeled shape is com.amazonaws.eventbridgev2#InvalidInputException, @error client, @httpError 400, member `Message`), and `Message` exactly `ClientToken is required`. That message is entirely fixed text - it has no variable part interpolated from the request or from state. Hypothesis: the operation requires the token explicitly even though the model only tags it as an idempotency token.
Emulator: returns HTTP 200 with a success body - no `__type` and no `Message` at all (replay: "status: expected 400, got 200; $.__type: missing; $.Message: missing"). The invalid request is accepted and an event bus is created.
Why: a client that omits ClientToken gets a bus back from the emulator and a 400 from AWS, so code written against the emulator ships without the idempotency token and fails on the first real call. Worse, the emulator silently creates state for a request AWS never accepted, so subsequent steps diverge too.
Fix: CreateEventBus input validation, ahead of the handler's state mutation - reject a request with no ClientToken before creating the bus.

Cause 2 of 3 - CreateEventBus name-pattern rejection uses the wrong error type and no message (fixture eventbridgev2-create-event-bus-validation-errors, step task#2).
AWS: Smithy: CreateEventBusRequest.Name is @required, target EventBusName with @length min 1 max 256 and @pattern ^[A-Za-z0-9][\.\-_A-Za-z0-9]*$; a name violating that pattern is rejected with the operation's modeled InvalidInputException (@httpError 400), discriminator `com.amazon.nexus.v20250515.exceptions#InvalidInputException` as recorded on the wire. `Message` follows AWS's standard constraint-violation template: "<n> validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>". Fixed text: "validation error detected: Value", "at", "failed to satisfy constraint: Member must satisfy regular expression pattern:". Variable parts, all taken from the request and the model: the violation count, the rejected value as supplied, the member name (`name`), and the member's modeled pattern. The recorded instance of that template is "1 validation error detected: Value '.bad-name' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9][\.\-_A-Za-z0-9]*" - the literal value '.bad-name' is this recording's input filling the template, not part of the requirement.
Emulator: returns HTTP 400 (correct status) but with `__type` of `ValidationException` - a shape this operation does not model at all - and with `Message` absent entirely (replay: "$.__type: value_mismatch (expected 'com.amazon.nexus.v20250515.exceptions#InvalidInputException', got 'ValidationException'); $.Message: missing").
Why: an SDK matches the error discriminator against the operation's modeled errors, so `ValidationException` does not deserialize into InvalidInputException: the client sees an unmodeled/unknown service error rather than the typed exception it catches, and error-handling branches keyed on InvalidInputException never run. With `Message` missing there is also nothing to log or surface about which member failed.
Fix: the EventBridgeV2 error serializer plus CreateEventBus validation - emit the operation's modeled InvalidInputException with its wire discriminator, and populate `Message` from the constraint-violation template above.

Cause 3 of 3 - DescribeEventBus on a missing bus returns an emulator-invented message (fixture eventbridgev2-describe-and-policy-against-missing-resources, step task#1).
AWS: Smithy: DescribeEventBus has modeled error ResourceNotFoundException (com.amazonaws.eventbridgev2#ResourceNotFoundException, @error client, @httpError 404, member `Message`, documented "The resource does not exist."); input EventBusArn is @required with the event-busv2 ARN pattern. The recording shows HTTP 404 with `Message` exactly `Resource does not exist` - fixed text with no variable part: AWS does not interpolate the requested ARN, name or id into this message.
Emulator: returns HTTP 404 (correct status) but `Message` is a constructed sentence that embeds the requested ARN - of the form "event bus <arn> does not exist" (replay: expected 'Resource does not exist', got 'event bus arn:aws:events:us-west-2:000000000000:event-busv2/sim2real-probe-ebv2-dpm-missing/a1b2c3d4e5f6a7b8c9d0e1f2a does not exist'). The ARN in that actual value is this run's input; the defect is the wording and the interpolation, not the specific ARN.
Why: the error message is part of the contract. Clients and test suites that assert on or parse the not-found message see different text from the emulator than from AWS, and anything matching AWS's fixed wording fails; conversely the emulator leaks a request value into a message AWS keeps constant.
Fix: the DescribeEventBus handler's not-found path (and the shared ResourceNotFoundException construction it uses) - emit the constant message AWS words for this error class rather than formatting the looked-up ARN into it.

No open issue covers any of these three: list_existing_issues returned nothing for 'eventbridgev2'.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up: true, /__admin/health 200; 16 steps, 1 passed, 15 mismatched, 12 of them unimplemented-operation blocked and filed elsewhere; 3 reportable divergence clusters, all service=eventbridgev2).

Cause 1 - eventbridgev2-create-event-bus-validation-errors#task#1, CreateEventBus, verdict mismatch:
  expected_status 400, actual_status 200
  expected $.__type = 'com.amazon.nexus.v20250515.exceptions#InvalidInputException', actual: missing
  expected $.Message = 'ClientToken is required', actual: missing

Cause 2 - eventbridgev2-create-event-bus-validation-errors#task#2, CreateEventBus, verdict mismatch:
  expected_status 400, actual_status 400
  $.__type: expected 'com.amazon.nexus.v20250515.exceptions#InvalidInputException', actual 'ValidationException'
  $.Message: expected "1 validation error detected: Value '.bad-name' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9][\.\-_A-Za-z0-9]*", actual: missing

Cause 3 - eventbridgev2-describe-and-policy-against-missing-resources#task#1, DescribeEventBus, verdict mismatch:
  expected_status 404, actual_status 404
  $.Message: expected 'Resource does not exist', actual 'event bus arn:aws:events:us-west-2:000000000000:event-busv2/sim2real-probe-ebv2-dpm-missing/a1b2c3d4e5f6a7b8c9d0e1f2a does not exist'

Model references (services/service-com-amazonaws-eventbridgev2-awseventsv2/smithy-model/com-amazonaws-eventbridgev2-awseventsv2.json): service AWSEventsV2 v2025-05-15, protocol rpcv2Cbor, sdkId EventBridgeV2, endpointPrefix eventsv2. CreateEventBus errors include InvalidInputException (400) and does NOT include ValidationException; Name @required @pattern ^[A-Za-z0-9][\.\-_A-Za-z0-9]*$ @length 1-256; ClientToken @idempotencyToken @length 1-64 @pattern ^[\u0021-\u007E]+$. DescribeEventBus errors include ResourceNotFoundException (404, doc "The resource does not exist."); both exception shapes carry a single `Message` member.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus) and read the three eventbridgev2 clusters.

Cause 1: POST an rpcv2Cbor CreateEventBus to the eventsv2 endpoint with a valid Name and NO ClientToken member. Emulator answers 200 with a created bus; AWS answers 400 with __type com.amazon.nexus.v20250515.exceptions#InvalidInputException and Message 'ClientToken is required'. Replay step: eventbridgev2-create-event-bus-validation-errors#task#1.

Cause 2: POST CreateEventBus with Name that violates ^[A-Za-z0-9][\.\-_A-Za-z0-9]*$ (e.g. a leading '.') plus a valid ClientToken. Emulator answers 400 with __type 'ValidationException' and no Message; AWS answers 400 with __type com.amazon.nexus.v20250515.exceptions#InvalidInputException and the "<n> validation error detected: Value '<value>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" message. Replay step: eventbridgev2-create-event-bus-validation-errors#task#2.

Cause 3: POST DescribeEventBus with a well-formed event-busv2 ARN naming a bus that does not exist. Both answer 404, but the emulator's Message interpolates the ARN ("event bus <arn> does not exist") where AWS returns the constant 'Resource does not exist'. Replay step: eventbridgev2-describe-and-policy-against-missing-resources#task#1.

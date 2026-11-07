Issue-Source: trace-sweep-divergence
Issue-Trace: eventbridgev2-describe-and-policy-against-missing-resources
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eventbridgev2-describe-and-policy-against-missing-resources eventbridgev2-event-source-and-subscriber-reference-missing-bus
Issue-Shape-Id: com.amazonaws.eventbridgev2#AWSEventsV2
Issue-Key: eventbridgev2|DescribeEventSource|4xx->4xx|$.Message
Issue-Key: eventbridgev2|CreateEventSource|4xx->4xx|$.Message
Issue-Key: eventbridgev2|CreateEventSource|4xx->4xx|$.__type
Issue-Key: eventbridgev2|CreateEventSource|4xx->4xx|status
Issue-Commit: f8fe3e3963f5907bb066138a03f0b182307b74d9

# EventBridgeV2 event-source error paths return the wrong error class and non-AWS message text

> _Note: this issue was identified against the code base at commit `f8fe3e3963f5907bb066138a03f0b182307b74d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEventSource

## Description
The emulator's EventBridgeV2 (AWSEventsV2, rpcv2Cbor, apiVersion 2025-05-15) event-source operations build their failure responses themselves instead of reproducing the service's modeled exceptions: the not-found message is emulator-authored prose that interpolates the requested ARN, and input-constraint validation is skipped so a request AWS rejects as invalid input is instead answered from the state lookup with a different error class and status. TWO distinct causes are bundled here; BOTH must be fixed and re-replayed before this issue is closed.

Cause 1 of 2 - DescribeEventSource on a missing event source emits an emulator-invented message (fixture eventbridgev2-describe-and-policy-against-missing-resources, step task#4).
AWS: Smithy: DescribeEventSource (input EventSourceArn @required, target EventSourceArn, @length 1-1011, @pattern ^arn:aws(-[a-z0-9]+)*:events:...:event-sourcev2\/(aws\.service|aws\.partner)\/[A-Za-z0-9][\.\-_A-Za-z0-9]{0,255}\/[a-z0-9]{25}$) models error ResourceNotFoundException (com.amazonaws.eventbridgev2#ResourceNotFoundException, @error client, @httpError 404, single member `Message`, doc "The resource does not exist."). The recording shows HTTP 404 with `Message` of the form "EventSource not found: <eventSourceArn>": the fixed text is "EventSource not found: " and the single variable part is the event-source ARN exactly as supplied in the request's EventSourceArn member. (The ARN in the recorded instance is that run's input filling the template, not part of the requirement.)
Emulator: returns HTTP 404 (status is correct) but `Message` is a differently worded sentence of the form "event source <arn> does not exist" (replay: "$.Message: value_mismatch (expected 'EventSource not found: arn:...:event-sourcev2/aws.service/<name>/<id>', got 'event source arn:...:event-sourcev2/aws.service/<name>/<id> does not exist')").
Why: the error message is part of the contract. A client, log scraper or test suite that matches AWS's "EventSource not found: " prefix - the natural way to distinguish a missing event source from a missing bus, since both are ResourceNotFoundException/404 - never matches against the emulator, so not-found handling that works against AWS silently fails here.
Fix: the DescribeEventSource handler's not-found path and the shared ResourceNotFoundException construction it calls - emit AWS's "EventSource not found: <arn>" wording for this error class rather than the emulator's own "<kind> <arn> does not exist" sentence. Note the resource-kind-specific wording: the sibling bus lookup uses a different constant message, already filed as Cause 3 of open issue 20261104T173733Z-eventbridgev2-error-responses-omit-modeled-exception-type-and.md, which does NOT cover this event-source path.

Cause 2 of 2 - CreateEventSource skips Name constraint validation, so an invalid name is answered from the bus lookup with the wrong error class and status (fixture eventbridgev2-event-source-and-subscriber-reference-missing-bus, step task#1; cluster also covers step task#2).
AWS: Smithy: CreateEventSourceRequest.Name is @required, target EventSourceName, @length 1-256, @pattern ^(?!aws\.)[A-Za-z0-9][\.\-_A-Za-z0-9]*$ - the negative lookahead reserves the `aws.` prefix. CreateEventSource models InvalidInputException (com.amazonaws.eventbridgev2#InvalidInputException, @error client, @httpError 400, member `Message`, doc "A request parameter is missing or not valid.") as well as ResourceNotFoundException (404). The recording shows that input-constraint validation runs BEFORE any state lookup: a name violating the reserved-prefix pattern is rejected with HTTP 400, discriminator `com.amazon.nexus.v20250515.exceptions#InvalidInputException` as it appears on the wire, even though the request's EventBusArn names a bus that does not exist. `Message` follows AWS's standard constraint-violation template: "<n> validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>". Fixed text: "validation error detected: Value", "at", "failed to satisfy constraint: Member must satisfy regular expression pattern:". Variable parts, all taken from the request and the model: the violation count, the rejected value as supplied, the member name (`name`), and the member's modeled pattern ((?!aws\.)[A-Za-z0-9][\.\-_A-Za-z0-9]*). The recorded instance substitutes the reserved name supplied in that run.
Emulator: validates nothing and goes straight to the event-bus lookup, returning HTTP 404 with `__type` `ResourceNotFoundException` and an emulator-authored message of the form "event bus <arn> does not exist" (replay: "status: expected 400, got 404; $.__type: value_mismatch (expected 'com.amazon.nexus.v20250515.exceptions#InvalidInputException', got 'ResourceNotFoundException'); $.Message: value_mismatch"). So the status class, the error type and the message are all wrong, and the diagnostic points at the wrong member of the request.
Why: a client gets a retry/provisioning decision instead of a validation failure. An SDK deserializes 404 ResourceNotFoundException and application code concludes the event bus must be created first, when in fact the name it chose is permanently invalid; code that catches InvalidInputException to surface a field error never runs, and the emulator's ordering means any CreateEventSource validation defect is masked whenever the referenced bus is absent. The wrong discriminator also means AWS's 400 is never observed in emulator-based tests.
Fix: CreateEventSource input validation in the operation's handler, ahead of the state lookup - enforce the modeled @required/@length/@pattern constraints on Name (and the other members) first and raise the operation's modeled InvalidInputException with the constraint-violation message template, only falling through to the ResourceNotFoundException bus lookup once the input is valid. The error serializer must emit the modeled exception's wire discriminator.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up: true, "/__admin/health answered 200 and the :37345 data listener accepts"; steps_total 16, steps_passed 1, steps_mismatched 15, not_implemented 9, score_pct 6.2, divergence_count 5). Three of the five divergence clusters are owned by open issue 20261104T173733Z-eventbridgev2-error-responses-omit-modeled-exception-type-and.md; the two clusters below are net-new and owned by nothing.

Cause 1 - eventbridgev2-describe-and-policy-against-missing-resources#task#4, DescribeEventSource, verdict mismatch, affected_count 1:
  expected_status 404, actual_status 404
  $.Message: expected 'EventSource not found: arn:aws:events:us-west-2:000000000000:event-sourcev2/aws.service/sim2real-probe-ebv2-dpm-missing/a1b2c3d4e5f6a7b8c9d0e1f2a'
             actual   'event source arn:aws:events:us-west-2:000000000000:event-sourcev2/aws.service/sim2real-probe-ebv2-dpm-missing/a1b2c3d4e5f6a7b8c9d0e1f2a does not exist'
  (divergence_key: service=eventbridgev2|DescribeEventSource|status=404->404|keys=$.Message)

Cause 2 - eventbridgev2-event-source-and-subscriber-reference-missing-bus#task#1, CreateEventSource, verdict mismatch, affected_count 2 (also #task#2):
  expected_status 400, actual_status 404
  $.__type: expected 'com.amazon.nexus.v20250515.exceptions#InvalidInputException', actual 'ResourceNotFoundException'
  $.Message: expected "1 validation error detected: Value 'aws.reserved' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: (?!aws\\.)[A-Za-z0-9][\\.\\-_A-Za-z0-9]*"
             actual   'event bus arn:aws:events:us-west-2:000000000000:event-busv2/sim2real-probe-ebv2-missing/a1b2c3d4e5f6a7b8c9d0e1f2a does not exist'
  (divergence_key: service=eventbridgev2|CreateEventSource|status=400->404|keys=$.Message,$.__type,status)

Model references (services/service-com-amazonaws-eventbridgev2-awseventsv2/smithy-model/com-amazonaws-eventbridgev2-awseventsv2.json): CreateEventSource errors = ConcurrentModificationException 409, IdempotentParameterMismatchException 409, InvalidStateException 409, LimitExceededException 400, ResourceAlreadyExistsException 409, ResourceNotFoundException 404, AccessDeniedException 403, InternalException 500, InvalidInputException 400, ThrottlingException 429; CreateEventSourceRequest.Name @required @length 1-256 @pattern ^(?!aws\.)[A-Za-z0-9][\.\-_A-Za-z0-9]*$. DescribeEventSource errors = ResourceNotFoundException 404, AccessDeniedException 403, InternalException 500, InvalidInputException 400, ThrottlingException 429; DescribeEventSourceRequest.EventSourceArn @required with the event-sourcev2 ARN pattern. Both InvalidInputException (doc "A request parameter is missing or not valid.") and ResourceNotFoundException (doc "The resource does not exist.") carry a single `Message` member.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout.
2. Call replay_wire_corpus() for trace set simparity-wire-aws-v1 and read the two eventbridgev2 clusters for DescribeEventSource and CreateEventSource.

Cause 1: send an rpcv2Cbor DescribeEventSource to the eventsv2 endpoint with a well-formed event-sourcev2 ARN naming an event source that does not exist. Both AWS and the emulator answer 404, but the emulator's Message reads "event source <arn> does not exist" where AWS returns "EventSource not found: <arn>". Replay step: eventbridgev2-describe-and-policy-against-missing-resources#task#4.

Cause 2: send CreateEventSource with Name violating ^(?!aws\.)[A-Za-z0-9][\.\-_A-Za-z0-9]*$ (any name beginning "aws."), a required Configuration, and an EventBusArn that is well formed but names a bus that does not exist. AWS answers 400 with __type com.amazon.nexus.v20250515.exceptions#InvalidInputException and the "<n> validation error detected: Value '<value>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" message; the emulator answers 404 ResourceNotFoundException with "event bus <arn> does not exist". Replay steps: eventbridgev2-event-source-and-subscriber-reference-missing-bus#task#1 and #task#2.

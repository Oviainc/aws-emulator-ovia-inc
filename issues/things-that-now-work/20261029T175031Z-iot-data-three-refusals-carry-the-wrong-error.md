Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-data.publish-oversize-payload-rejected iot-data.publish-wildcard-topic-rejected iot-data.shadow-reserved-and-oversize-name-rejected
Issue-Shape-Id: com.amazonaws.iotdataplane#IotMoonrakerService
Issue-Key: iot-data.publish-oversize-payload-rejected|run#17|AssertionError|assert message == f"payload larger than {_MAX_PAYLOAD_BYTES} bytes", (
Issue-Key: iot-data.publish-wildcard-topic-rejected|run#18|AssertionError|assert read_code == "InvalidRequestException", (
Issue-Key: iot-data.shadow-reserved-and-oversize-name-rejected|run#18|AssertionError|assert long_message == "Bad Request", (
Issue-Commit: daf74f430223eff1dc28404b54e4f3c7221767a2

# iot-data: three refusals carry the wrong error class or the wrong message — Publish's oversize text, GetRetainedMessage on a wildcard topic, and UpdateThingShadow's over-length shadowName (3 causes)

> _Note: this issue was identified against the code base at commit `daf74f430223eff1dc28404b54e4f3c7221767a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRetainedMessage

## Description
Three distinct root causes in the iot-data (IoT data plane) refusal surface, grounded in 3 failing behavioral checks. Each must be closed before this issue is submitted.

=== Cause 1 of 3: Publish's over-size refusal does not use the service's message ===
AWS: recorded behavior only for the limit itself — Smithy types Publish.payload as a bare @httpPayload blob (Payload) with NO @length trait, so the 131072-byte ceiling exists only in the service. Recorded: a payload of exactly 131072 bytes is accepted and round-trips byte-identically through GetRetainedMessage; 131073 bytes is refused with RequestEntityTooLargeException / HTTP 413 (the error is modelled on UpdateThingShadow; Publish's own modelled error set is InternalFailureException 500 / InvalidRequestException 400 / MethodNotAllowedException 405 / ThrottlingException 429 / UnauthorizedException 401) and the message is exactly "payload larger than 131072 bytes". The whole message is fixed text apart from the limit, which is the service's constant 131072 — notably it does NOT name the request's actual size.
Emulator: the error class and status are right, but the message is "the payload is 131073 bytes; the maximum message payload size is 131072 bytes" — emulator-authored prose that additionally interpolates the request's byte count, a value AWS's message never contains.
Why: a client that matches on the refusal text (or logs/alerts on it) does not recognise the refusal, and anything that parses the message to learn the ceiling reads two numbers where AWS states one.
Fix: the size refusal in the Publish handler's validation (services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops/publish.rs and the message catalog in that crate's src/state.rs) must emit the service's fixed sentence naming only the ceiling.
Traces: iot-data.publish-oversize-payload-rejected.

=== Cause 2 of 3: GetRetainedMessage reports an MQTT wildcard topic as absent instead of refusing it as malformed ===
AWS: recorded behavior only (Topic is an unconstrained string in the model, so nothing model-driven separates these). Recorded: a topic containing an MQTT subscription wildcard ('#' or '+') is not a legal topic identity on EITHER path — GetRetainedMessage answers InvalidRequestException / HTTP 400 with message "Invalid topic filter" (entirely fixed text), exactly as Publish does, while a legal-but-absent topic answers ResourceNotFoundException / HTTP 404 "Retained message not found". The model does list both errors for this operation (InvalidRequestException 400 and ResourceNotFoundException 404), so both classes are available.
Emulator: Publish correctly refuses the wildcard with 400 InvalidRequestException "Invalid topic filter", and nothing is stored under the literal wildcard name — but the READ path diverges: GetRetainedMessage(topic="<base>/#") answers 404 ResourceNotFoundException. The handler does a bare map lookup and 404s on a miss (services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops/get_retained_message.rs:168-172) with no topic-name validation at all.
Why: the 400-vs-404 distinction is the entire signal. A 404 tells the client the wildcard IS a well-formed topic name that merely holds no value, so a client that builds topic names programmatically keeps a wildcard in its request path and retries forever, instead of being told its topic name is a subscription filter. The validation is also asymmetric with Publish, which refuses the same name.
Fix: GetRetainedMessage must apply the same topic-name validation Publish applies, before the store lookup — share the publish-side topic validator rather than duplicating it (services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops/get_retained_message.rs, validator in that crate's src/state.rs).
Traces: iot-data.publish-wildcard-topic-rejected.

=== Cause 3 of 3: an over-length shadowName is refused with the generic constraint-validator prose instead of the service's terse "Bad Request" ===
AWS: Smithy models UpdateThingShadow.shadowName as ShadowName @length(min 1, max 64) @pattern ^[$a-zA-Z0-9:_-]+$ and the max is INCLUSIVE (a 64-character name is accepted and appears in ListNamedShadowsForThing). Recorded: a 65-character name is refused with InvalidRequestException / HTTP 400 and the message is exactly "Bad Request" — all fixed text, and deliberately terse: unlike every other shadow refusal it does NOT name the offending member, its value, or the limit.
Emulator: the error class and status are right (InvalidRequestException), but the message is the framework's model-driven constraint diagnostic: "1 validation error detected: Value '<the 65-character shadowName from the request>' at 'shadowName' failed to satisfy constraint: Member must have length less than or equal to 64". That is emulator-authored text; it echoes the caller's value and names the member and bound, none of which AWS's message carries.
Why: a client matching the refusal text does not recognise it, and the emulator leaks the request value into a message AWS keeps deliberately opaque — so a test or log assertion written against AWS fails, and a client cannot rely on the message being safe to surface.
Fix: shadowName's length bound must be enforced in the IoT data plane's own validation path so it produces the service's fixed "Bad Request" text, rather than being left to the centralized Smithy constraint validator (services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops/update_thing_shadow.rs, with the message in that crate's src/state.rs catalog). Note the same check also pins the reserved-name rule (a $-prefixed shadowName such as "$aws-..." must be refused InvalidRequestException "Invalid shadow name : <name>" even though the modelled pattern permits '$'); that half was never reached because the length refusal failed first, so verify it when re-replaying. This handler is also the subject of Cause 2 of the sibling issue filed for service `iot` (the reply's leaked state.delta) — the two are independent.
Traces: iot-data.shadow-reserved-and-oversize-name-rejected.

## Evidence
Cause 1 — iot-data.publish-oversize-payload-rejected (the RequestEntityTooLargeException code assertion passed; the message assertion failed):
  expected: message == "payload larger than 131072 bytes"
  actual:   AssertionError: the refusal must name the byte ceiling, got 'the payload is 131073 bytes; the maximum message payload size is 131072 bytes'

Cause 2 — iot-data.publish-wildcard-topic-rejected (Publish's 400 InvalidRequestException "Invalid topic filter" and the absence-from-ListRetainedMessages assertions all passed):
  expected: GetRetainedMessage(topic="<base>/#") -> 400 InvalidRequestException, message "Invalid topic filter"
  actual:   AssertionError: GetRetainedMessage must refuse the wildcard topic 'sim2real-behav-iotdata-wild-3d94f96b96b3/#' as malformed rather than report it absent, got 'ResourceNotFoundException'
  contrast that must be preserved: a legal-but-absent sibling topic "<base>/never" correctly answers 404 ResourceNotFoundException

Cause 3 — iot-data.shadow-reserved-and-oversize-name-rejected (the 64-character control name was accepted at version 1 and listed; the InvalidRequestException code assertion passed):
  expected: message == "Bad Request"
  actual:   AssertionError: expected the (deliberately terse) message 'Bad Request', got "1 validation error detected: Value 'sim2real-behav-nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnz' at 'shadowName' failed to satisfy constraint: Member must have length less than or equal to 64"

## Steps to Reproduce
Cause 1:
1. iot-data:Publish(topic=<T>, payload=b"P"*131072, retain=True, qos=1) -> accepted; GetRetainedMessage(topic=<T>) returns all 131072 bytes byte-identically.
2. iot-data:Publish(topic=<T2>, payload=b"P"*131073, retain=True, qos=1) -> 413 RequestEntityTooLargeException; read Error.Message -> expect "payload larger than 131072 bytes", observe emulator prose naming the request's size.

Cause 2:
1. iot-data:Publish(topic="<base>/leaf", payload=b"x", retain=True, qos=1).
2. iot-data:Publish(topic="<base>/#", payload=b"y", retain=True, qos=1) -> correctly 400 InvalidRequestException "Invalid topic filter".
3. iot-data:GetRetainedMessage(topic="<base>/#") -> expect 400 InvalidRequestException "Invalid topic filter"; observe 404 ResourceNotFoundException.
4. iot-data:GetRetainedMessage(topic="<base>/never") -> 404 ResourceNotFoundException (this one is correct and must not change).

Cause 3:
1. iot:CreateThing(thingName=T).
2. iot-data:UpdateThingShadow(thingName=T, shadowName=<64 characters>, payload={"state":{"reported":{"edge":"ok"}}}) -> accepted at version 1 (correct).
3. iot-data:UpdateThingShadow(thingName=T, shadowName=<the same 64 characters + "z">, payload={"state":{"reported":{"edge":"too-long"}}}) -> 400 InvalidRequestException; read Error.Message -> expect "Bad Request", observe the "1 validation error detected: ..." constraint diagnostic.
4. Then also iot-data:UpdateThingShadow(thingName=T, shadowName="$aws-sim2real-behav", ...) -> must be 400 InvalidRequestException "Invalid shadow name : $aws-sim2real-behav" (unreached in this run).

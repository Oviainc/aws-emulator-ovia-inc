Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-data.connection-ops-rejected-for-unconnected-client iot-data.publish-oversize-payload-rejected iot-data.publish-topic-depth-limit-rejected iot-data.shadow-reserved-and-oversize-name-rejected iot-data.shadow-version-counter-survives-delete-and-recreate
Issue-Shape-Id: com.amazonaws.iotdataplane#IotMoonrakerService
Issue-Key: iot-data.connection-ops-rejected-for-unconnected-client|run#5|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: iot-data.publish-oversize-payload-rejected|run#15|AssertionError|assert code == "RequestEntityTooLargeException", (
Issue-Key: iot-data.publish-topic-depth-limit-rejected|run#23|AssertionError|assert long_code == "InvalidRequestException", (
Issue-Key: iot-data.shadow-reserved-and-oversize-name-rejected|run#16|AssertionError|assert long_code == "InvalidRequestException", (
Issue-Key: iot-data.shadow-version-counter-survives-delete-and-recreate|run#16|AssertionError|assert recreated["version"] == deleted["version"] + 2, (
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# IoT Data Plane: 5 divergences — connection ops answer NotImplemented instead of 404, Publish misclassifies oversize payloads and ignores the 256-byte topic ceiling, shadowName length uses ValidationException, and the shadow version counter resets after delete

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConnection / Publish / UpdateThingShadow / DeleteThingShadow

## Description
Five independent root causes in the iot-data (IoT Data Plane, iotmoonrakerservice) emulator. All are grounded in failing checks with the `iot-data` service token.

=== Cause 1 of 5: the MQTT-session connection operations are unimplemented stubs instead of returning ResourceNotFoundException ===
Failing check: iot-data.connection-ops-rejected-for-unconnected-client.
AWS: Smithy: GetConnection errors are ForbiddenException(403), InternalFailureException(500), InvalidRequestException(400), ResourceNotFoundException(404), ThrottlingException(429); ListSubscriptions and DeleteConnection likewise model ResourceNotFoundException, and SendDirectMessage models ResourceNotFoundException alongside RequestEntityTooLargeException. These operations read live MQTT session state, so a clientId that has never connected is a 404 ResourceNotFoundException. Recorded message (example, all four operations): "The client ID is not found. Verify that the client is connected and try again".
Emulator: GetConnection on a never-connected clientId answers error code `NotImplemented` — the operation is a scaffolded stub. The same check then exercises ListSubscriptions, SendDirectMessage and DeleteConnection, which must also answer 404 ResourceNotFoundException (only GetConnection's answer was observed before the check stopped).
Why: a fleet-management caller gets neither the documented 404 nor a usable answer; NotImplemented is not in this operation's modeled error set, so SDK error handling for the not-connected case (the normal, expected outcome) never triggers. The same shortcut's other failure mode is worse: a fabricated 200 from SendDirectMessage is undetected message loss.
Fix: implement the connection-family handlers (GetConnection, ListSubscriptions, SendDirectMessage, DeleteConnection) in the iot-data crate over a session table, and return ResourceNotFoundException (404) when the clientId has no live session.
Traces: iot-data.connection-ops-rejected-for-unconnected-client.

=== Cause 2 of 5: Publish classifies an oversize payload as InvalidRequestException instead of RequestEntityTooLargeException ===
Failing check: iot-data.publish-oversize-payload-rejected.
AWS: Smithy: `PublishRequest.payload` is an unconstrained blob, so the ceiling exists only in the service; the service's error shape set does include `RequestEntityTooLargeException` (httpError 413, modeled on SendDirectMessage and UpdateThingShadow). Recorded: a payload of 131073 bytes (one over the measured-inclusive 131072-byte / 128 KiB limit) is refused with `RequestEntityTooLargeException` (413).
Emulator: the same call is refused with `InvalidRequestException` (400).
Why: the status class and error code are what a client switches on. 413 tells a publisher to split or shrink the message; 400 InvalidRequestException reads as a malformed request and sends the caller looking at topic/QoS/header validity instead of the payload size.
Fix: the Publish handler's payload-size validation in the iot-data crate — raise RequestEntityTooLargeException (413) once the payload exceeds 131072 bytes.
Traces: iot-data.publish-oversize-payload-rejected.

=== Cause 3 of 5: Publish enforces the 8-part topic depth limit but not the 256-byte topic ceiling ===
Failing check: iot-data.publish-topic-depth-limit-rejected.
AWS: Smithy: `Topic` carries no `length` and no `pattern` trait, so both ceilings exist only in the service. Recorded: two independent rules, each with its own message built from the limit — more than 8 slash-separated parts -> InvalidRequestException "Topic can't contain more than 8 parts"; more than 256 bytes total -> InvalidRequestException "Topic can't contain more than 256 bytes". The 8-part boundary is inclusive (8 accepted, 9 refused).
Emulator: the depth rule is enforced (the 9-part publish is refused and the 8-part publish is retained), but a topic of legal depth whose total length exceeds 256 bytes is ACCEPTED — the publish succeeds where AWS answers InvalidRequestException (400).
Why: an over-long topic that the broker should have refused is one no subscription filter can match, so the publish reports success while the message goes nowhere — silent loss that only appears against real AWS.
Fix: the Publish handler's topic validation in the iot-data crate — add the byte-length ceiling (256 bytes, measured on the UTF-8 topic) next to the existing part-count rule, each with its own message naming its limit.
Traces: iot-data.publish-topic-depth-limit-rejected.

=== Cause 4 of 5: a shadowName over the modeled maximum is refused with ValidationException instead of InvalidRequestException ===
Failing check: iot-data.shadow-reserved-and-oversize-name-rejected.
AWS: Smithy: `UpdateThingShadowRequest.shadowName` has @length min 1 max 64 and pattern ^[$a-zA-Z0-9:_-]+$; the service-common error set is InternalFailureException / InvalidRequestException / ThrottlingException, and `ValidationException` is not a shape this service returns at all. Recorded: a 65-character shadowName -> InvalidRequestException / HTTP 400 (recorded message, as an example: "Bad Request"); a 64-character name is accepted.
Emulator: the 65-character name is refused with error code `ValidationException`.
Why: the error code is a requirement, and this one is outside the service's modeled errors, so a client's `except InvalidRequestException` (or the SDK's modeled-error dispatch) never matches and the refusal surfaces as an unknown/unmodeled failure.
Fix: the shadow handlers' constraint-violation mapping in the iot-data crate — emit InvalidRequestException (400) for shadowName length/pattern violations rather than a generic ValidationException.
Traces: iot-data.shadow-reserved-and-oversize-name-rejected.

=== Cause 5 of 5: the shadow version counter is reset by DeleteThingShadow instead of surviving it ===
Failing check: iot-data.shadow-version-counter-survives-delete-and-recreate.
AWS: recorded behavior only (the shadow document body is an unstructured JsonDocument in the model; Smithy does model ConflictException (409) on UpdateThingShadow, which is the gate this counter feeds). Recorded, three times: after a shadow reaches version N and is deleted, the DeleteThingShadow reply reports N, the delete itself consumes N+1, and the next create returns N+2 — the counter is monotone per thing name across the delete, and a write conditioned on the pre-delete version is still refused with ConflictException afterwards.
Emulator: after deleting a shadow at version 2, the next UpdateThingShadow returns version 1 — the counter was reset together with the document.
Why: shadow versions are the optimistic-concurrency token devices hold. With the counter reset, a device holding a stale pre-delete version has its write ACCEPTED (the version matches again by coincidence) where AWS refuses it with ConflictException, so a delete/recreate silently reopens a window for lost updates.
Fix: the shadow state component in the iot-data crate — keep the per-thing-name (and per-shadow-name) version counter when the document is deleted, charge one version to the delete, and resume from there on the next create.
Traces: iot-data.shadow-version-counter-survives-delete-and-recreate.

## Evidence
Cause 1 — expected: GetConnection(clientId=<never connected>) -> 404 ResourceNotFoundException. Actual: AssertionError "GetConnection against a client id that has never connected must be refused with ResourceNotFoundException, got 'NotImplemented'".
Cause 2 — expected: Publish with a 131073-byte payload -> RequestEntityTooLargeException (413). Actual: 'InvalidRequestException'.
Cause 3 — expected: Publish on a legal-depth topic of >256 bytes -> InvalidRequestException (400). Actual: '<<CALL UNEXPECTEDLY SUCCEEDED>>' (the 9-part depth refusal immediately before it passed).
Cause 4 — expected: UpdateThingShadow with a 65-character shadowName -> InvalidRequestException (400). Actual: 'ValidationException'.
Cause 5 — expected: version 4 after deleting version 2 (delete burns 3). Actual: 1 — "A value of 1 means the counter was reset with the document, which voids optimistic concurrency across a delete".

## Steps to Reproduce
Cause 1: iot-data.GetConnection(clientId="never-connected-<uuid>") -> expect 404 ResourceNotFoundException; repeat with ListSubscriptions, SendDirectMessage and DeleteConnection on the same clientId.
Cause 2: iot-data.Publish(topic="t/u", payload=b"x"*131073) -> expect RequestEntityTooLargeException; a 131072-byte payload must still be accepted.
Cause 3: iot-data.Publish(topic="<base>/" + "z"*256, qos=1, retain=True) with <= 8 parts -> expect InvalidRequestException "Topic can't contain more than 256 bytes".
Cause 4: iot.CreateThing(thingName=T); iot-data.UpdateThingShadow(thingName=T, shadowName=<65 chars>, payload={"state":{"reported":{}}}) -> expect InvalidRequestException; a 64-character name must be accepted.
Cause 5: iot-data.UpdateThingShadow(T) twice (versions 1, 2); DeleteThingShadow(T) (reports 2); UpdateThingShadow(T) again -> expect version 4; then UpdateThingShadow with payload {"version": 2, ...} -> expect ConflictException.

Issue-Source: trace-sweep-divergence
Issue-Trace: iotdata-connection-ops-invalid-client-id-validation
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotdata-connection-ops-invalid-client-id-validation iotdata-shadow-name-validation-guards
Issue-Shape-Id: com.amazonaws.iotdataplane#IotMoonrakerService
Issue-Key: iot-data|GetConnection|4xx->4xx|$.message
Issue-Key: iot-data|GetConnection|4xx->4xx|$.traceId
Issue-Key: iot-data|ListSubscriptions|4xx->4xx|$.message
Issue-Key: iot-data|ListSubscriptions|4xx->4xx|$.traceId
Issue-Key: iot-data|SendDirectMessage|4xx->4xx|$.message
Issue-Key: iot-data|SendDirectMessage|4xx->4xx|$.traceId
Issue-Key: iot-data|DeleteConnection|4xx->4xx|$.message
Issue-Key: iot-data|DeleteConnection|4xx->4xx|$.traceId
Issue-Key: iot-data|GetThingShadow|4xx->4xx|$.message
Issue-Key: iot-data|GetThingShadow|4xx->4xx|$.traceId
Issue-Key: iot-data|GetThingShadow|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot-data|GetThingShadow|4xx->4xx|status
Issue-Commit: 42112e68a35449969ef0708992459a3b7df02e34

# iot-data validation refusals come from the generic Smithy constraint validator instead of the service's own error class, message and traceId envelope (6 causes)

> _Note: this issue was identified against the code base at commit `42112e68a35449969ef0708992459a3b7df02e34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConnection

## Description
Six net-new wire divergences in the iot-data (IoT Data Plane) refusal surface, all of one shared fault: input-shape constraints on `clientId`, `thingName` and `shadowName` are left to the centralized Smithy constraint validator, so a refused request is answered with the framework's model-driven diagnostic ("1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>") instead of the IoT Data Plane's own fixed message, and in one case with the wrong error class/status entirely. Separately but on the same path, none of these error bodies carry the service's `traceId` member. Each cause below must be closed before this issue is submitted.

=== Cause 1 of 6: GetConnection on a $-prefixed clientId ===
AWS: Smithy models GetConnection.clientId as ClientId @httpLabel @required @length(min 1, max 128) @pattern ^[^$]; the modelled refusal class is InvalidRequestException (HTTP 400). Recorded: HTTP 400 and the message is exactly "This client ID isn't valid. Client IDs must not exceed 128 characters and can't start with a dollar sign ($). Enter a different client ID" — that sentence is entirely FIXED text: it names neither the offending value, nor the member, nor the pattern. The recorded body also carries a `traceId` member alongside `message` (recorded behavior only — the model declares only `message` on InvalidRequestException, so the traceId field exists in the service and not in the model; its value is per-run and is not asserted).
Emulator: status 400 is right, but the body's `$.message` is the generic validator text "1 validation error detected: Value '<the clientId from the request>' at 'clientId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[^$]", and `$.traceId` is absent (None).
Why: a client (or test, or log alert) that matches the documented refusal sentence does not recognise the refusal, the emulator echoes the caller's value and leaks the service's internal regex into a message AWS keeps fixed and self-explanatory, and a client that reads `traceId` out of an IoT data-plane error for support correlation finds no such field.
Fix: the clientId identity rules belong in the connection operations' own validation in the iot-data crate (services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops/get_connection.rs, with the fixed sentence in that crate's src/state.rs message catalog) so they refuse before the centralized constraint validator can author the message; the `traceId` member must be emitted by this service's error serializer for every refusal body.
Traces: iotdata-connection-ops-invalid-client-id-validation#task#1.

=== Cause 2 of 6: ListSubscriptions on a $-prefixed clientId ===
AWS: Smithy models ListSubscriptions.clientId identically (ClientId @httpLabel @required @length(min 1, max 128) @pattern ^[^$]), errors include InvalidRequestException 400. Recorded: HTTP 400 with the same fixed sentence "This client ID isn't valid. Client IDs must not exceed 128 characters and can't start with a dollar sign ($). Enter a different client ID", plus a `traceId` member.
Emulator: 400 with the generic "1 validation error detected: Value '<clientId>' at 'clientId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[^$]" and no `traceId`.
Why: same as Cause 1 — the refusal is unrecognisable to a client matching AWS's wording, and the message content is emulator-authored.
Fix: same shared clientId validator, applied in services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops/list_subscriptions.rs rather than duplicated per operation.
Traces: iotdata-connection-ops-invalid-client-id-validation#task#2.

=== Cause 3 of 6: SendDirectMessage on a $-prefixed clientId ===
AWS: Smithy models SendDirectMessage.clientId identically (@httpLabel @required @length 1..128 @pattern ^[^$]); InvalidRequestException 400 is modelled. Recorded: HTTP 400 with the same fixed client-ID sentence and a `traceId` member.
Emulator: 400 with the generic constraint diagnostic naming the value, the member 'clientId' and the pattern ^[^$]; `traceId` absent.
Why: same as Cause 1.
Fix: same shared clientId validator, applied in services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops/send_direct_message.rs.
Traces: iotdata-connection-ops-invalid-client-id-validation#task#3.

=== Cause 4 of 6: DeleteConnection on a $-prefixed clientId ===
AWS: Smithy models DeleteConnection.clientId identically; InvalidRequestException 400 is modelled. Recorded: HTTP 400 with the same fixed client-ID sentence and a `traceId` member.
Emulator: 400 with the generic constraint diagnostic; `traceId` absent.
Why: same as Cause 1.
Fix: same shared clientId validator, applied in services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops/delete_connection.rs.
Traces: iotdata-connection-ops-invalid-client-id-validation#task#4.

=== Cause 5 of 6: GetThingShadow with a malformed thingName answers the wrong CLASS as well as the wrong message ===
AWS: Smithy models GetThingShadow.thingName as ThingName @httpLabel @required @length(min 1, max 128) @pattern ^[a-zA-Z0-9:_-]+$, and the operation models both InvalidRequestException (400) and ResourceNotFoundException (404). Recorded: a thingName containing a character outside that pattern is answered HTTP 404 with the message exactly "Invalid thing name specified in request" — entirely fixed text, naming neither the value nor the member. Given the recorded 404, the error class is the operation's 404 error, ResourceNotFoundException (Hypothesis: the data plane resolves the thing by name before shape validation, so a name it cannot resolve is reported as an absent resource rather than a malformed request). The body also carries a `traceId` member.
Emulator: HTTP 400 with x-amzn-errortype: InvalidRequestException and the body message "1 validation error detected: Value '<the thingName from the request>' at 'thingName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9:_-]+$"; `traceId` absent. The handler itself performs no thing-name validation at all (services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops/get_thing_shadow.rs:163-176 goes straight to the store lookup), so the refusal is produced entirely by the centralized validator ahead of `handle`.
Why: the 400-vs-404 split is the whole signal. A 400 InvalidRequestException tells a client its request was structurally rejected and must not be retried against any thing, while AWS's 404 tells it the addressed shadow was not found — so a client that branches on status (provision-then-retry on 404, fail-fast on 400) takes the opposite branch, and SDK error-type matching on ResourceNotFoundException never fires.
Fix: GetThingShadow's thing-name handling belongs in the operation's own path in the iot-data crate — resolve/refuse the thing name in services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops/get_thing_shadow.rs with the fixed sentence from that crate's src/state.rs catalog, answering the operation's 404 error, so the centralized Smithy constraint validator never gets to author this refusal.
Traces: iotdata-shadow-name-validation-guards#task#2.

=== Cause 6 of 6: GetThingShadow with a malformed shadowName returns the constraint diagnostic instead of the service's terse "Bad Request" ===
AWS: Smithy models GetThingShadow.shadowName as ShadowName @httpQuery("name") @length(min 1, max 64) @pattern ^[$a-zA-Z0-9:_-]+$; InvalidRequestException (400) is modelled. Recorded: a shadowName containing a character outside that pattern is refused HTTP 400 and the message is exactly "Bad Request" — all fixed text, and deliberately terse: unlike the emulator's text it does NOT name the offending member, its value or the pattern. The body also carries a `traceId` member. This matches the service's behavior for an over-length shadowName already recorded elsewhere, so "Bad Request" is the shadowName refusal text generally, not one recording's value.
Emulator: status 400 is right, but the message is the framework diagnostic "1 validation error detected: Value '<the shadowName from the request>' at 'shadowName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[$a-zA-Z0-9:_-]+$", and `traceId` is absent.
Why: a client matching the refusal text does not recognise it, and the emulator leaks the caller's value and the service's internal regex into a message AWS keeps deliberately opaque — so an assertion or log rule written against AWS fails, and the message cannot be assumed safe to surface.
Fix: enforce shadowName's pattern (and length) in this service's own validation path so it emits the fixed "Bad Request" text — services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops/get_thing_shadow.rs with the message in that crate's src/state.rs catalog — instead of leaving it to the centralized constraint validator. The same terse-wording rule for shadowName on the WRITE path is Cause 3 of the open behavioral issue 20261029T175031Z-iot-data-three-refusals-carry-the-wrong-error.md (UpdateThingShadow, over-length name); that issue does not cover this read-path/pattern divergence, so fix both through one shared validator.
Traces: iotdata-shadow-name-validation-guards#task#3.

Note: the resolved issue 20261022T071243Z-iot-data-plane-responses-omit-the-service-s.md previously addressed the message/traceId envelope for this service; a resolved issue does not cover these failures — the `traceId` member is still missing from every refusal body above.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; 9 of 79 steps mismatched).

Cause 1 — fixture iotdata-connection-ops-invalid-client-id-validation step task#1 (GetConnection), expected 400, actual 400:
  $.message: expected "This client ID isn't valid. Client IDs must not exceed 128 characters and can't start with a dollar sign ($). Enter a different client ID"
                 got "1 validation error detected: Value '$sim2real-probe-invalid-client' at 'clientId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[^$]"
  $.traceId: missing (expected a shaped value, got None)
  (the '$sim2real-probe-invalid-client' text is this run's request value; only the fixed sentence above is the requirement)

Cause 2 — same fixture, step task#2 (ListSubscriptions), expected 400, actual 400: identical $.message value_mismatch and $.traceId missing.

Cause 3 — same fixture, step task#3 (SendDirectMessage), expected 400, actual 400: identical $.message value_mismatch and $.traceId missing.

Cause 4 — same fixture, step task#4 (DeleteConnection), expected 400, actual 400: identical $.message value_mismatch and $.traceId missing.

Cause 5 — fixture iotdata-shadow-name-validation-guards step task#2 (GetThingShadow):
  status: expected 404, got 400
  $.message: expected 'Invalid thing name specified in request'
                 got "1 validation error detected: Value 'bad!thing' at 'thingName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9:_-]+$"
  $headers.x-amzn-errortype: shape_mismatch, got 'InvalidRequestException' (the recorded 404 is the operation's ResourceNotFoundException)
  $.traceId: missing

Cause 6 — fixture iotdata-shadow-name-validation-guards step task#3 (GetThingShadow), expected 400, actual 400:
  $.message: expected 'Bad Request'
                 got "1 validation error detected: Value 'bad!shadow' at 'shadowName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[$a-zA-Z0-9:_-]+$"
  $.traceId: missing

## Steps to Reproduce
Causes 1-4 (replay fixture iotdata-connection-ops-invalid-client-id-validation):
1. iot-data:GetConnection(clientId="$<any-name>")  -> expect 400, body {"message": "This client ID isn't valid. Client IDs must not exceed 128 characters and can't start with a dollar sign ($). Enter a different client ID", "traceId": "<any>"}; observe the "1 validation error detected: ..." diagnostic and no traceId.
2. iot-data:ListSubscriptions(clientId="$<any-name>") -> same expectation, same divergence.
3. iot-data:SendDirectMessage(clientId="$<any-name>", topic=<T>, payload=b"x") -> same expectation, same divergence.
4. iot-data:DeleteConnection(clientId="$<any-name>") -> same expectation, same divergence.

Causes 5-6 (replay fixture iotdata-shadow-name-validation-guards):
5. iot-data:GetThingShadow(thingName="bad!thing") -> expect HTTP 404, x-amzn-errortype ResourceNotFoundException, message "Invalid thing name specified in request", plus a traceId member; observe 400 InvalidRequestException with the constraint diagnostic.
6. iot-data:GetThingShadow(thingName=<a thing name matching ^[a-zA-Z0-9:_-]+$>, shadowName="bad!shadow") -> expect HTTP 400 with message exactly "Bad Request" plus a traceId member; observe the "1 validation error detected: ... ^[$a-zA-Z0-9:_-]+$" diagnostic.

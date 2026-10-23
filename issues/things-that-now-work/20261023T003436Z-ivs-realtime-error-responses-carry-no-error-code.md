Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-realtime-create-participant-token-duration-boundary-rejected-over-max
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-realtime-create-participant-token-duration-boundary-rejected-over-max ivs-realtime-create-participant-token-nonexistent-stage-not-found
Issue-Shape-Id: com.amazonaws.ivsrealtime#AmazonInteractiveVideoServiceRealTime
Issue-Key: ivs-realtime|CreateParticipantToken|4xx->4xx|$.Code
Issue-Key: ivs-realtime|CreateParticipantToken|4xx->4xx|$.Message
Issue-Key: ivs-realtime|CreateParticipantToken|4xx->4xx|$.RequestID
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# IVS RealTime error responses carry no error code or message

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateParticipantToken

## Description
The ivs-realtime emulator picks the right HTTP status for a refused request but serializes an error payload a client cannot read: the replay harness finds no error code, no message and no request id on the response at all. 2 causes, both on CreateParticipantToken.

Cause 1 of 2: a duration above the modeled maximum is refused with a 400 that carries no error document. Fixture ivs-realtime-create-participant-token-duration-boundary-rejected-over-max, step task#3.
  AWS: Smithy: CreateParticipantToken.duration -> ParticipantTokenDurationMinutes @range min 1 max 20160; the operation models ValidationException (errorKind client, httpError 400), whose `exceptionMessage` member carries the text and whose code is announced to the client (restJson1: the error shape name, via x-amzn-ErrorType / the body's type field). A request with duration=20161 is therefore a 400 ValidationException, and because the text is built from the request it states the member and the constraint it violated - the recording shows 'CreateParticipantToken.duration is required to have a maximum value of 20160.0, found 20161.0' as an example of that rule. AWS also returns a request id on every response.
  Emulator: status 400 is correct, but the response carries $.Code missing (expected 'ValidationException'), $.Message missing, and $.RequestID missing - i.e. an empty/unreadable error envelope.
  Why: an AWS SDK maps a modeled exception by its error code. With no code the SDK cannot raise ValidationException, cannot branch on it, and surfaces an unidentified 400 with no message, so any client that retries or reports on validation failures behaves differently against the emulator than against AWS.
  Fix: the service's restJson1 error serializer (the shared error-response path the ivs-realtime crate's modeled errors are serialized through) must emit the modeled error shape's code and its message member in the form the protocol specifies, and must set a request id on the response. Hypothesis: the message member is being emitted under its model member name rather than the name the restJson1 error envelope uses, which is why both the code and the message read as absent.
  Traces: ivs-realtime-create-participant-token-duration-boundary-rejected-over-max#task#3.

Cause 2 of 2: a token request against a stage ARN that does not exist is refused with a 404 that carries no error document. Fixture ivs-realtime-create-participant-token-nonexistent-stage-not-found, step task#2.
  AWS: Smithy: CreateParticipantToken models ResourceNotFoundException (errorKind client, httpError 404), documented 'Request references a resource which does not exist.' The code is ResourceNotFoundException; the message is built from the request and names the missing resource - the recording shows 'Resource: arn:aws:ivs:us-west-2:000000000000:stage/<id> not found' as an example of that rule.
  Emulator: status 404 is correct, but $.Code, $.Message and $.RequestID are all missing from the response.
  Why: same client impact as cause 1 - the not-found condition is unrecognisable to an SDK, so a caller that distinguishes 'stage gone' from any other 404 cannot.
  Fix: same component as cause 1 - the modeled-error serialization path, not the handler (the handler already selects the right error and status).
  Traces: ivs-realtime-create-participant-token-nonexistent-stage-not-found#task#2.

Related prior art, not a duplicate: the sibling IVS crate had the same family of defects filed and resolved as 20261022T071923Z-ivs-9-behavioral-divergences-empty-error-message-on.md ('empty error Message on all modeled exceptions'); that issue is resolved and covers the ivs service, not ivs-realtime.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline.

Cause 1 - fixture ivs-realtime-create-participant-token-duration-boundary-rejected-over-max step task#3, operation CreateParticipantToken, verdict mismatch:
  expected status 400, actual status 400
  $.Code: missing (expected 'ValidationException', got None)
  $.Message: missing (expected 'CreateParticipantToken.duration is required to have a maximum value of 20160.0, found 20161.0', got None)
  $.RequestID: missing (expected '<shaped-not-asserted>', got None)

Cause 2 - fixture ivs-realtime-create-participant-token-nonexistent-stage-not-found step task#2, operation CreateParticipantToken, verdict mismatch:
  expected status 404, actual status 404
  $.Code: missing (expected 'ResourceNotFoundException', got None)
  $.Message: missing (expected 'Resource: arn:aws:ivs:us-west-2:000000000000:stage/sim2real-missing-344fc3578463 not found', got None)
  $.RequestID: missing (expected '<shaped-not-asserted>', got None)

Model evidence: smithy_query(operation="CreateParticipantToken") on services/service-com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime/smithy-model/com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime.json - duration @range 1..20160; errors include ValidationException (400) and ResourceNotFoundException (404). smithy_query(shape="ValidationException,ResourceNotFoundException") shows each carries an exceptionMessage member plus an x-amzn-ErrorType header binding.

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. Cause 1: POST /CreateParticipantToken with a valid existing stageArn and duration=20161. Observe HTTP 400 whose body carries no error code and no message, and no request id on the response; AWS answers 400 with code ValidationException and a message naming CreateParticipantToken.duration and its 20160 maximum. Fixture ivs-realtime-create-participant-token-duration-boundary-rejected-over-max, step task#3.
3. Cause 2: POST /CreateParticipantToken with a syntactically valid stageArn that names no existing stage. Observe HTTP 404 with no error code and no message; AWS answers 404 with code ResourceNotFoundException and a message naming the missing stage ARN. Fixture ivs-realtime-create-participant-token-nonexistent-stage-not-found, step task#2.

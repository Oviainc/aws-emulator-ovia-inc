Issue-Source: trace-sweep-divergence
Issue-Trace: mediatailor-channel-start-stop-and-log-configuration
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediatailor-channel-start-stop-and-log-configuration
Issue-Shape-Id: com.amazonaws.mediatailor#MediaTailor
Issue-Key: mediatailor|StartChannel|4xx->4xx|$.message
Issue-Key: mediatailor|StartChannel|4xx->4xx|$.requestId
Issue-Key: mediatailor|StartChannel|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: mediatailor|StartChannel|4xx->4xx|status
Issue-Commit: 415bb240c081114340f3b6ac41e90b57c519fa79

# MediaTailor StartChannel miss answers 400 BadRequestException instead of 404 and omits message/requestId

> _Note: this issue was identified against the code base at commit `415bb240c081114340f3b6ac41e90b57c519fa79`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartChannel

## Description
One root cause (Cause 1 of 1). StartChannel against a channel name the account does not hold is refused with the service's BadRequestException at HTTP 400, where AWS answers HTTP 404 with a not-found error type, and the error body carries neither the `message` nor the `requestId` member AWS returns. Replay fixture mediatailor-channel-start-stop-and-log-configuration, step task#7.

Cause 1 of 1 - StartChannel on an absent channel: wrong status, wrong error type, empty error body (fixture mediatailor-channel-start-stop-and-log-configuration, step task#7).

AWS: Smithy model services/service-com-amazonaws-mediatailor-mediatailor/smithy-model/com-amazonaws-mediatailor-mediatailor.json - StartChannel: PUT /channel/{ChannelName}/start, success code 200, @idempotent, input ChannelName -> __string @required @httpLabel, errors [] (so the refusal is recorded behavior only). The recorded response for a channel name that does not exist is HTTP 404 whose JSON body carries `message` = "The specified channel doesn't exist." - this is ENTIRELY FIXED text, with no variable part: AWS does not interpolate the requested ChannelName - plus a `requestId` member whose value is a per-run id (asserted for presence/shape only, never literally). The recorded x-amzn-errortype header is present but redacted in the fixture, and it is NOT BadRequestException. This is the same not-found envelope the recording shows for DescribeChannel on an absent channel.

Emulator: HTTP 400 instead of 404; x-amzn-errortype: BadRequestException, which fails the recorded header's shape; and both $.message and $.requestId are absent from the body. Source: src/ops/start_channel.rs lines 66-82 widen the service's BadRequestException (httpError 400) into this operation via extra_errors, and lines 147-154 refuse the miss with that shape carrying channel_not_found_message(&input.channel_name) - so the status and error type come from a generic bad-request shape rather than a not-found one, and the text that is computed never reaches the wire because the modeled-error serializer emits the shape's member name (`Message`, no jsonName) instead of AWS's lowercase `message` and emits no `requestId` at all.

Why: a client cannot distinguish "this channel does not exist" from "my request is malformed". SDK waiters and control flow branch on 404 (and on the not-found error type) to mean absent - create-if-absent, start-after-create retry loops and delete-if-present logic all take the opposite branch on a 400, which is modeled as a non-retryable client fault. On top of that the 400 body is completely silent: the SDK exception's message is empty and no requestId is available for correlation, so neither a human nor matching logic has anything to work with.

Fix: the StartChannel handler's miss branch (src/ops/start_channel.rs) must answer AWS's 404 with the service's not-found error type and AWS's fixed message sentence rather than the widened BadRequestException; and the service's modeled-error serializer must render the error text under the wire key `message` and add `requestId` to the envelope.

Related open issue (does NOT cover this step - the replay reported task#7 as unowned): "MediaTailor error responses omit message and requestId and answer 400 where AWS answers 404" (20261030T045613Z-mediatailor-error-responses-omit-message-and-requestid-and.md) files the same fault family for CreateChannel, DescribeChannel and ConfigureLogsForPlaybackConfiguration, but not for StartChannel; its replay key does not exercise this fixture step.

Traces: mediatailor-channel-start-stop-and-log-configuration#task#7.

## Evidence
Trace set simparity-wire-aws-v1, fixture mediatailor-channel-start-stop-and-log-configuration, step task#7, operation StartChannel, verdict mismatch, expected status 404, actual status 400:
  status: expected 404, got 400
  $.requestId: missing (expected '<shaped-not-asserted>', got None)
  $.message: missing (expected "The specified channel doesn't exist.", got None)
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'BadRequestException')

The expected message is fixed text in full: "The specified channel doesn't exist." - no part of it is taken from the request or from state. The expected requestId is a per-run value and is asserted only for presence/shape.

Model evidence (smithy_query query="operation" operation="StartChannel,DescribeChannel", model_path="services/service-com-amazonaws-mediatailor-mediatailor/smithy-model/com-amazonaws-mediatailor-mediatailor.json"):
  StartChannel: PUT /channel/{ChannelName}/start, code 200, idempotent, errors []
                input { ChannelName: __string, httpLabel, required }, output {}
  DescribeChannel: GET /channel/{ChannelName}, code 200, errors []  (same recorded not-found envelope)

Code citations:
  services/service-com-amazonaws-mediatailor-mediatailor/src/ops/start_channel.rs:66-82 (BadRequestException widened in via extra_errors, documented as "the only error shape the model gives this service")
  services/service-com-amazonaws-mediatailor-mediatailor/src/ops/start_channel.rs:147-154 (miss branch -> BadRequestException with channel_not_found_message, i.e. HTTP 400)
  services/service-com-amazonaws-mediatailor-mediatailor/src/state.rs (channel_not_found_message, the text helper)

## Steps to Reproduce
1. git_clone() the repo and boot the emulator.
2. Start a channel the emulator does not hold:
   aws mediatailor start-channel --channel-name no-such-channel
3. Expected (AWS, per the recording): HTTP 404, x-amzn-errortype set to the service's not-found error type (NOT BadRequestException), body containing message = "The specified channel doesn't exist." and a requestId member.
4. Actual (emulator): HTTP 400, x-amzn-errortype: BadRequestException, body with no `message` key and no `requestId` key.
5. Inspect the raw response with curl -i -X PUT "http://<emulator-host>:<port>/channel/no-such-channel/start" to confirm the status, the errortype header and the key casing of the error member.
6. Re-replay with replay_wire_corpus: fixture mediatailor-channel-start-stop-and-log-configuration step task#7 must pass.

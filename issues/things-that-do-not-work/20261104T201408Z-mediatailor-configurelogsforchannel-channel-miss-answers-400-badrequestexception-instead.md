Issue-Source: trace-sweep-divergence
Issue-Trace: mediatailor-channel-start-stop-and-log-configuration
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediatailor-channel-start-stop-and-log-configuration
Issue-Shape-Id: com.amazonaws.mediatailor#MediaTailor
Issue-Key: mediatailor|ConfigureLogsForChannel|4xx->4xx|$.message
Issue-Key: mediatailor|ConfigureLogsForChannel|4xx->4xx|$.requestId
Issue-Key: mediatailor|ConfigureLogsForChannel|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: mediatailor|ConfigureLogsForChannel|4xx->4xx|status
Issue-Commit: 7f890db38bc0d5336bbca4cc09866f453bb79982

# MediaTailor ConfigureLogsForChannel channel miss answers 400 BadRequestException instead of 404 and omits message/requestId

> _Note: this issue was identified against the code base at commit `7f890db38bc0d5336bbca4cc09866f453bb79982`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ConfigureLogsForChannel

## Description
One root cause (Cause 1 of 1). ConfigureLogsForChannel, asked to configure logs on a channel the account does not hold, refuses with the wrong HTTP status, the wrong error type and an error body that carries neither of the two members AWS returns.

Cause 1 of 1 - ConfigureLogsForChannel on an unknown channel (fixture mediatailor-channel-start-stop-and-log-configuration, step task#8).

AWS: Smithy model services/service-com-amazonaws-mediatailor-mediatailor/smithy-model/com-amazonaws-mediatailor-mediatailor.json - ConfigureLogsForChannel: PUT /configureLogs/channel, success code 200, input {ChannelName: __string @required, LogTypes: LogTypes @required}, errors [] (the model declares no error on this operation, so the refusal is recorded behavior only). The recorded response for a ChannelName that addresses no channel is HTTP 404 whose JSON body carries `message` = "The specified channel doesn't exist." - this is entirely FIXED text with NO variable part: AWS does not interpolate the requested ChannelName into it - plus a `requestId` member, whose value is a per-run identifier and is asserted for presence/shape only, never literally. The recorded x-amzn-errortype header is present but redacted in the fixture, and it is NOT BadRequestException: it is this service's not-found error type.

Emulator: HTTP 400 where AWS returns 404; x-amzn-errortype: BadRequestException, which fails the recorded header's shape; and the body carries NEITHER member - $.message is absent and $.requestId is absent. The handler widens the service-wide BadRequestException (HTTP 400) in through `extra_errors` and answers a miss with it (services/service-com-amazonaws-mediatailor-mediatailor/src/ops/configure_logs_for_channel.rs:82-89 for the widening, 170-177 for the miss branch), and the text it builds is the shared non-AWS sentence "Channel <ChannelName> not found." (src/state.rs:271-273, channel_not_found_message) - wrong wording as well as wrong status. Hypothesis for the two missing members: the modeled BadRequestException declares its single member as `Message` with no jsonName, so the restJson1 error serializer emits the key `Message` while AWS's MediaTailor error envelope uses lowercase `message`, and nothing in the modeled-error path emits `requestId` at all; the framework's own synthetic errors DO serialize lowercase `message` (src/scaffolded.rs:117-144), which is why only MODELED errors lose it.

Why: a client cannot tell "that channel does not exist" from "your request is malformed". SDK waiters and create-if-absent / configure-if-exists flows branch on 404 (and on the not-found error type) to mean absent and on 400 to mean do-not-retry-do-not-create, so they take the opposite branch; SDK error-type matching for the not-found case never fires; and because the body has no message and no requestId at all, the SDK exception exposes an empty message and there is nothing for logging or support tooling to correlate on.

Fix: the ConfigureLogsForChannel handler's miss branch plus this service's error classification and modeled-error serializer - the refusal must carry AWS's 404 status and its not-found error type with AWS's fixed message text, and the error envelope must render that text under the wire key `message` and carry `requestId`. The shared helper that words a channel miss must word it as AWS does rather than interpolating the requested name.

Related open issues (neither covers this step; the replay reported task#8 as unowned): 20261103T024708Z-mediatailor-startchannel-miss-answers-400-badrequestexception-instead-of.md owns the same fault on StartChannel (task#7) only, and 20261030T045613Z-mediatailor-error-responses-omit-message-and-requestid-and.md owns it on CreateChannel, DescribeChannel and ConfigureLogsForPlaybackConfiguration only. Neither names ConfigureLogsForChannel or this fixture step, so the channel-logging route stays broken when they are closed.

Traces: mediatailor-channel-start-stop-and-log-configuration#task#8.

## Evidence
Trace set simparity-wire-aws-v1, fixture mediatailor-channel-start-stop-and-log-configuration, step task#8, operation ConfigureLogsForChannel, verdict mismatch, expected status 404, actual status 400:
  status: expected 404, got 400
  $.requestId: missing (expected '<shaped-not-asserted>', got None)
  $.message: missing (expected "The specified channel doesn't exist.", got None)
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'BadRequestException')

The expected message is entirely fixed text; the requested ChannelName is NOT part of it. The expected requestId is a per-run value, asserted for presence/shape only.

Model evidence (smithy_query query="operation" operation="ConfigureLogsForChannel", model_path=services/service-com-amazonaws-mediatailor-mediatailor/smithy-model/com-amazonaws-mediatailor-mediatailor.json):
  ConfigureLogsForChannel: PUT /configureLogs/channel, code 200, errors []
  input  { ChannelName: __string REQUIRED, LogTypes: LogTypes REQUIRED }
  output { ChannelName: __string, LogTypes: LogTypes }
BadRequestException is the one client error this SERVICE declares (httpError 400, members { Message: __string }, no jsonName, no requestId member).

Code citations:
  services/service-com-amazonaws-mediatailor-mediatailor/src/ops/configure_logs_for_channel.rs:82-89 (BadRequestException widened in via extra_errors)
  services/service-com-amazonaws-mediatailor-mediatailor/src/ops/configure_logs_for_channel.rs:170-177 (channel miss -> 400 BadRequestException)
  services/service-com-amazonaws-mediatailor-mediatailor/src/state.rs:271-273 (channel_not_found_message -> "Channel {name} not found.", non-AWS wording, interpolates the request's name)
  services/service-com-amazonaws-mediatailor-mediatailor/src/scaffolded.rs:117-144 (synthetic errors DO serialize lowercase `message`, so only modeled errors lose it)

## Steps to Reproduce
1. git_clone() the repo and boot the emulator.
2. Create nothing, then configure logs on a channel name no channel holds:
   aws mediatailor configure-logs-for-channel --channel-name no-such-channel --log-types AS_RUN
3. Expected (AWS): HTTP 404, x-amzn-errortype naming this service's not-found error type, JSON body containing message = "The specified channel doesn't exist." (fixed text, the channel name is not interpolated) and a requestId member.
   Actual (emulator): HTTP 400, x-amzn-errortype: BadRequestException, and a body with neither a `message` nor a `requestId` key.
4. Inspect the raw body with curl -i to confirm the key casing of the error member (the emulator emits `Message`, AWS emits `message`).
5. Re-replay with replay_wire_corpus: mediatailor-channel-start-stop-and-log-configuration step task#8 must pass.

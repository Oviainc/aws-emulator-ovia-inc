Issue-Source: trace-sweep-divergence
Issue-Trace: mediatailor-channel-tags-and-schedule-lifecycle
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediatailor-channel-tags-and-schedule-lifecycle mediatailor-playback-configuration-log-configuration
Issue-Shape-Id: com.amazonaws.mediatailor#MediaTailor
Issue-Key: mediatailor|CreateChannel|4xx->4xx|$.message
Issue-Key: mediatailor|CreateChannel|4xx->4xx|$.requestId
Issue-Key: mediatailor|DescribeChannel|4xx->4xx|$.message
Issue-Key: mediatailor|DescribeChannel|4xx->4xx|$.requestId
Issue-Key: mediatailor|DescribeChannel|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: mediatailor|DescribeChannel|4xx->4xx|status
Issue-Key: mediatailor|ConfigureLogsForPlaybackConfiguration|4xx->4xx|$.message
Issue-Key: mediatailor|ConfigureLogsForPlaybackConfiguration|4xx->4xx|$.requestId
Issue-Key: mediatailor|ConfigureLogsForPlaybackConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: mediatailor|ConfigureLogsForPlaybackConfiguration|4xx->4xx|status
Issue-Commit: 841ccf5bd961485229918da026bcdc910fdfcb94

# MediaTailor error responses omit message and requestId and answer 400 where AWS answers 404

> _Note: this issue was identified against the code base at commit `841ccf5bd961485229918da026bcdc910fdfcb94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeChannel

## Description
Four root causes (Cause 1 of 4 .. Cause 4 of 4) sharing one fault family: every mediatailor error body is serialized without the `message` and `requestId` members AWS returns, and the two not-found refusals additionally use HTTP 400/BadRequestException where AWS returns HTTP 404. Replay fixtures mediatailor-channel-tags-and-schedule-lifecycle and mediatailor-playback-configuration-log-configuration.

Cause 1 of 4 - CreateChannel's duplicate-name refusal has the right status but an error body with no message and no requestId (fixture mediatailor-channel-tags-and-schedule-lifecycle, step task#9).

AWS: Smithy model services/service-com-amazonaws-mediatailor-mediatailor/smithy-model/com-amazonaws-mediatailor-mediatailor.json - CreateChannel: POST /channel/{ChannelName}, success 200, errors [] (so the refusal text is recorded behavior only). The recorded refusal is HTTP 400 whose JSON body carries `message` = "This channel already exists. Create a unique channel and resubmit your request." - entirely FIXED text, with no variable part: AWS does not interpolate the requested ChannelName - plus a `requestId` member (its value is a per-run id and is asserted only for presence/shape, never literally).

Emulator: HTTP 400 (status matches) but the body has NEITHER member: $.message is absent and $.requestId is absent. The handler does build text (src/ops/create_channel.rs lines 373-383 raises BadRequestException with "Channel with name <ChannelName> already exists."), so the text exists but is not on the wire under `message`. Hypothesis: the modeled shape BadRequestException declares its member as `Message` with no jsonName (smithy_query query="shape" shape="BadRequestException" -> members {Message: __string}), so the restJson1 error serializer emits the key `Message`, while AWS's MediaTailor error envelope uses lowercase `message`; nothing in the error path emits `requestId` at all. Note the framework's own synthetic errors DO serialize lowercase `message` (src/scaffolded.rs lines 117-144), which is why only MODELED errors lose it. Two requirements therefore diverge: the member key, and the message text itself (AWS's is the fixed sentence above, the emulator's interpolates the request's channel name). Open issue "mediatailor CreateChannel duplicate-name refusal uses a non-AWS BadRequestException message" (20261030T005241Z-...) covers the TEXT only; the replay reported this step as unowned and that issue does not cover the missing `message`/`requestId` members, which are a serializer defect affecting every error this service returns.

Why: a client gets a 400 with no human- or machine-readable reason whatsoever. SDK exception objects expose an empty message, retry/branching logic that distinguishes a name collision from any other bad request (the only discriminator this service offers, since BadRequestException is its single client error) has nothing to match on, and support/debug tooling that logs requestId logs nothing.

Fix: the service's modeled-error serializer - BadRequestException must render its text under the wire key AWS uses (`message`) and the error envelope must carry `requestId`; separately, the CreateChannel handler's duplicate-name branch must emit AWS's fixed sentence instead of an interpolated one.

Traces: mediatailor-channel-tags-and-schedule-lifecycle#task#9.

Cause 2 of 4 - DescribeChannel on a channel that does not exist answers 400 BadRequestException instead of 404, with no message or requestId (fixture mediatailor-channel-tags-and-schedule-lifecycle, step task#10).

AWS: Smithy model - DescribeChannel: GET /channel/{ChannelName}, success 200, errors [] (recorded behavior only for the miss). The recorded response for a channel name the account does not hold is HTTP 404 whose body carries `message` = "The specified channel doesn't exist." - FIXED text with no variable part (the requested channel name is NOT interpolated) - plus `requestId`. The recorded x-amzn-errortype header is present but redacted in the fixture, and it is NOT BadRequestException.

Emulator: HTTP 400 where AWS returns 404; x-amzn-errortype: BadRequestException, which fails the recorded header's shape; and $.message and $.requestId both absent. Source: src/ops/describe_channel.rs lines 168-175 widens the service's BadRequestException (HTTP 400) in via extra_errors and refuses the miss with text "Channel <ChannelName> not found." - wrong status, wrong error type and, per Cause 1, no `message` key on the wire at all.

Why: a not-found is indistinguishable from a malformed request. Clients and waiters branch on 404 (and on the not-found error type) to mean "absent", and on 400 to mean "my request is wrong, do not retry, do not create" - so create-if-absent and delete-if-present logic takes the opposite branch, and SDK error-type matching for the not-found case never fires.

Fix: the DescribeChannel handler's miss branch plus the service's error classification - the refusal must carry AWS's 404 status and its not-found error type, with AWS's fixed message text; the member/requestId rendering is the shared serializer fix from Cause 1.

Traces: mediatailor-channel-tags-and-schedule-lifecycle#task#10.

Cause 3 of 4 - ConfigureLogsForPlaybackConfiguration's PercentEnabled refusal carries no message or requestId (fixture mediatailor-playback-configuration-log-configuration, step task#4).

AWS: Smithy model - ConfigureLogsForPlaybackConfiguration: PUT /configureLogs/playbackConfiguration, success 200, errors []; input PercentEnabled -> __integer, @required, default 0, and its documentation gives the valid range as 0-100 with no modeled @range. Recorded behavior for a PercentEnabled above the maximum is HTTP 400 with `message` = "PercentEnabled must not exceed 100" - fixed text "PercentEnabled must not exceed " plus the limit (100, the documented maximum, a constant of the rule rather than a per-run value) - plus `requestId`.

Emulator: HTTP 400 (status matches) with $.message and $.requestId both absent. The handler computes the correct sentence (src/ops/configure_logs_for_playback_configuration.rs lines 219-244 formats "PercentEnabled must not exceed {MAX_PERCENT_ENABLED}") and raises the modeled BadRequestException, so the text is right and only the serialization loses it - the same shared defect as Cause 1. (Open issue "Implement mediatailor ConfigureLogsForPlaybackConfiguration: unimplemented scaffold stub" does not cover this: the operation is implemented and validating, and the replay reported this step as unowned.)

Why: the caller is told its request is bad without being told which member or which limit, so it cannot correct the request from the response; validation-message assertions in client test suites fail.

Fix: the shared modeled-error serializer from Cause 1 (render the text under `message`, add `requestId`). No change to the handler's validation, which already produces AWS's wording.

Traces: mediatailor-playback-configuration-log-configuration#task#4.

Cause 4 of 4 - ConfigureLogsForPlaybackConfiguration on an unknown playback configuration answers 400 BadRequestException instead of 404, with the wrong text and no message/requestId (fixture mediatailor-playback-configuration-log-configuration, step task#5).

AWS: Smithy model - same operation as Cause 3, errors [] (recorded behavior only for the miss). The recorded response for a PlaybackConfigurationName the account does not hold is HTTP 404 with `message` = "PlaybackConfiguration <name> not found" - fixed text "PlaybackConfiguration " + " not found" around ONE variable part, the PlaybackConfigurationName taken verbatim from the request - plus `requestId`. The recorded x-amzn-errortype is present but redacted and is not BadRequestException.

Emulator: HTTP 400 where AWS returns 404; x-amzn-errortype: BadRequestException, failing the recorded header's shape; $.message and $.requestId absent. The handler's miss branch (src/ops/configure_logs_for_playback_configuration.rs lines 266-282) raises the widened BadRequestException with the non-AWS text "No playback configuration is named <name>." instead of AWS's "PlaybackConfiguration <name> not found".

Why: identical to Cause 2 - a missing configuration reads as a malformed request, so configure-if-exists / create-then-configure flows and waiters take the wrong branch and do not retry or create; and no message text is available to disambiguate.

Fix: this operation's miss branch must answer AWS's 404 and not-found error type with AWS's message template (fixed text around the requested name), and the shared serializer fix from Cause 1 must put it on the wire under `message` with a `requestId`.

Traces: mediatailor-playback-configuration-log-configuration#task#5.

## Evidence
Trace set simparity-wire-aws-v1.

Cause 1 - fixture mediatailor-channel-tags-and-schedule-lifecycle, step task#9, operation CreateChannel, verdict mismatch, expected status 400, actual status 400:
  $.requestId: missing (expected '<shaped-not-asserted>', got None)
  $.message: missing (expected 'This channel already exists. Create a unique channel and resubmit your request.', got None)

Cause 2 - fixture mediatailor-channel-tags-and-schedule-lifecycle, step task#10, operation DescribeChannel, verdict mismatch:
  status: expected 404, got 400
  $.requestId: missing (expected '<shaped-not-asserted>', got None)
  $.message: missing (expected "The specified channel doesn't exist.", got None)
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'BadRequestException')

Cause 3 - fixture mediatailor-playback-configuration-log-configuration, step task#4, operation ConfigureLogsForPlaybackConfiguration, verdict mismatch, expected status 400, actual status 400:
  $.requestId: missing (expected '<shaped-not-asserted>', got None)
  $.message: missing (expected 'PercentEnabled must not exceed 100', got None)

Cause 4 - fixture mediatailor-playback-configuration-log-configuration, step task#5, operation ConfigureLogsForPlaybackConfiguration, verdict mismatch:
  status: expected 404, got 400
  $.requestId: missing (expected '<shaped-not-asserted>', got None)
  $.message: missing (expected 'PlaybackConfiguration sim2real-probe-mediatailor-missing-pc-ec94ccf1 not found', got None)
                     ^ fixed text 'PlaybackConfiguration ' + ' not found' around the PlaybackConfigurationName from the request;
                       'sim2real-probe-mediatailor-missing-pc-ec94ccf1' is this run's name, not part of the template
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'BadRequestException')

Model evidence (smithy_query query="operation" operation="CreateChannel,DescribeChannel,ConfigureLogsForPlaybackConfiguration", query="shape" shape="BadRequestException"):
  CreateChannel:                         POST /channel/{ChannelName}, code 200, errors []
  DescribeChannel:                       GET  /channel/{ChannelName}, code 200, errors []
  ConfigureLogsForPlaybackConfiguration: PUT  /configureLogs/playbackConfiguration, code 200, errors [];
                                         input PercentEnabled -> __integer, REQUIRED, default 0, no modeled @range (doc: valid values 0-100)
  BadRequestException: client error, httpError 400, members { Message: __string }  <- member name `Message`, NO jsonName, so the
                       serializer emits `Message` while AWS's envelope uses lowercase `message`; no member models requestId.

Code citations:
  services/service-com-amazonaws-mediatailor-mediatailor/src/ops/create_channel.rs:373-383 (duplicate-name BadRequestException, interpolated text)
  services/service-com-amazonaws-mediatailor-mediatailor/src/ops/describe_channel.rs:97-104 (BadRequestException widened in via extra_errors) and 168-175 (miss -> 400 "Channel <name> not found.")
  services/service-com-amazonaws-mediatailor-mediatailor/src/ops/configure_logs_for_playback_configuration.rs:219-244 (PercentEnabled refusal text) and 266-282 (miss -> 400 "No playback configuration is named <name>.")
  services/service-com-amazonaws-mediatailor-mediatailor/src/scaffolded.rs:117-144 (synthetic errors DO serialize lowercase `message`, so only modeled errors lose it)

Related open issues (neither covers these steps; the replay reported all four as unowned):
  20261030T005241Z-mediatailor-createchannel-duplicate-name-refusal-uses-a-non.md (message TEXT only, behavioral corpus)
  20261029T174215Z-mediatailor-configurelogsforplaybackconfiguration-stub.md (stub; the operation now answers and validates)

## Steps to Reproduce
1. git_clone() the repo and boot the emulator.
2. Cause 1 - duplicate channel name:
   aws mediatailor create-channel --channel-name dup --outputs '[{"ManifestName":"default","SourceGroup":"default","HlsPlaylistSettings":{"ManifestWindowSeconds":30}}]' --playback-mode LOOP --tier BASIC
   aws mediatailor create-channel --channel-name dup --outputs '[{"ManifestName":"other","SourceGroup":"other","HlsPlaylistSettings":{"ManifestWindowSeconds":60}}]' --playback-mode LOOP --tier BASIC
   -> HTTP 400 whose body must contain message = "This channel already exists. Create a unique channel and resubmit your request." and a requestId; the emulator's body carries neither key.
3. Cause 2 - unknown channel:
   aws mediatailor describe-channel --channel-name no-such-channel
   -> must be HTTP 404 with a not-found error type and message "The specified channel doesn't exist."; the emulator answers HTTP 400, x-amzn-errortype: BadRequestException, no message/requestId.
4. Cause 3 - PercentEnabled above the maximum (on an existing playback configuration):
   aws mediatailor put-playback-configuration --name pc-probe --ad-decision-server-url http://example.com/ads --video-content-source-url http://example.com/vod
   aws mediatailor configure-logs-for-playback-configuration --percent-enabled 101 --playback-configuration-name pc-probe
   -> HTTP 400 whose body must contain message = "PercentEnabled must not exceed 100" and a requestId; the emulator's body carries neither.
5. Cause 4 - unknown playback configuration:
   aws mediatailor configure-logs-for-playback-configuration --percent-enabled 10 --playback-configuration-name no-such-pc
   -> must be HTTP 404 with a not-found error type and message "PlaybackConfiguration no-such-pc not found"; the emulator answers HTTP 400, x-amzn-errortype: BadRequestException, no message/requestId.
6. Inspect raw bodies with curl -i to confirm the key casing of the error member.
7. Re-replay with replay_wire_corpus: mediatailor-channel-tags-and-schedule-lifecycle steps task#9 and task#10 and mediatailor-playback-configuration-log-configuration steps task#4 and task#5 must all pass.

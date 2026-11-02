Issue-Source: trace-sweep-divergence
Issue-Trace: medialive-cmc-traj-create-channel-request-mistakes
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medialive-cmc-traj-create-channel-request-mistakes medialive-cmc-traj-input-request-mistakes medialive-cmc-traj-list-channels-paging-mistakes medialive-cmc-traj-role-mistakes-and-idempotent-retry medialive-cmc-traj-channel-id-mistakes
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive|CreateChannel|4xx->2xx|$.message
Issue-Key: medialive|CreateChannel|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: medialive|CreateChannel|4xx->2xx|status
Issue-Key: medialive|CreateChannel|4xx->4xx|$.message
Issue-Key: medialive|CreateChannel|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: medialive|CreateChannel|4xx->4xx|status
Issue-Key: medialive|CreateInput|4xx->4xx|$.message
Issue-Key: medialive|CreateInput|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: medialive|ListChannels|4xx->4xx|$.message
Issue-Key: medialive|ListChannels|4xx->2xx|$.message
Issue-Key: medialive|ListChannels|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: medialive|ListChannels|4xx->2xx|status
Issue-Key: medialive|CreateChannel|4xx->2xx|$.validationErrors
Issue-Key: medialive|DescribeChannel|4xx->4xx|$.message
Issue-Key: medialive|DescribeChannel|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: medialive|DescribeChannel|4xx->4xx|status
Issue-Key: medialive|DescribeInput|4xx->4xx|$.message
Issue-Key: medialive|DescribeInput|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: medialive|DescribeInput|4xx->4xx|status
Issue-Commit: 54f388b9499ec3dd7b64bc533fb6b62161b4e3d7

# medialive request validation missing or misworded across channel and input operations

> _Note: this issue was identified against the code base at commit `54f388b9499ec3dd7b64bc533fb6b62161b4e3d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
MediaLive's request-validation layer diverges from AWS in three ways: checks AWS performs are absent (a bad request is accepted with a success status), the refusal is raised by the generic Smithy constraint validator instead of MediaLive's own BadRequestException wording, or it is raised as the wrong HTTP status/error type. This bundle has 10 causes; all ten must be closed.

Cause 1 of 10 - CreateChannel accepts a request with no requestId (fixture medialive-cmc-traj-create-channel-request-mistakes, step task#2).
AWS: CreateChannel (Smithy: POST /prod/channels, code 201; input member RequestId jsonName "requestId", idempotencyToken) refuses a request whose requestId is absent or empty with 400 BadRequestException and the fixed message "Non-empty channel request id required" (no variable parts).
Emulator: 201 with a created channel; no message and no x-amzn-errortype header.
Why: the idempotency token is what makes a retried create safe; a client that omits it gets a channel instead of the error that tells it to supply one, and every retry mints another channel.
Fix: CreateChannel's request validation, before any state mutation.
Traces: medialive-cmc-traj-create-channel-request-mistakes#task#2 (cluster also covers task#4, task#5, task#6 of the same fixture).

Cause 2 of 10 - CreateChannel with an empty name answers the wrong status, error type and message (fixture medialive-cmc-traj-create-channel-request-mistakes, step task#1).
AWS: 400 BadRequestException with the fixed message "Non-empty channel name required".
Emulator: 422 with x-amzn-errortype "UnprocessableEntityException" and the message "Channel contains errors." (Smithy models both BadRequestException httpError 400 and UnprocessableEntityException httpError 422 on CreateChannel, so the emulator picked the wrong one).
Why: a client branching on the 400/BadRequestException pair - or on the message to tell the user which field is wrong - sees a retry-shaped 422 and an opaque message naming no member.
Fix: CreateChannel's request validation and its error mapping.
Traces: medialive-cmc-traj-create-channel-request-mistakes#task#1.

Cause 3 of 10 - CreateChannel's invalid-enum refusal is worded by the generic constraint validator (fixture medialive-cmc-traj-create-channel-request-mistakes, step task#3).
AWS: a channelClass outside enum ChannelClass is refused 400 with MediaLive's own wording: the fixed text "Invalid channel request '" + the request body as submitted + "' Invalid enum value'" + the rejected value + "'". The variable parts are the request JSON and the offending value; the status 400 and that wording are the requirement.
Emulator: 400 but with x-amzn-errortype "ValidationException" and the framework message "1 validation error detected: Value '<v>' at 'channelClass' failed to satisfy constraint: Member must satisfy enum value set: [STANDARD, SINGLE_PIPELINE]" - an error type MediaLive does not model on CreateChannel.
Why: clients matching on the modeled MediaLive error types never match ValidationException, so the refusal is classified as an unknown error.
Fix: CreateChannel's input deserialization/validation - enum rejections must surface as MediaLive's BadRequestException with MediaLive's text, not the shared constraint validator's.
Traces: medialive-cmc-traj-create-channel-request-mistakes#task#3.

Cause 4 of 10 - CreateInput's invalid-enum refusal is worded by the generic constraint validator (fixture medialive-cmc-traj-input-request-mistakes, step task#2).
AWS: a type outside enum InputType is refused 400 with the fixed message "Invalid input or ISG request Invalid enum value" (no variable parts; Smithy: CreateInput POST /prod/inputs, input member Type -> InputType, errors include BadRequestException httpError 400).
Emulator: 400 with x-amzn-errortype "ValidationException" and "1 validation error detected: Value 'rtmp_push' at 'type' failed to satisfy constraint: Member must satisfy enum value set: [UDP_PUSH, RTP_PUSH, ...]". CreateInput does not model ValidationException at all.
Why: same as cause 3 - an unmodeled error type and a message no AWS client expects.
Fix: CreateInput's input validation and error mapping.
Traces: medialive-cmc-traj-input-request-mistakes#task#2.

Cause 5 of 10 - CreateInput validates in the wrong order, so a missing requestId is reported as a security-group problem (fixture medialive-cmc-traj-input-request-mistakes, step task#3).
AWS: CreateInput (input member RequestId jsonName "requestId", idempotencyToken) checks the request id first and refuses a request with none with 400 and the fixed message "Non-empty input request id required", regardless of what else in the request is wrong.
Emulator: 400 but with the message "inputSecurityGroups references input security group <id>, which this account does not hold" - it validated the referenced security group first and never reported the missing request id.
Why: the client is told to fix a security group when the actual defect is the absent idempotency token; after creating the group the request still fails, now for a reason the first response never mentioned.
Fix: the order of checks in CreateInput's request validation (request-id presence before reference resolution).
Traces: medialive-cmc-traj-input-request-mistakes#task#3 (cluster also covers task#4 and task#5 of the same fixture).

Cause 6 of 10 - ListChannels' maxResults=0 refusal is worded by the emulator (fixture medialive-cmc-traj-list-channels-paging-mistakes, step task#1).
AWS: ListChannels (Smithy: GET /prod/channels, input member MaxResults httpQuery "maxResults"; the model carries no range constraint, so this is recorded behavior only) refuses maxResults=0 with 400 and the fixed message "The maximum number of results must be greater than 0."
Emulator: 400 with its own wording "maxResults must be at least 1, got 0".
Why: the status matches but the message is the emulator's; anything surfacing or asserting on the AWS text diverges.
Fix: ListChannels' paging-parameter validation.
Traces: medialive-cmc-traj-list-channels-paging-mistakes#task#1.

Cause 7 of 10 - ListChannels accepts a maxResults above the service maximum (fixture medialive-cmc-traj-list-channels-paging-mistakes, step task#2).
AWS: a maxResults greater than 1000 is refused 400 with the fixed message "The maximum number of results you can request at once is 1000." (recorded behavior only - MaxResults carries no modeled range).
Emulator: 200 with a channel list; no message, no x-amzn-errortype.
Why: a client probing the page-size ceiling concludes an unsupported page size is supported and builds pagination that real MediaLive rejects.
Fix: ListChannels' paging-parameter validation (upper bound alongside the lower bound of cause 6).
Traces: medialive-cmc-traj-list-channels-paging-mistakes#task#2 (cluster also covers task#3 of the same fixture).

Cause 8 of 10 - CreateChannel accepts a roleArn that names no existing role (fixture medialive-cmc-traj-role-mistakes-and-idempotent-retry, step task#7).
AWS: CreateChannel refuses a roleArn that does not resolve to a role trusting medialive.amazonaws.com with 422 UnprocessableEntityException (Smithy: UnprocessableEntityException httpError 422, members Message jsonName "message" and ValidationErrors jsonName "validationErrors" of ValidationError{elementPath, errorMessage}). The message is the element path followed by the fixed text: "roleArn Make sure the role '<roleArn from the request>' exists in the user account and medialive.amazonaws.com is a trusted service", and validationErrors carries one entry with elementPath "roleArn" and that same sentence as errorMessage. The only variable part is the roleArn echoed from the request.
Emulator: 201 - the channel is created, with no message, no validationErrors and no x-amzn-errortype.
Why: the misconfiguration a client is testing for silently succeeds, so it never learns the role is wrong; worse, the accepted channel stays in state and corrupts later steps (see the lifecycle issue filed alongside this one).
Fix: CreateChannel's validation - resolve roleArn against IAM state and emit the modeled UnprocessableEntityException with its validationErrors list.
Traces: medialive-cmc-traj-role-mistakes-and-idempotent-retry#task#7 (cluster also covers task#8 of the same fixture).

Cause 9 of 10 - DescribeChannel treats a non-numeric channel id as a missing channel instead of a malformed one (fixture medialive-cmc-traj-channel-id-mistakes, step task#1).
AWS: MediaLive channel ids are numeric strings. DescribeChannel (Smithy: GET /prod/channels/{ChannelId}, ChannelId httpLabel required; errors include BadRequestException httpError 400 and NotFoundException httpError 404) rejects a syntactically invalid id with 400 and the fixed message "Id should include only numbers" - it never reaches the lookup.
Emulator: 404 with x-amzn-errortype "NotFoundException" and "Channel <id> not found".
Why: a client distinguishing "you passed garbage" (400, do not retry, fix the input) from "that channel is gone" (404, treat as deleted) draws the wrong conclusion and may treat a typo'd id as a successfully deleted channel.
Fix: DescribeChannel's path-parameter validation, ahead of the state lookup.
Traces: medialive-cmc-traj-channel-id-mistakes#task#1.

Cause 10 of 10 - DescribeInput has the same non-numeric-id defect (fixture medialive-cmc-traj-channel-id-mistakes, step task#4).
AWS: DescribeInput (Smithy: GET /prod/inputs/{InputId}, InputId httpLabel required) rejects a non-numeric input id with 400 and the fixed message "Id should include only numbers".
Emulator: 404 NotFoundException "Input <id> not found".
Why: as cause 9, for inputs.
Fix: DescribeInput's path-parameter validation (the same id-syntax check the channel path needs).
Traces: medialive-cmc-traj-channel-id-mistakes#task#4.

## Evidence
Expected vs actual, from the replay of simparity-wire-aws-v1 (all clusters verdict=mismatch, service=medialive):

1. CreateChannel, medialive-cmc-traj-create-channel-request-mistakes#task#2: status expected 400, got 201; $.message missing (expected 'Non-empty channel request id required'); $headers.x-amzn-errortype missing.
2. CreateChannel, ...#task#1: status expected 400, got 422; $.message expected 'Non-empty channel name required', got 'Channel contains errors.'; x-amzn-errortype got 'UnprocessableEntityException'.
3. CreateChannel, ...#task#3: status 400 both; $.message expected "Invalid channel request '<request body>' Invalid enum value'standard'", got "1 validation error detected: Value 'standard' at 'channelClass' failed to satisfy constraint: Member must satisfy enum value set: [STANDARD, SINGLE_PIPELINE]"; x-amzn-errortype got 'ValidationException'.
4. CreateInput, medialive-cmc-traj-input-request-mistakes#task#2: status 400 both; $.message expected 'Invalid input or ISG request Invalid enum value', got "1 validation error detected: Value 'rtmp_push' at 'type' failed to satisfy constraint: Member must satisfy enum value set: [UDP_PUSH, RTP_PUSH, RTMP_PUSH, ...]"; x-amzn-errortype got 'ValidationException'.
5. CreateInput, ...#task#3: status 400 both; $.message expected 'Non-empty input request id required', got 'inputSecurityGroups references input security group 9999999, which this account does not hold'.
6. ListChannels, medialive-cmc-traj-list-channels-paging-mistakes#task#1: status 400 both; $.message expected 'The maximum number of results must be greater than 0.', got 'maxResults must be at least 1, got 0'.
7. ListChannels, ...#task#2: status expected 400, got 200; $.message missing (expected 'The maximum number of results you can request at once is 1000.').
8. CreateChannel, medialive-cmc-traj-role-mistakes-and-idempotent-retry#task#7: status expected 422, got 201; $.message missing (expected "roleArn Make sure the role '<roleArn>' exists in the user account and medialive.amazonaws.com is a trusted service"); $.validationErrors missing (expected one entry {elementPath: 'roleArn', errorMessage: "Make sure the role '<roleArn>' exists in the user account and medialive.amazonaws.com is a trusted service"}).
9. DescribeChannel, medialive-cmc-traj-channel-id-mistakes#task#1: status expected 400, got 404; $.message expected 'Id should include only numbers', got 'Channel <id> not found'; x-amzn-errortype got 'NotFoundException'.
10. DescribeInput, ...#task#4: status expected 400, got 404; $.message expected 'Id should include only numbers', got 'Input <id> not found'; x-amzn-errortype got 'NotFoundException'.

Model references: CreateChannel errors {BadRequestException 400, ConflictException 409, UnprocessableEntityException 422, ...}; CreateInput errors {BadRequestException 400, ...} (no ValidationException on either); UnprocessableEntityException{message, validationErrors: [ValidationError{elementPath, errorMessage}]}; ListChannels MaxResults is httpQuery 'maxResults' with no modeled range (bounds are recorded behavior only); DescribeChannel/DescribeInput take a required httpLabel id and model both BadRequestException 400 and NotFoundException 404.

Recorded ids, names and request bodies above are per-run fillers for the variable slots of each message, not literal requirements.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1 (replay_wire_corpus); every step below is a recorded step of that corpus.
2. POST /prod/channels with a body that omits requestId -> expect 400 'Non-empty channel request id required', observe 201 (medialive-cmc-traj-create-channel-request-mistakes#task#2).
3. POST /prod/channels with name: "" -> expect 400 'Non-empty channel name required', observe 422 'Channel contains errors.' (#task#1).
4. POST /prod/channels with channelClass: "standard" -> expect MediaLive's "Invalid channel request '<body>' Invalid enum value'standard'", observe the generic ValidationException text (#task#3).
5. POST /prod/inputs with type: "rtmp_push" -> expect 400 'Invalid input or ISG request Invalid enum value', observe the generic ValidationException text (medialive-cmc-traj-input-request-mistakes#task#2).
6. POST /prod/inputs with no requestId and an inputSecurityGroups id that does not exist -> expect 400 'Non-empty input request id required', observe the security-group message (#task#3).
7. GET /prod/channels?maxResults=0 -> expect 'The maximum number of results must be greater than 0.', observe 'maxResults must be at least 1, got 0' (medialive-cmc-traj-list-channels-paging-mistakes#task#1).
8. GET /prod/channels?maxResults=<greater than 1000> -> expect 400 'The maximum number of results you can request at once is 1000.', observe 200 (#task#2).
9. POST /prod/channels with a roleArn naming a role that does not exist -> expect 422 with message and validationErrors[0].elementPath 'roleArn', observe 201 (medialive-cmc-traj-role-mistakes-and-idempotent-retry#task#7).
10. GET /prod/channels/MyMediaLiveChannel and GET /prod/inputs/MyRTMPInput -> expect 400 'Id should include only numbers' on both, observe 404 NotFoundException (medialive-cmc-traj-channel-id-mistakes#task#1 and #task#4).

Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-messaging-channel-create-describe-update-and-delete
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-messaging-channel-create-describe-update-and-delete chime-sdk-messaging-channel-membership-create-list-and-delete chime-sdk-messaging-channel-moderator-promote-list-and-demote
Issue-Shape-Id: com.amazonaws.chimesdkmessaging#ChimeMessagingService
Issue-Key: chime-sdk-messaging|DescribeChannel|4xx->4xx|$.Code
Issue-Key: chime-sdk-messaging|DescribeChannel|4xx->4xx|$.Message
Issue-Key: chime-sdk-messaging|DescribeChannel|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: chime-sdk-messaging|DescribeChannel|4xx->4xx|status
Issue-Key: chime-sdk-messaging|DescribeChannelMembership|4xx->4xx|$.Code
Issue-Key: chime-sdk-messaging|DescribeChannelMembership|4xx->4xx|$.Message
Issue-Key: chime-sdk-messaging|DescribeChannelMembership|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: chime-sdk-messaging|DescribeChannelMembership|4xx->4xx|status
Issue-Key: chime-sdk-messaging|DescribeChannelModerator|4xx->4xx|$.Message
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Reads of a channel resource the bearer cannot reach are refused with the wrong status, code and wording

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeChannel

## Description
THREE causes, one shared fault: when a read cannot resolve the channel resource for the calling `x-amz-chime-bearer`, this emulator answers its own store-shaped refusal (an echo of the request's ARNs, and twice the wrong status and error code) instead of the refusal the service models and words.

Cause 1 of 3: DescribeChannel on a channel the bearer cannot resolve answers 404 NotFoundException instead of 403 ForbiddenException.
  AWS: Smithy - `DescribeChannel` models exactly BadRequestException 400, ForbiddenException 403, UnauthorizedClientException 401, ThrottledClientException 429, ServiceFailureException 500, ServiceUnavailableException 503. `NotFoundException` is NOT in its error list, so a 404 is an error this operation cannot raise. The recording shows HTTP 403 with `Code` `Forbidden` (a member of the modelled `ErrorCode` enum) and `Message` "You do not have sufficient access to perform this action." - that sentence is entirely FIXED text naming no value from the request or the store, so it is the same sentence for every caller. This crate already states that rule for the sibling resource: `state.rs:52-67` documents that an ARN this service holds no channel flow for is refused with the modelled ForbiddenException because neither reader models NotFoundException.
  Emulator: HTTP 404, `Code` `NotFound`, `x-amzn-errortype` `NotFoundException`, `Message` "channel <ChannelArn> not found" - wording of its own, whose only variable part is the request's ChannelArn.
  Why: the SDK has no modelled shape for a 404 on this operation, so a client's `catch ForbiddenException` branch never runs and the failure surfaces as an unmodelled/unknown error; a client distinguishing "gone" from "not permitted" is told the opposite of what AWS says.
  Fix: the DescribeChannel handler's channel-resolution gate - refuse with the modelled ForbiddenException and the service's fixed sentence, single-homed as a const in `state.rs` beside `UNRESOLVABLE_APP_INSTANCE` (`state.rs:66`).
  Traces: chime-sdk-messaging-channel-create-describe-update-and-delete#task#8.

Cause 2 of 3: DescribeChannelMembership for an ARN that is not a member answers 404 NotFoundException instead of 403 ForbiddenException.
  AWS: Smithy - `DescribeChannelMembership` does model NotFoundException 404, but the recording shows the service answering this case with HTTP 403, `Code` `Forbidden` and `Message` "You do not have sufficient access to perform this action." - the same fixed sentence, no request values. So the access decision precedes the membership lookup: a membership that is not the bearer's to see is refused as forbidden, not reported as absent.
  Emulator: HTTP 404, `Code` `NotFound`, `x-amzn-errortype` `NotFoundException`, `Message` "<MemberArn> is not a member of channel <ChannelArn>" - its own wording, echoing two request ARNs.
  Why: a client that removes a member and then probes the membership gets a 404 "absent" answer where AWS gives a 403 "not permitted"; retry/authorization logic keyed on 403 never fires, and the emulator's message leaks the membership topology AWS deliberately does not disclose.
  Fix: the DescribeChannelMembership handler's authorization gate must run before the lookup and raise the modelled ForbiddenException with the shared fixed sentence.
  Traces: chime-sdk-messaging-channel-membership-create-list-and-delete#task#9.

Cause 3 of 3: DescribeChannelModerator's not-a-moderator refusal is worded by the emulator instead of the service.
  AWS: recorded behavior - status 404 and `Code` `NotFound` are correct here; the message is "App instance user is not a moderator of the specified Channel.", which is entirely FIXED text with no variable part (it names neither the user nor the channel).
  Emulator: same status and code, but `Message` "<ChannelModeratorArn> does not moderate channel <ChannelArn>" - wording of its own, built from two request ARNs.
  Why: an error message is part of the contract; a client or test asserting on AWS's sentence fails, and the substituted sentence echoes request values AWS keeps out of this refusal.
  Fix: single-home the fixed sentence as a const in `state.rs` (beside `UNRESOLVABLE_APP_INSTANCE`) and have the DescribeChannelModerator handler raise it verbatim.
  Traces: chime-sdk-messaging-channel-moderator-promote-list-and-demote#task#10.

## Evidence
Expected vs actual, per replay:
- DescribeChannel [chime-sdk-messaging-channel-create-describe-update-and-delete#task#8]: status expected 403, got 404; `$.Code` expected 'Forbidden', got 'NotFound'; `$.Message` expected 'You do not have sufficient access to perform this action.', got 'channel <arn>/channel/<id> not found'; `x-amzn-errortype` served as 'NotFoundException', an error DescribeChannel does not model.
- DescribeChannelMembership [chime-sdk-messaging-channel-membership-create-list-and-delete#task#9]: status expected 403, got 404; `$.Code` expected 'Forbidden', got 'NotFound'; `$.Message` expected 'You do not have sufficient access to perform this action.', got '<member arn> is not a member of channel <channel arn>'; `x-amzn-errortype` 'NotFoundException'.
- DescribeChannelModerator [chime-sdk-messaging-channel-moderator-promote-list-and-demote#task#10]: status 404 and Code matched; `$.Message` expected 'App instance user is not a moderator of the specified Channel.', got '<user arn> does not moderate channel <channel arn>'.
Fixed vs variable text: in all three AWS messages every word is fixed - none of them interpolates a request or store value; the emulator's three messages are wholly non-AWS wordings whose variable parts are the request's ARNs.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay chime-sdk-messaging-channel-create-describe-update-and-delete to step task#8: DescribeChannel on the channel the fixture has deleted answers 404/NotFound instead of 403/Forbidden.
3. Replay chime-sdk-messaging-channel-membership-create-list-and-delete to step task#9: DescribeChannelMembership for the removed member answers 404/NotFound instead of 403/Forbidden.
4. Replay chime-sdk-messaging-channel-moderator-promote-list-and-demote to step task#10: DescribeChannelModerator for the demoted user answers 404/NotFound with the emulator's own ARN-echoing sentence instead of 'App instance user is not a moderator of the specified Channel.'

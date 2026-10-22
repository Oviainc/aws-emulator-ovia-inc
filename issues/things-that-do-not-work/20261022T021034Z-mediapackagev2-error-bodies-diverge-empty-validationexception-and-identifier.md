Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackagev2-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackagev2-error-paths mediapackagev2-channel-group-lifecycle mediapackagev2-tagging-workflow mediapackagev2-channel-lifecycle mediapackagev2-multiple-origin-endpoints-isolation
Issue-Shape-Id: com.amazonaws.mediapackagev2#mediapackagev2
Issue-Key: mediapackagev2|CreateChannelGroup|4xx->4xx|$.Message
Issue-Key: mediapackagev2|CreateChannelGroup|4xx->4xx|$.ValidationExceptionType
Issue-Key: mediapackagev2|GetChannelGroup|4xx->4xx|$.Message
Issue-Key: mediapackagev2|GetChannel|4xx->4xx|$.Message
Issue-Key: mediapackagev2|GetOriginEndpoint|4xx->4xx|$.Message
Issue-Key: mediapackagev2|CreateChannel|4xx->4xx|$.Message
Issue-Key: mediapackagev2|DeleteChannelGroup|4xx->4xx|$.Message
Issue-Commit: 07653fe6d7ae123674bcbfa9037d42ab6af0ac40

# mediapackagev2 error bodies diverge: empty ValidationException and identifier-echoing not-found/conflict messages

> _Note: this issue was identified against the code base at commit `07653fe6d7ae123674bcbfa9037d42ab6af0ac40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannelGroup

## Description
mediapackagev2 rejects requests with the right HTTP status but the wrong error body. One path returns an empty body with none of the exception's modeled members; the rest build the message by interpolating the requested resource names, whereas AWS returns a fixed, resource-type-specific sentence that never contains the caller's identifiers. 7 causes.

Cause 1 of 7: ValidationException body carries neither Message nor ValidationExceptionType.
AWS: Smithy: ValidationException (httpError 400) binds Message (String) and ValidationExceptionType (enum ValidationExceptionType, whose values include MEMBER_DOES_NOT_MATCH_PATTERN). CreateChannelGroupRequest.ChannelGroupName is ResourceName with @pattern ^[a-zA-Z0-9_-]+$ and @length 1..256, so a name with an unsupported character is a pattern violation. The message here is a requirement because it is built from the request: it names the rejected member path and restates the constraint — recorded form 'Validation error encountered at request.channelGroupName: Member must contain only supported characters. Supported characters are A-Z, a-z, 0-9, _ (underscore), and - (hyphen). Check the value you specified in your request and then try again' — and the discriminator ValidationExceptionType is MEMBER_DOES_NOT_MATCH_PATTERN for that class of violation.
Emulator: status 400 is right, but both members are absent from the body (missing, got None), i.e. the rejection is serialized with an empty payload.
Why: an SDK deserializes a ValidationException with no message and no type, so the caller cannot tell which member failed or why; code that branches on ValidationExceptionType (pattern vs missing vs length) has nothing to branch on, and retry/report logic sees an unexplained 400.
Fix: the mediapackagev2 input-validation path and the ValidationException serializer — emit both modeled members, with the type discriminator set from the violated constraint and the message naming the offending member path and its constraint.
Traces: mediapackagev2-error-paths#task#4 (CreateChannelGroup with an unsupported character in channelGroupName).

Cause 2 of 7: channel-group not-found message echoes the requested name.
AWS: Smithy: ResourceNotFoundException (httpError 404) binds Message and ResourceTypeNotFound. Recorded behavior only for the text, but the rule the recording establishes is that the message is a fixed per-resource-type sentence independent of the request: "MediaPackage can't process your request because we can't find your channel group. Verify your channel group name or add a channel group and then try again" — it contains no channel-group name.
Emulator: returns 404 with a message interpolating the name, e.g. 'Channel group <name> not found'.
Why: callers and test suites that match on AWS's documented message (or that redact logs on the assumption the service does not echo their resource identifiers back) see a different, request-dependent string; the divergence is in the rule producing the message, not merely in the recorded wording.
Fix: the mediapackagev2 not-found error construction for the channel-group resource — use the service's fixed message text rather than formatting the requested name into it.
Traces: mediapackagev2-channel-group-lifecycle#teardown#7 (GetChannelGroup after delete), mediapackagev2-tagging-workflow#teardown#8 (same cluster).

Cause 3 of 7: channel not-found message echoes the requested channel and group names.
AWS: ResourceNotFoundException, 404. Recorded behavior only: the fixed sentence is "MediaPackage can't process your request because we can't find your channel. Verify your channel name or add a channel and then try again", with no identifiers in it.
Emulator: 'Channel <name> not found in channel group <group>'.
Why: as Cause 2, at channel scope.
Fix: the mediapackagev2 not-found error construction for the channel resource.
Traces: mediapackagev2-channel-lifecycle#teardown#9 (GetChannel after delete), mediapackagev2-error-paths#task#2 (same cluster).

Cause 4 of 7: origin-endpoint not-found message echoes the endpoint, channel and group names.
AWS: ResourceNotFoundException, 404. Recorded behavior only: "MediaPackage can't process your request because we can't find your origin endpoint. Verify your origin endpoint name or add an origin endpoint and then try again".
Emulator: 'Origin endpoint <name> not found on channel <channel> in channel group <group>'.
Why: as Cause 2, at origin-endpoint scope.
Fix: the mediapackagev2 not-found error construction for the origin-endpoint resource.
Traces: mediapackagev2-multiple-origin-endpoints-isolation#teardown#10 and #teardown#11 (same cluster).

Cause 5 of 7: CreateChannel into a missing channel group reports the group name in the message.
AWS: Smithy: CreateChannel binds ResourceNotFoundException (404) and its ChannelGroupName is an @httpLabel, so a create against an absent group is a parent-not-found. Recorded behavior only: AWS returns the same fixed channel-group sentence as Cause 2, "MediaPackage can't process your request because we can't find your channel group. Verify your channel group name or add a channel group and then try again".
Emulator: 404 with 'Channel group <name> not found'.
Why: the create path and the read path must report a missing parent identically; here both diverge from AWS and in a request-dependent way.
Fix: route the CreateChannel parent-lookup failure through the same fixed channel-group not-found message as Cause 2.
Traces: mediapackagev2-error-paths#task#1.

Cause 6 of 7: duplicate-create conflict message names the colliding resource.
AWS: Smithy: ConflictException (httpError 409) binds Message and ConflictExceptionType (values include RESOURCE_ALREADY_EXISTS); CreateChannelGroup declares it. Recorded behavior only for the text: a fixed, resource-agnostic sentence "MediaPackage can't create the resource you requested because a resource with that name already exists. Verify the name of the resource and then try again".
Emulator: 409 with 'Channel group <name> already exists'.
Why: idempotency/retry logic that recognises AWS's already-exists wording does not recognise the emulator's, so a benign duplicate create is misclassified as an unknown conflict.
Fix: the mediapackagev2 conflict error construction on the create paths.
Traces: mediapackagev2-error-paths#task#6 (CreateChannelGroup for an existing group).

Cause 7 of 7: delete-with-children conflict message reports a child count.
AWS: Smithy: DeleteChannelGroup binds ConflictException (409) (ConflictExceptionType value RESOURCE_IN_USE covers this class). Recorded behavior only: the fixed sentence 'The channel group you tried to delete has channels attached to it. If you want to delete this channel group, you must first delete the channels attached to it'.
Emulator: 409 with 'Channel group <name> still has N channel(s); delete them first' — both the name and a count AWS does not report.
Why: the status is right but the body is request-derived where AWS's is fixed; a client matching AWS's wording to drive a cascade-delete retry does not match.
Fix: the mediapackagev2 DeleteChannelGroup conflict error construction.
Traces: mediapackagev2-error-paths#task#8.

## Evidence
Cause 1 — mediapackagev2-error-paths#task#4 CreateChannelGroup, expected 400 / actual 400: $.Message missing (expected 'Validation error encountered at request.channelGroupName: Member must contain only supported characters. Supported characters are A-Z, a-z, 0-9, _ (underscore), and - (hyphen). Check the value you specified in your request and then try again', got None); $.ValidationExceptionType missing (expected 'MEMBER_DOES_NOT_MATCH_PATTERN', got None).

Cause 2 — mediapackagev2-channel-group-lifecycle#teardown#7 GetChannelGroup, expected 404 / actual 404: $.Message value_mismatch (expected "MediaPackage can't process your request because we can't find your channel group. Verify your channel group name or add a channel group and then try again", got 'Channel group sim2real-probe-mediapackagev2-mpv2-group-0c61ab0f not found'). Also affects mediapackagev2-tagging-workflow#teardown#8.

Cause 3 — mediapackagev2-channel-lifecycle#teardown#9 GetChannel, 404/404: expected "MediaPackage can't process your request because we can't find your channel. Verify your channel name or add a channel and then try again", got 'Channel sim2real-probe-mediapackagev2-mpv2-channel-0c4343e8 not found in channel group sim2real-probe-mediapackagev2-mpv2-group-0c4343e8'. Also affects mediapackagev2-error-paths#task#2.

Cause 4 — mediapackagev2-multiple-origin-endpoints-isolation#teardown#10 GetOriginEndpoint, 404/404: expected "MediaPackage can't process your request because we can't find your origin endpoint. Verify your origin endpoint name or add an origin endpoint and then try again", got 'Origin endpoint sim2real-probe-mediapackagev2-mpv2-ep-a-f33c8145 not found on channel ... in channel group ...'. Also affects #teardown#11.

Cause 5 — mediapackagev2-error-paths#task#1 CreateChannel, 404/404: expected the fixed channel-group sentence, got 'Channel group sim2real-probe-mediapackagev2-mpv2-missing-group-d387fced not found'.

Cause 6 — mediapackagev2-error-paths#task#6 CreateChannelGroup, 409/409: expected "MediaPackage can't create the resource you requested because a resource with that name already exists. Verify the name of the resource and then try again", got 'Channel group sim2real-probe-mediapackagev2-mpv2-group-d387fced already exists'.

Cause 7 — mediapackagev2-error-paths#task#8 DeleteChannelGroup, 409/409: expected 'The channel group you tried to delete has channels attached to it. If you want to delete this channel group, you must first delete the channels attached to it', got 'Channel group sim2real-probe-mediapackagev2-mpv2-group-d387fced still has 1 channel(s); delete them first'.

The quoted resource names are per-run probe names and are examples; what diverges is that the emulator's messages are built from the request at all, where AWS's are fixed per resource type, and that the 400 carries no modeled members whatsoever.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus); trace set simparity-wire-aws-v1.
2. Cause 1: fixture mediapackagev2-error-paths, step task#4 — CreateChannelGroup with a channelGroupName containing a character outside ^[a-zA-Z0-9_-]+$; inspect the 400 body for Message and ValidationExceptionType.
3. Causes 2-4: fixtures mediapackagev2-channel-group-lifecycle (teardown#7), mediapackagev2-channel-lifecycle (teardown#9) and mediapackagev2-multiple-origin-endpoints-isolation (teardown#10) — Get the resource after deleting it and inspect the 404 Message.
4. Cause 5: fixture mediapackagev2-error-paths, step task#1 — CreateChannel naming a channel group that does not exist; inspect the 404 Message.
5. Cause 6: fixture mediapackagev2-error-paths, step task#6 — CreateChannelGroup with the name of an existing group; inspect the 409 Message.
6. Cause 7: fixture mediapackagev2-error-paths, step task#8 — DeleteChannelGroup while a channel is still attached; inspect the 409 Message.

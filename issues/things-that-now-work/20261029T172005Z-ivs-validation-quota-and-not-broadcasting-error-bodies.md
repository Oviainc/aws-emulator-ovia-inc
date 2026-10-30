Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-ad-configuration-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-ad-configuration-error-paths ivs-channel-error-paths ivs-playback-key-pair-error-paths ivs-stream-key-management ivs-stream-operations-not-broadcasting ivs-viewer-session-revocation
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs|CreateAdConfiguration|4xx->4xx|$.Code
Issue-Key: ivs|CreateAdConfiguration|4xx->4xx|$.Message
Issue-Key: ivs|CreateAdConfiguration|4xx->4xx|$.RequestID
Issue-Key: ivs|CreateChannel|4xx->4xx|$.Code
Issue-Key: ivs|CreateChannel|4xx->4xx|$.Message
Issue-Key: ivs|ImportPlaybackKeyPair|4xx->4xx|$.Code
Issue-Key: ivs|ImportPlaybackKeyPair|4xx->4xx|$.Message
Issue-Key: ivs|CreateStreamKey|4xx->4xx|$.Code
Issue-Key: ivs|CreateStreamKey|4xx->4xx|$.Message
Issue-Key: ivs|CreateStreamKey|4xx->4xx|$.RequestID
Issue-Key: ivs|GetStream|4xx->4xx|$.Code
Issue-Key: ivs|GetStream|4xx->4xx|$.Message
Issue-Key: ivs|StopStream|4xx->4xx|$.Code
Issue-Key: ivs|StopStream|4xx->4xx|$.Message
Issue-Key: ivs|PutMetadata|4xx->4xx|$.Code
Issue-Key: ivs|PutMetadata|4xx->4xx|$.Message
Issue-Key: ivs|StartViewerSessionRevocation|4xx->4xx|$.Code
Issue-Key: ivs|StartViewerSessionRevocation|4xx->4xx|$.Message
Issue-Key: ivs|StartViewerSessionRevocation|4xx->4xx|$.RequestID
Issue-Commit: 4fccfcb9e83e9e12ca223d9fc117bac8a0390f59

# IVS validation, quota and not-broadcasting error bodies omit Code and Message (8 causes)

> _Note: this issue was identified against the code base at commit `4fccfcb9e83e9e12ca223d9fc117bac8a0390f59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAdConfiguration

## Description
The same empty-error-envelope defect as the not-found bundle, on the service's OTHER error classes: ValidationException (400), ServiceQuotaExceededException (402), ChannelNotBroadcasting (404) and StartViewerSessionRevocation's ResourceNotFoundException (404). On every one of the eight failing steps the HTTP status is correct and the body resolves neither the error code nor the message the recorded AWS response carries. EIGHT causes, each its own fixture step; all must be closed.

Shared AWS behavior. Smithy (com.amazonaws.ivs, restJson1): ValidationException is a service-common error, errorKind client, @httpError 400 and models the `x-amzn-ErrorType` header; ServiceQuotaExceededException is @httpError 402 and declared by CreateStreamKey; ChannelNotBroadcasting is @httpError 404 (doc: "The stream is offline for the given channel ARN") and declared by GetStream, StopStream and PutMetadata; ResourceNotFoundException is @httpError 404 and declared by StartViewerSessionRevocation. Recorded behavior for the envelope: the body carries `Code`, `Message` and `RequestID`, and 400s also carry the `x-amzn-errortype` header naming the code. The message texts, by class, with their fixed and variable parts:
- boundary constraint refusal: fixed form `<Operation>.<wire member path> does not match the required pattern <pattern>`; the operation, the dotted member path and the pattern come from the request and the model.
- handler-enforced member refusal: fixed form `Request member: <member> failed to satisfy the constraints: <what is wrong>`.
- quota refusal: fixed form `Resource: <resource type> quota exceeded limit: <limit>`, with the resource type and the account limit from the service's quota rule.
- channel offline: fixed text `Channel: ` + the channel ARN from the request + ` is not currently online`.
- not found: `Resource: <arn> not found`.

Shared emulator behavior: correct status on all eight, no `Code` and no `Message` in the body (and no `x-amzn-errortype` where the recording asserts it). crate::state's message rules (validation/quota_exceeded/channel_not_broadcasting, state.rs:836-911, and the boundary wording in violation_clause, state.rs:962+) build the sentences and hand them to Scaffolded::stated, so the loss is in the error rendering, not the rules.

Why: a client cannot distinguish a validation refusal from a quota refusal from an offline channel other than by HTTP status — 404 alone cannot tell ChannelNotBroadcasting (retry later, the channel exists) from ResourceNotFoundException (the channel is gone), which are opposite client actions. No message means no field-level diagnostics either.

Fix: the error-response rendering in crate::scaffolded (the Stated body's key set and the error-type header), the same component as the not-found bundle; no handler change is needed for these eight.

Cause 1 of 8: CreateAdConfiguration with a mediaTailorPlaybackConfigurations[0].playbackConfigurationArn that does not match the modelled MediaTailor ARN pattern. Fixture ivs-ad-configuration-error-paths, step task#1. Expected 400, Code 'ValidationException', Message of the boundary form naming CreateAdConfiguration, the member path mediaTailorPlaybackConfiguration.0.playbackConfigurationArn and the pattern ^arn:aws:mediatailor:[a-z0-9-]+:[0-9]+:playbackConfiguration/[a-zA-Z0-9-]+$; got 400 with no Code/Message/RequestID.
Cause 2 of 8: CreateChannel with a name breaking ChannelName's @pattern ^[a-zA-Z0-9-_]*$. Fixture ivs-channel-error-paths, step task#2. Expected Code 'ValidationException', Message 'CreateChannel.name does not match the required pattern ^[a-zA-Z0-9-_]*$'; got neither.
Cause 3 of 8: ImportPlaybackKeyPair with publicKeyMaterial that is not a decodable PEM. Fixture ivs-playback-key-pair-error-paths, step task#1. Expected Code 'ValidationException', Message 'Request member: publicKeyMaterial failed to satisfy the constraints: could not decode as a PEM file'; got neither.
Cause 4 of 8: CreateStreamKey past the per-channel stream-key quota. Fixture ivs-stream-key-management, step task#5. Expected 402, Code 'ServiceQuotaExceededException', Message 'Resource: stream-key quota exceeded limit: <limit>' (limit 1 in the recording); got 402 with no Code/Message.
Cause 5 of 8: GetStream on a channel with no live stream. Fixture ivs-stream-operations-not-broadcasting, step task#2 (cluster also covers #task#3). Expected Code 'ChannelNotBroadcasting', Message 'Channel: <channel arn> is not currently online'; got neither.
Cause 6 of 8: StopStream on the same idle channel. Fixture ivs-stream-operations-not-broadcasting, step task#4. Same expected envelope.
Cause 7 of 8: PutMetadata on the same idle channel. Fixture ivs-stream-operations-not-broadcasting, step task#5. Same expected envelope.
Cause 8 of 8: StartViewerSessionRevocation naming a channel ARN that does not exist. Fixture ivs-viewer-session-revocation, step task#3. Expected 404, Code 'ResourceNotFoundException', Message 'Resource: <channel arn> not found'; got 404 with neither.

The open behavioral issue 20261023T004057Z-ivs-7-behavioural-divergences-error-messages-dropped-on.md describes the same empty-message mechanism for the behavioral suite but carries none of these replay keys, so these eight are filed here.

## Evidence
All eight steps replayed with the correct status and an empty error envelope:
- CreateAdConfiguration, ivs-ad-configuration-error-paths#task#1: status 400 -> 400; $.Code expected 'ValidationException' got None; $.Message expected 'CreateAdConfiguration.mediaTailorPlaybackConfiguration.0.playbackConfigurationArn does not match the required pattern ^arn:aws:mediatailor:[a-z0-9-]+:[0-9]+:playbackConfiguration/[a-zA-Z0-9-]+$' got None; $.RequestID None.
- CreateChannel, ivs-channel-error-paths#task#2: $.Code None; $.Message expected 'CreateChannel.name does not match the required pattern ^[a-zA-Z0-9-_]*$' got None.
- ImportPlaybackKeyPair, ivs-playback-key-pair-error-paths#task#1: $.Message expected 'Request member: publicKeyMaterial failed to satisfy the constraints: could not decode as a PEM file' got None.
- CreateStreamKey, ivs-stream-key-management#task#5: status 402 -> 402; $.Code expected 'ServiceQuotaExceededException' got None; $.Message expected 'Resource: stream-key quota exceeded limit: 1' got None.
- GetStream, ivs-stream-operations-not-broadcasting#task#2 (and #task#3), StopStream #task#4, PutMetadata #task#5: status 404 -> 404; $.Code expected 'ChannelNotBroadcasting' got None; $.Message expected 'Channel: arn:aws:ivs:us-west-2:000000000000:channel/3e29f61a804e is not currently online' got None — the ARN is the run's value, the fixed text is 'Channel: ' / ' is not currently online'.
- StartViewerSessionRevocation, ivs-viewer-session-revocation#task#3: $.Code expected 'ResourceNotFoundException' got None; $.Message expected 'Resource: arn:aws:ivs:us-west-2:000000000000:channel/does-not-exist not found' got None.
Recorded RequestID values and ARNs are per-run; the key set, the status, the code and the fixed message text are the requirement.

## Steps to Reproduce
Boot the emulator from this checkout, region us-west-2:
1. POST /CreateAdConfiguration {"name":"x","mediaTailorPlaybackConfigurations":[{"playbackConfigurationArn":"not-an-arn"}]} -> 400, body with no Code/Message.
2. POST /CreateChannel {"name":"bad name!"} -> 400, body with no Code/Message.
3. POST /ImportPlaybackKeyPair {"publicKeyMaterial":"not a pem"} -> 400, same.
4. POST /CreateChannel, then POST /CreateStreamKey for that channel twice -> the second answers 402 with no Code/Message (fixture ivs-stream-key-management#task#5).
5. With that channel idle, POST /GetStream, /StopStream and /PutMetadata for its ARN -> 404 each, with no Code/Message.
6. POST /StartViewerSessionRevocation {"channelArn":"arn:aws:ivs:us-west-2:000000000000:channel/does-not-exist","viewerId":"v"} -> 404 with no Code/Message.

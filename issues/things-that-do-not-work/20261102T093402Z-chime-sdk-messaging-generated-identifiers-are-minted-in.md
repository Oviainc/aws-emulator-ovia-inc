Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-messaging-channel-ban-create-list-and-delete
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-messaging-channel-ban-create-list-and-delete chime-sdk-messaging-channel-create-describe-update-and-delete chime-sdk-messaging-channel-expiration-settings-above-maximum-rejected chime-sdk-messaging-channel-membership-create-list-and-delete chime-sdk-messaging-channel-message-send-update-and-redact chime-sdk-messaging-channel-moderator-promote-list-and-demote chime-sdk-messaging-list-channels-private-scope-without-admin-rejected
Issue-Shape-Id: com.amazonaws.chimesdkmessaging#ChimeMessagingService
Issue-Key: chime-sdk-messaging|CreateChannel|2xx->2xx|$.ChannelArn
Issue-Key: chime-sdk-messaging|SendChannelMessage|2xx->2xx|$.MessageId
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# chime-sdk-messaging: generated identifiers are minted in the wrong shape (channel id, MessageId)

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
This issue bundles 2 net-new root causes in chime-sdk-messaging. Both are the same fault: the ids this service mints for the resources it creates are laid out in a shape Amazon Chime SDK Messaging does not use, so every generated identifier the emulator puts on the wire is shaped differently from the real service's. A fixer must close BOTH causes before submitting.

Cause 1 of 2 — CreateChannel: the generated channel id in ChannelArn is a 64-character hex digest instead of a UUID (fixture chime-sdk-messaging-channel-ban-create-list-and-delete, step task#4).
AWS: Smithy model com.amazonaws.chimesdkmessaging#CreateChannel, input member ChannelId (ChannelId, len 1..64, pattern ^[A-Za-z0-9]([A-Za-z0-9\:\-\_\.\@]{0,62}[A-Za-z0-9])?$) documents the rule for an omitted id verbatim: "An ID for the channel being created. If you do not specify an ID, a UUID will be created for the channel." The channel's ARN (output member ChannelArn, ChimeArn) is the request's AppInstanceArn extended by /channel/<channelId>, so when the caller omits ChannelId the ARN's last segment is a UUID in the canonical hyphenated lowercase form (8-4-4-4-12 hex groups). The 201 status and the ARN's app-instance prefix are correct in the recording and in the emulator; only the shape of the generated last segment diverges.
Emulator: replays of all 7 CreateChannel steps in this cluster return 201 with $.ChannelArn whose /channel/ segment is a 64-character lowercase hex digest, e.g. arn:aws:chime:<region>:<account>:app-instance/<appInstanceId>/channel/bc9747a9e5e050c835e6fbfb36c2ddf759f5bbbfa905863b03da299a9d0d9c4c (the account, region, app-instance id and the digest itself are per-run values; the SHAPE — 64 undelimited hex characters where AWS has a hyphenated UUID — is the defect). The source states the choice deliberately: services/service-com-amazonaws-chimesdkmessaging-chimemessagingservice/src/state.rs mint_resource_id() returns hex::encode(id_digest(..)) and CreateChannel calls it for an omitted ChannelId.
Why: a client that round-trips the generated channel id through a UUID type, a uuid-typed database column, a UUID regex or a path/segment validator parses the real service's ARN and fails on the emulator's — the id is 64 chars where a UUID is 36 and carries no hyphen groups. Code under test that derives a channel id from an ARN, or asserts the documented "a UUID will be created" contract, passes against AWS and breaks against the emulator (and vice versa), which is exactly the class of bug an emulator must not introduce.
Fix: the channel-id minting in this service's state (mint_resource_id, as used by CreateChannel's generated-id path) must emit the canonical hyphenated lowercase UUID form for a generated ChannelId, the same rendering mint_message_id already uses for 16 digest bytes; CreateChannel's ARN derivation itself is correct and needs no change. A caller-supplied ChannelId must keep being used verbatim.
Traces: chime-sdk-messaging-channel-ban-create-list-and-delete#task#4 (representative); also chime-sdk-messaging-channel-create-describe-update-and-delete#task#3, chime-sdk-messaging-channel-expiration-settings-above-maximum-rejected#task#3, chime-sdk-messaging-channel-membership-create-list-and-delete#task#4, chime-sdk-messaging-channel-message-send-update-and-redact#task#3, chime-sdk-messaging-channel-moderator-promote-list-and-demote#task#4, chime-sdk-messaging-list-channels-private-scope-without-admin-rejected#task#3.

Cause 2 of 2 — SendChannelMessage: MessageId is minted as a hyphenated UUID, which is not the shape the recording shows (fixture chime-sdk-messaging-channel-message-send-update-and-redact, step task#4).
AWS: recorded behavior only — the Smithy model constrains MessageId (SendChannelMessageResponse.MessageId -> MessageId, len 1..128, pattern ^[-_a-zA-Z0-9]*$, doc "The ID string assigned to each message.") but states no format, and the recorded value is a generated id the fixture asserts by SHAPE rather than by literal value. The recording's MessageId does NOT have the canonical hyphenated UUID layout the emulator emits; it is an opaque id drawn from the service's generated-id alphabet without the 8-4-4-4-12 hyphen grouping. Status 201 and every other output member (ChannelArn, Status) agree with the recording. Hypothesis: the two id conventions in this service are swapped — the opaque undelimited id belongs to MessageId and the UUID belongs to the generated channel id of Cause 1 — which is why the only two shape divergences in this corpus are these two members.
Emulator: the SendChannelMessage step returns 201 with $.MessageId in hyphenated lowercase UUID form, e.g. 9bee7730-b9d6-4cb2-bd51-10a9ea1acf59 (the value is per-run; the hyphenated UUID SHAPE is the defect). Source: services/service-com-amazonaws-chimesdkmessaging-chimemessagingservice/src/state.rs mint_message_id() takes the first 16 bytes of the id digest and renders them through service_common::uuid_from_random_bytes.
Why: MessageId is the key a client uses for GetChannelMessage, UpdateChannelMessage, RedactChannelMessage and for its own message bookkeeping. A client that validates or stores the id under the format the real service returns, or that asserts the ids it gets are not UUIDs (e.g. keying a store whose id column rejects hyphens, or asserting id length), passes against AWS and fails against the emulator; conversely a test written against the emulator's UUIDs is silently wrong about the real service.
Fix: the message-id minting in this service's state (mint_message_id, consumed by SendChannelMessage's handler) must emit the id shape the recording shows — the same undelimited generated-id rendering the service's other ids use — rather than the UUID form; the handler's idempotency replay path must keep returning the id the first send minted.
Traces: chime-sdk-messaging-channel-message-send-update-and-redact#task#4.

No open issue covers either cause; the existing chime-family issues on file are for chime-sdk-identity, chime-sdk-meetings and chime-sdk-media-pipelines only.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200): 79 steps, 70 passed, 9 mismatched, 2 divergence clusters, both chime-sdk-messaging, score 88.6%.

Cause 1 (cluster divergence_key service=chime-sdk-messaging|CreateChannel|status=201->201|keys=$.ChannelArn, 7 affected steps):
  expected: status 201, $.ChannelArn whose /channel/ segment is a generated UUID (shape-asserted, value not asserted); Smithy ChannelId doc: "If you do not specify an ID, a UUID will be created for the channel."
  actual:   status 201, $.ChannelArn = arn:aws:chime:us-east-1:000000000000:app-instance/b78b0769-c6a7-4029-b7d0-a221c0e40851/channel/bc9747a9e5e050c835e6fbfb36c2ddf759f5bbbfa905863b03da299a9d0d9c4c — a 64-character hex digest where a hyphenated UUID is required. (account/region/ids are per-run values.)
  replay detail: "$.ChannelArn: value_mismatch (expected '<shaped-not-asserted>', got 'arn:aws:chime:us-east-1:000000000000:app-instance/b78b0769-c6a7-4029-b7d0-a221c0e40851/channel/bc9747a9e5e050c835e6fbfb36c2ddf759f5bbbfa905863b03da299a9d0d9c4c')"

Cause 2 (cluster divergence_key service=chime-sdk-messaging|SendChannelMessage|status=201->201|keys=$.MessageId, 1 affected step):
  expected: status 201, $.MessageId an opaque generated id of the shape the recording carries (shape-asserted, value not asserted) — not a hyphenated UUID.
  actual:   status 201, $.MessageId = 9bee7730-b9d6-4cb2-bd51-10a9ea1acf59 — canonical hyphenated lowercase UUID. (value is per-run.)
  replay detail: "$.MessageId: value_mismatch (expected '<shaped-not-asserted>', got '9bee7730-b9d6-4cb2-bd51-10a9ea1acf59')"

Neither cluster carries a stub_path or an owned_by, so neither is already filed. Both HTTP statuses, error codes and the remaining response members match the recordings; only the generated-id shapes diverge.

## Steps to Reproduce
1. git clone the project and boot the emulator from the checkout.
2. Cause 1: replay fixture chime-sdk-messaging-channel-ban-create-list-and-delete, step task#4 (CreateChannel). Equivalently: POST /channels with a valid AppInstanceArn, Name, ClientRequestToken and x-amz-chime-bearer header and NO ChannelId, then inspect $.ChannelArn — the segment after /channel/ is 64 hex characters instead of a hyphenated UUID. The same step recurs in chime-sdk-messaging-channel-create-describe-update-and-delete#task#3, chime-sdk-messaging-channel-expiration-settings-above-maximum-rejected#task#3, chime-sdk-messaging-channel-membership-create-list-and-delete#task#4, chime-sdk-messaging-channel-message-send-update-and-redact#task#3, chime-sdk-messaging-channel-moderator-promote-list-and-demote#task#4 and chime-sdk-messaging-list-channels-private-scope-without-admin-rejected#task#3.
3. Cause 2: replay fixture chime-sdk-messaging-channel-message-send-update-and-redact, step task#4 (SendChannelMessage). Equivalently: POST /channels/{ChannelArn}/messages with Content, Type, Persistence, ClientRequestToken and x-amz-chime-bearer, then inspect $.MessageId — it is a hyphenated UUID instead of the opaque generated id shape the recording shows.
4. Re-run replay_wire_corpus after the fix: both clusters must clear (9 mismatched steps -> 0).

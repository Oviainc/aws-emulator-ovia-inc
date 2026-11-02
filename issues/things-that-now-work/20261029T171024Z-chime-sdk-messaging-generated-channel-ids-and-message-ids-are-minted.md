Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-messaging-channel-ban-create-list-and-delete
Issue-Step: task#4
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-messaging-channel-ban-create-list-and-delete chime-sdk-messaging-channel-create-describe-update-and-delete chime-sdk-messaging-channel-expiration-settings-above-maximum-rejected chime-sdk-messaging-channel-membership-create-list-and-delete chime-sdk-messaging-channel-message-send-update-and-redact chime-sdk-messaging-channel-moderator-promote-list-and-demote chime-sdk-messaging-list-channels-private-scope-without-admin-rejected
Issue-Shape-Id: com.amazonaws.chimesdkmessaging#ChimeMessagingService
Issue-Key: chime-sdk-messaging|CreateChannel|2xx->2xx|$.ChannelArn
Issue-Key: chime-sdk-messaging|SendChannelMessage|2xx->2xx|$.MessageId
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Generated channel ids and message ids are minted in a shape AWS never serves

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
TWO causes, one shared fault: both generated identifiers this service mints come from one helper that emits a 32-character hex string, which is not the shape class AWS's recorded identifiers have. Only the SHAPE is at issue here - the literal id value is environment-specific and the fixtures assert its shape, not its value.

Shared emulator basis: `state.rs:280-295` `mint_id(salt)` hex-encodes the first 16 bytes of a SHA-256 and is documented as "32 characters that satisfy both ChannelId's and MessageId's pattern". It is the single source of both ids, so one change fixes both causes.

Cause 1 of 2: CreateChannel's ChannelArn carries a channel segment of the wrong shape.
  AWS: recorded behavior only - Smithy constrains `ChannelArn` merely as `ChimeArn` (len 5..1600, pattern `^arn:[a-z0-9-.]{1,63}:...`), so the model says nothing about the generated segment; the recording asserts the shape class of the value AWS returned (`arn:<partition>:chime:<region>:<account>:app-instance/<uuid>/channel/<generated id>`) and the emulator's value does not satisfy it. Hypothesis: AWS mints the channel segment as a 64-character lowercase hex digest, twice the length the emulator emits.
  Emulator: returns `.../app-instance/<uuid>/channel/<32 lowercase hex chars>` - inside the modelled pattern and length, but a different id shape from the recorded one.
  Why: a client that validates, round-trips or re-derives a ChannelArn against AWS's id shape (segment length and alphabet) rejects an ARN this emulator mints, even though the ARN is otherwise well formed. This step is the CreateChannel of every fixture in the corpus, so it is the first response most flows see.
  Fix: the id-minting helper in `state.rs` (`mint_id`) - emit the full digest for a channel id rather than its first 16 bytes - and nothing in the CreateChannel handler itself.
  Traces: chime-sdk-messaging-channel-ban-create-list-and-delete#task#4 (same cluster in 6 further fixtures: channel-create-describe-update-and-delete#task#3, channel-expiration-settings-above-maximum-rejected#task#3, channel-membership-create-list-and-delete#task#4, channel-message-send-update-and-redact#task#3, channel-moderator-promote-list-and-demote#task#4, list-channels-private-scope-without-admin-rejected#task#3).

Cause 2 of 2: SendChannelMessage's MessageId is minted in the wrong shape.
  AWS: recorded behavior only - Smithy constrains `MessageId` as len 1..128, pattern `^[-_a-zA-Z0-9]*$`, which permits hyphens; the recording asserts the shape class of the value AWS returned and the emulator's does not satisfy it. Hypothesis: AWS mints a MessageId as a hyphenated UUID, which the emulator's unhyphenated 32-hex string resembles in alphabet but not in shape.
  Emulator: returns a 32-character lowercase hex MessageId from the same `mint_id` helper - modelled-pattern-valid but not AWS's shape.
  Why: a client that recognises or validates a message id by its AWS shape (e.g. parsing it as a UUID before storing or correlating it) fails on ids this emulator mints.
  Fix: the same id-minting helper in `state.rs` (`mint_id`), which is shared between channel ids and message ids; give the message-id caller AWS's shape.
  Traces: chime-sdk-messaging-channel-message-send-update-and-redact#task#4.

## Evidence
Expected vs actual, per replay (status matched at 201 for both; only the generated id's shape diverged, so the recordings assert the shape class rather than the literal value):
- CreateChannel [chime-sdk-messaging-channel-ban-create-list-and-delete#task#4, 7 fixtures affected]: `$.ChannelArn` value_mismatch against the asserted shape - got `arn:aws:chime:<region>:<account>:app-instance/<uuid>/channel/<32 hex chars>`.
- SendChannelMessage [chime-sdk-messaging-channel-message-send-update-and-redact#task#4]: `$.MessageId` value_mismatch against the asserted shape - got a 32-hex-character string with no hyphens.
Source: `state.rs:287-295` - `hex::encode(&digest[..16])`, documented as the id for "a channel id or a message id".

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay chime-sdk-messaging-channel-ban-create-list-and-delete step task#4 (CreateChannel) and inspect the returned ChannelArn: its channel segment is a 32-character hex string, which fails the fixture's shape assertion for the segment.
3. Replay chime-sdk-messaging-channel-message-send-update-and-redact step task#4 (SendChannelMessage) and inspect MessageId: a 32-character hex string with no hyphens, which fails the fixture's shape assertion.
4. By hand: CreateChannel then SendChannelMessage against the emulator and compare the id shapes with any recorded AWS response.

## Resolution
The id FORM of both generated identifiers was corrected and pinned by tests; the remaining
divergence - the literal id value - is DECLINED, because it is a per-run, service-generated
value this emulator must not derive from a recording.

One helper minted every id in this service (`state.rs` `mint_id`) by hex-encoding the first
16 of SHA-256's 32 digest bytes, so both id classes came out as the same 32-character hex
string. It is now two mints over one digest: `mint_resource_id` renders the FULL digest, the
64-character lowercase hex id the generated last segment of this service's
app-instance-scoped ARNs is (`.../channel/<id>` from `CreateChannel`, `.../channel-flow/<id>`
from `CreateChannelFlow`), which is also exactly `ChannelId`'s modelled maximum; and
`mint_message_id` lays the digest bytes out as the canonical hyphenated lowercase UUID a
`MessageId` is, through the `uuid` crate, which owns that form's version/variant nibbles and
its rendering. A `ChannelId` the caller supplies is still carried through unchanged.

Both replay keys (`CreateChannel|2xx->2xx|$.ChannelArn`,
`SendChannelMessage|2xx->2xx|$.MessageId`) still report as failing, byte-identically to
before the change: they compare the recorded id VALUE, which is per-account and per-run.
What the service owes instead is asserted end-to-end in `tests/all/generated_id_shapes.rs`:
every generated ARN id segment is a full lowercase hex digest, every `MessageId` a canonical
hyphenated uuid, each unique per resource over a table of request forms and a run of sends,
each the id the resource reads back by, and a caller-supplied `ChannelId` untouched.

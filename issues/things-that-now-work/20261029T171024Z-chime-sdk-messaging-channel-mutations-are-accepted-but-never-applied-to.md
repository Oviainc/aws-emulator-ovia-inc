Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-messaging-channel-create-describe-update-and-delete
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-messaging-channel-create-describe-update-and-delete chime-sdk-messaging-channel-expiration-settings-above-maximum-rejected
Issue-Shape-Id: com.amazonaws.chimesdkmessaging#ChimeMessagingService
Issue-Key: chime-sdk-messaging|DescribeChannel|2xx->2xx|$.Channel.Metadata
Issue-Key: chime-sdk-messaging|DescribeChannel|2xx->2xx|$.Channel.Mode
Issue-Key: chime-sdk-messaging|DescribeChannel|2xx->2xx|$.Channel.Name
Issue-Key: chime-sdk-messaging|DescribeChannel|2xx->2xx|$.Channel.ExpirationSettings
Issue-Related: 20261029T171024Z-chime-sdk-messaging-identity-name-is-never-served-for-channels-and.md
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Channel mutations are accepted but never applied to the stored channel

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateChannel

## Description
TWO causes, one shared fault: a write that mutates an existing channel answers success and returns the ChannelArn, but the stored channel is left exactly as created, so the next DescribeChannel reports the pre-write channel.

Cause 1 of 2: UpdateChannel does not apply Name, Mode or Metadata.
  AWS: Smithy - `UpdateChannel` (PUT /channels/{ChannelArn}, 200) takes `Name` (NonEmptyResourceName, @sensitive), `Mode` (ChannelMode = UNRESTRICTED|RESTRICTED) and `Metadata` (len 0..1024) and returns `ChannelArn`; the channel it updated is thereafter reported by `DescribeChannel` through `Channel.Name`, `Channel.Mode` and `Channel.Metadata`. In the recording the update is accepted and the following DescribeChannel reports the renamed channel, the newly requested mode and the metadata the update supplied - i.e. the request's own values, which is the requirement (the particular name/metadata strings are the fixture's).
  Emulator: the UpdateChannel step itself succeeds, but the following DescribeChannel still reports the channel as created - the creation-time Name, `Mode` RESTRICTED instead of the requested UNRESTRICTED, and no `Metadata` member at all. The update mutated nothing.
  Why: read-after-write is broken for the only operation that edits a channel. A client that renames a channel, switches it between RESTRICTED and UNRESTRICTED, or attaches metadata, reads back the old channel and sees its write silently discarded - and a moderation flow that depends on the mode change keeps the old access rules.
  Fix: the UpdateChannel handler must write the requested Name/Mode/Metadata onto the stored channel record (`ChannelRecord.name` / `.mode` / `.metadata`, `state.rs:401-416`) and advance its `last_updated_millis` before answering.
  Traces: chime-sdk-messaging-channel-create-describe-update-and-delete#task#6. NOTE: the same step also reports `$.Channel.CreatedBy.Name` missing, which is the separate Identity defect filed as "Identity.Name is never served for channels and channel messages" - the step needs BOTH fixes to pass.

Cause 2 of 2: a channel's ExpirationSettings are not reported by DescribeChannel.
  AWS: Smithy - `PutChannelExpirationSettings` (PUT /channels/{ChannelArn}/expiration-settings, 200) takes and echoes `ExpirationSettings` { `ExpirationDays` (required, 1..5475), `ExpirationCriterion` (required) }, and `Channel.ExpirationSettings` is documented "Settings that control when a channel expires". In the recording the accepted settings are reported by the following DescribeChannel; the requirement is that the settings in force on the channel are reported, with the criterion and day count taken from the accepted request.
  Emulator: the write is accepted, but DescribeChannel omits `Channel.ExpirationSettings` entirely - the settings are either not persisted or not projected.
  Why: a client cannot read back the retention policy it configured, so it cannot confirm or audit channel expiry; the fixture's companion case (a day count above the modelled maximum being rejected) passes, so the validation is right and only the persistence/projection is missing.
  Fix: persist `ExpirationSettings` on the stored channel record in the PutChannelExpirationSettings handler and project it in the record's `Channel` projection (`to_channel`), alongside Name/Mode/Metadata.
  Traces: chime-sdk-messaging-channel-expiration-settings-above-maximum-rejected#task#6 (this step also reports the `CreatedBy.Name` Identity defect).

## Evidence
Expected vs actual, per replay (HTTP 200 matched; the body reported pre-write state):
- DescribeChannel after UpdateChannel [chime-sdk-messaging-channel-create-describe-update-and-delete#task#6]: `$.Channel.Name` value_mismatch (expected the renamed channel, got the creation-time name); `$.Channel.Mode` value_mismatch (expected 'UNRESTRICTED', got 'RESTRICTED'); `$.Channel.Metadata` missing (expected the metadata the update supplied, got None).
- DescribeChannel after PutChannelExpirationSettings [chime-sdk-messaging-channel-expiration-settings-above-maximum-rejected#task#6]: `$.Channel.ExpirationSettings` missing (expected the accepted settings - criterion CREATED_TIMESTAMP with the requested day count - got None).
Stored shape that should have changed: `ChannelRecord` { name, mode, metadata, last_updated_millis } at `state.rs:401-416`.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay chime-sdk-messaging-channel-create-describe-update-and-delete: the UpdateChannel step succeeds, then step task#6 (DescribeChannel) still reports the original Name, Mode RESTRICTED and no Metadata.
3. Replay chime-sdk-messaging-channel-expiration-settings-above-maximum-rejected: the accepted PutChannelExpirationSettings succeeds, then step task#6 (DescribeChannel) reports no ExpirationSettings.
4. By hand: CreateChannel (Mode RESTRICTED) -> UpdateChannel (new Name, Mode UNRESTRICTED, Metadata) -> DescribeChannel; and CreateChannel -> PutChannelExpirationSettings -> DescribeChannel.

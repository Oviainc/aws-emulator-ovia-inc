Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-messaging-channel-create-describe-update-and-delete
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-messaging-channel-create-describe-update-and-delete chime-sdk-messaging-channel-message-send-update-and-redact
Issue-Shape-Id: com.amazonaws.chimesdkmessaging#ChimeMessagingService
Issue-Key: chime-sdk-messaging|DescribeChannel|2xx->2xx|$.Channel.CreatedBy.Name
Issue-Key: chime-sdk-messaging|GetChannelMessage|2xx->2xx|$.ChannelMessage.Sender.Name
Issue-Key: chime-sdk-messaging|ListChannelMessages|2xx->2xx|$.ChannelMessages[*].Sender.Name
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Identity.Name is never served for channels and channel messages

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeChannel

## Description
THREE causes, same root defect as the membership/ban/moderator bundle ("Identity.Name is never served for channel memberships, bans and moderators"), on the channel and channel-message projections. Filed separately only because one issue carries at most ten causes; a fixer must close BOTH, and the single change below closes all thirteen.

Shared AWS basis - Smithy: `Identity` = { `Arn`: ChimeArn, `Name`: ResourceName (len 0..256, @sensitive) }; `Channel.CreatedBy` is an `Identity` documented "The AppInstanceUser who created the channel", and `ChannelMessage.Sender`/`ChannelMessageSummary.Sender` likewise. AWS fills `Name` with the name the AppInstanceUser is registered under in Chime SDK Identity. The requirement is "Name is the stored name of the AppInstanceUser the Arn names"; the particular names in the recordings are values the fixture itself registered.

Shared emulator basis: `state.rs:844-854` `identity_of()` returns `name: None` for every Identity, on the stated premise that the name cannot be read from this crate - contradicted by the shared-engine directories this crate already reads (`state.rs:881-935`). `ChannelRecord.created_by` (`state.rs:413`) and the message record's `sender` (projected at `state.rs:643`) are therefore stored nameless at create time too.

Shared Why: a client rendering "who created this channel" or "who sent this message" has no name to show for any channel or any message this emulator serves, and the name a caller registered is unobservable through the messaging API.

Shared Fix: the `Identity` projection helper (`state.rs` `identity_of`) plus the create paths that snapshot an Identity into a stored record - resolve `Name` from the shared app-instance-user directory instead of hard-coding None.

Cause 1 of 3: DescribeChannel - `$.Channel.CreatedBy.Name` absent where AWS reports the creating user's registered name. Traces: chime-sdk-messaging-channel-create-describe-update-and-delete#task#4.

Cause 2 of 3: GetChannelMessage - `$.ChannelMessage.Sender.Name` absent, both before and after the message is edited/redacted. Traces: chime-sdk-messaging-channel-message-send-update-and-redact#task#6 (also #task#10).

Cause 3 of 3: ListChannelMessages - `$.ChannelMessages[*].Sender.Name` absent for every row. Traces: chime-sdk-messaging-channel-message-send-update-and-redact#task#8.

## Evidence
Expected vs actual, per replay (HTTP 200 matched in all three; only the body diverged):
- DescribeChannel: `$.Channel.CreatedBy.Name` missing (expected the creator's registered name, got None) [chime-sdk-messaging-channel-create-describe-update-and-delete#task#4]
- GetChannelMessage: `$.ChannelMessage.Sender.Name` missing (got None) [chime-sdk-messaging-channel-message-send-update-and-redact#task#6 and #task#10]
- ListChannelMessages: `$.ChannelMessages[0].Sender.Name` missing (got None) [chime-sdk-messaging-channel-message-send-update-and-redact#task#8]
Source: `state.rs:849-853` `identity_of` -> `name: None`; `ChannelRecord.created_by` `state.rs:413`; message `sender` projected at `state.rs:643`.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay chime-sdk-messaging-channel-create-describe-update-and-delete step task#4 (DescribeChannel): `Channel.CreatedBy` carries Arn only.
3. Replay chime-sdk-messaging-channel-message-send-update-and-redact steps task#6 / task#8 (GetChannelMessage, ListChannelMessages): every `Sender` carries Arn only.
4. Equivalent by hand: create a channel as a named AppInstanceUser, send a message, then DescribeChannel and GetChannelMessage - no name is reported for CreatedBy or Sender.

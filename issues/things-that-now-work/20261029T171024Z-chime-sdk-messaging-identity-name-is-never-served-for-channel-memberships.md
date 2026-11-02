Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-messaging-channel-ban-create-list-and-delete
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-messaging-channel-ban-create-list-and-delete chime-sdk-messaging-channel-membership-create-list-and-delete chime-sdk-messaging-channel-moderator-promote-list-and-demote
Issue-Shape-Id: com.amazonaws.chimesdkmessaging#ChimeMessagingService
Issue-Key: chime-sdk-messaging|CreateChannelMembership|2xx->2xx|$.Member.Name
Issue-Key: chime-sdk-messaging|CreateChannelBan|2xx->2xx|$.Member.Name
Issue-Key: chime-sdk-messaging|ListChannelBans|2xx->2xx|$.ChannelBans[*].Member.Name
Issue-Key: chime-sdk-messaging|DescribeChannelBan|2xx->2xx|$.ChannelBan.CreatedBy.Name
Issue-Key: chime-sdk-messaging|DescribeChannelBan|2xx->2xx|$.ChannelBan.Member.Name
Issue-Key: chime-sdk-messaging|ListChannelMemberships|2xx->2xx|$.ChannelMemberships[*].Member.Name
Issue-Key: chime-sdk-messaging|ListChannelMemberships|2xx->2xx|$.ChannelMemberships[*]
Issue-Key: chime-sdk-messaging|DescribeChannelMembership|2xx->2xx|$.ChannelMembership.InvitedBy.Name
Issue-Key: chime-sdk-messaging|DescribeChannelMembership|2xx->2xx|$.ChannelMembership.Member.Name
Issue-Key: chime-sdk-messaging|CreateChannelModerator|2xx->2xx|$.ChannelModerator.Name
Issue-Key: chime-sdk-messaging|ListChannelModerators|2xx->2xx|$.ChannelModerators[*]
Issue-Key: chime-sdk-messaging|DescribeChannelModerator|2xx->2xx|$.ChannelModerator.CreatedBy.Name
Issue-Key: chime-sdk-messaging|DescribeChannelModerator|2xx->2xx|$.ChannelModerator.Moderator.Name
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Identity.Name is never served for channel memberships, bans and moderators

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannelMembership

## Description
TEN causes, one root defect: every `Identity` this crate puts on the wire is built by one helper that hard-codes the `Name` member to absent, so no membership, ban or moderator projection ever reports the AppInstanceUser's name. Companion bundle (same root defect, channel/message projections): "Identity.Name is never served for channels and channel messages".

Shared AWS basis - Smithy: `Identity` = { `Arn`: ChimeArn (len 5..1600), `Name`: ResourceName (len 0..256, @sensitive) }, doc "The details of a user or bot." / "The name in an Identity.". AWS fills `Name` with the name the AppInstanceUser was registered under through Chime SDK Identity; the recordings show the two users the fixtures register (the generic owner and member of each run). The required behaviour is "Name is the stored name of the AppInstanceUser the Arn names" - the literal strings in the recordings are values from the fixture's own CreateAppInstanceUser calls, not part of the requirement.

Shared emulator basis: `state.rs:844-854` `identity_of(bearer_arn)` returns `Identity { arn: Some(..), name: None }`, documented as "an AppInstanceUser's Name is held by the Chime SDK Identity service, which this emulator cannot read from here ... inventing a name would be inventing state". That premise is false in this emulator: the identity service runs in the SAME engine and this crate already reads two of its/the family's shared directories (`app_instance_admins`, `chime_tags`, `state.rs:881-935`), so the name is state that IS reachable, not an invention. Every projection calls that one helper (e.g. `to_channel_membership` `state.rs:730`, `to_summary` `state.rs:742`).

Shared Why: a client that renders or matches a participant by name gets nothing for every member, ban, moderator, inviter and creator in this service - the member is modelled optional, so the SDK reports it simply as unset and the name a caller registered is unobservable through the messaging API.

Shared Fix: the `Identity` projection helper (`state.rs` `identity_of`) and the shared app-instance-user directory it must consult; no handler should be changed individually, because all ten causes flow from that single helper.

Cause 1 of 10: CreateChannelMembership - `$.Member.Name` absent where AWS reports the invited user's registered name. Traces: chime-sdk-messaging-channel-ban-create-list-and-delete#task#5 (also chime-sdk-messaging-channel-membership-create-list-and-delete#task#5, chime-sdk-messaging-channel-moderator-promote-list-and-demote#task#5).

Cause 2 of 10: CreateChannelBan - `$.Member.Name` absent. Traces: chime-sdk-messaging-channel-ban-create-list-and-delete#task#6.

Cause 3 of 10: ListChannelBans - `$.ChannelBans[*].Member.Name` absent for every row. Traces: chime-sdk-messaging-channel-ban-create-list-and-delete#task#7.

Cause 4 of 10: DescribeChannelBan - BOTH identities on the ban are nameless: `$.ChannelBan.Member.Name` and `$.ChannelBan.CreatedBy.Name`. Traces: chime-sdk-messaging-channel-ban-create-list-and-delete#task#8.

Cause 5 of 10: ListChannelMemberships - `$.ChannelMemberships[*].Member.Name` absent. Traces: chime-sdk-messaging-channel-ban-create-list-and-delete#task#9.

Cause 6 of 10: ListChannelMemberships, multi-row page - every `$.ChannelMemberships[i].Member` is served as Arn-only, so each row differs from AWS's row by its missing `Name`; the replay reports the rows both missing and extra because the whole element compares unequal. Traces: chime-sdk-messaging-channel-membership-create-list-and-delete#task#6.

Cause 7 of 10: DescribeChannelMembership - `$.ChannelMembership.Member.Name` and `$.ChannelMembership.InvitedBy.Name` absent (modelled `ChannelMembership.Member`/`InvitedBy`, both `Identity`). Traces: chime-sdk-messaging-channel-membership-create-list-and-delete#task#7.

Cause 8 of 10: CreateChannelModerator - `$.ChannelModerator.Name` absent (the response's `ChannelModerator` member is itself an `Identity`). Traces: chime-sdk-messaging-channel-moderator-promote-list-and-demote#task#6.

Cause 9 of 10: ListChannelModerators - every `$.ChannelModerators[i].Moderator` is Arn-only, so each row compares unequal to AWS's row. Traces: chime-sdk-messaging-channel-moderator-promote-list-and-demote#task#7.

Cause 10 of 10: DescribeChannelModerator - `$.ChannelModerator.Moderator.Name` and `$.ChannelModerator.CreatedBy.Name` absent. Traces: chime-sdk-messaging-channel-moderator-promote-list-and-demote#task#8.

## Evidence
Expected vs actual, per replay (status matched in all ten - 201 or 200 - only the body diverged):
- CreateChannelMembership: `$.Member.Name` missing (expected the member's registered name, got None) [chime-sdk-messaging-channel-ban-create-list-and-delete#task#5, +2 affected]
- CreateChannelBan: `$.Member.Name` missing (got None) [#task#6]
- ListChannelBans: `$.ChannelBans[0].Member.Name` missing (got None) [#task#7]
- DescribeChannelBan: `$.ChannelBan.CreatedBy.Name` missing, `$.ChannelBan.Member.Name` missing [#task#8]
- ListChannelMemberships: `$.ChannelMemberships[0].Member.Name` missing [#task#9]
- ListChannelMemberships (2 rows): `$.ChannelMemberships[0]` / `[1]` reported missing AND extra - expected `{Member:{Arn, Name}, ReadMarkerTimestamp:null}`, got `{Member:{Arn}}` [chime-sdk-messaging-channel-membership-create-list-and-delete#task#6]
- DescribeChannelMembership: `$.ChannelMembership.InvitedBy.Name` and `$.ChannelMembership.Member.Name` missing [#task#7]
- CreateChannelModerator: `$.ChannelModerator.Name` missing [chime-sdk-messaging-channel-moderator-promote-list-and-demote#task#6]
- ListChannelModerators (2 rows): expected `{Moderator:{Arn, Name}}`, got `{Moderator:{Arn}}` [#task#7]
- DescribeChannelModerator: `$.ChannelModerator.CreatedBy.Name` and `$.ChannelModerator.Moderator.Name` missing [#task#8]
Source: `state.rs:849-853` `identity_of` -> `name: None` unconditionally.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture chime-sdk-messaging-channel-ban-create-list-and-delete and compare step task#5 (CreateChannelMembership): `Member.Name` is absent from the response.
3. Replay steps task#6..task#9 of the same fixture, chime-sdk-messaging-channel-membership-create-list-and-delete task#6/task#7, and chime-sdk-messaging-channel-moderator-promote-list-and-demote task#6..task#8: every Identity in those bodies (Member, CreatedBy, InvitedBy, Moderator, ChannelModerator) is served with Arn only.
4. Equivalent by hand: register an AppInstanceUser with a name through chime-sdk-identity, add it to a channel, then DescribeChannelMembership - Member.Name is never reported.

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# UpdateChannel always returns AttachedMultiviewChannels as absent instead of the channel's current attachments

_Filed: 2026-10-24T05:11:59Z_
_Operation: UpdateChannel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AttachedMultiviewChannels` is part of a channel's representation: it is the inverse of the group's multiview source lists — the MULTIVIEW channels in the same group that name this channel as a source. `CreateChannelResponse`, `GetChannelResponse`, `UpdateChannelResponse` and `ChannelListConfiguration` all declare the member, and the emulator derives it correctly in three of those four places via `crate::state::attached_multiview_channels(group, &channel.name)`.

`UpdateChannel` hard-codes `attached_multiview_channels: None` with the comment "Updating a channel attaches no multiview source channels to it" — but the member reports attachments that ALREADY exist, not ones the request creates. So a channel that is a source of a multiview channel reports its attachment list on `GetChannel` and in `ListChannels`, and then reports nothing on `UpdateChannel`, even though the update changed nothing about those attachments. A client that refreshes its view of a channel from the update response loses the field.

## Evidence

Spec: Smithy `UpdateChannelResponse$AttachedMultiviewChannels` is declared (same member as on `CreateChannelResponse`/`GetChannelResponse`/`ChannelListConfiguration`), documented as the multiview channels attached to this channel — a derived, read-only property of the channel.

Handler: `src/ops/update_channel.rs:348-349` — `// Updating a channel attaches no multiview source channels to it` / `attached_multiview_channels: None`. Compare `src/ops/get_channel.rs:228-231`, `src/ops/create_channel.rs:276-279` and `src/ops/list_channels.rs:130-133`, which all call `crate::state::attached_multiview_channels(...)`.

Tests: the only assertion on this member is `tests/all/channel_listing.rs:405` (ListChannels). No test asserts the member on an `UpdateChannel` response, so nothing covers the current behavior.

## Steps to Reproduce

1. Create channel group `tg` and two CMAF channels `source-a`, `source-b`.
2. Create channel `mv` with `InputType: "MULTIVIEW"` and a `MultiviewConfiguration` naming `source-a` and `source-b` as sources.
3. GET `/channelGroup/tg/channel/source-a/`. Observe `AttachedMultiviewChannels: ["mv"]`.
4. PUT `/channelGroup/tg/channel/source-a/` with `{"Description": "updated"}`. Expected: the response still reports `AttachedMultiviewChannels: ["mv"]`. Actual: the member is absent.
5. GET `/channelGroup/tg/channel/source-a/` again — the attachment is still reported, confirming the update response dropped a field the resource still has.

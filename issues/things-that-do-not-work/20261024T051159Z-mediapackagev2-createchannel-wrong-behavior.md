Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# CreateChannel does not materialise the documented HLS default for InputType, so channel responses omit InputType entirely

_Filed: 2026-10-24T05:11:59Z_
_Operation: CreateChannel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`InputType` is documented as a server-applied default: "The input type is an immutable field… If unprovided, the value defaults to HLS." Real AWS therefore stores HLS on a channel created without the member and reports `"InputType": "HLS"` in the `CreateChannel` response and in every later `GetChannel`, `UpdateChannel` and `ListChannels` projection — the field is always present on a channel.

The emulator stores the raw optional (`input_type: input.input_type`) and reports `channel.input_type.clone()`, so a channel created without `InputType` is persisted with NO input type and every response for it OMITS the `InputType` key. The crate even defines the right constant (`crate::state::DEFAULT_INPUT_TYPE = "HLS"`) but uses it only for the cross-member `InputType` restriction check (`create_channel.rs:226-231`, `update_channel.rs:311-314`), never for storage — in contradiction with the module's own contract in `src/state.rs` that server-applied members "are materialised ONCE, at create time, from the submitted value or the service's default, and stored" (which IS done for `InputSwitchConfiguration`, `OutputHeaderConfiguration`, `Segment`, `StartoverWindowSeconds`, `UriSeparator`, `StreamNameOutputMode`).

A client that reads `InputType` to decide how to ingest (CMAF vs HLS) gets a missing field instead of `HLS`.

## Evidence

Spec: Smithy `CreateChannelRequest$InputType` doc — "If unprovided, the value defaults to HLS" (confirmed via smithy_query doc). `CreateChannelResponse$InputType`, `GetChannelResponse$InputType`, `UpdateChannelResponse$InputType` and `ChannelListConfiguration$InputType` all declare the member.

Handler: `src/ops/create_channel.rs:444` stores `input_type: input.input_type` (no materialisation); `channel_output` at line 287 emits `channel.input_type.clone()`; `src/ops/get_channel.rs:240`, `src/ops/update_channel.rs:357` and `src/ops/list_channels.rs:126` all propagate the same `Option`. Compare `create_channel.rs:432-437` where the other server-applied blocks ARE materialised via `crate::state::materialized_*`.

Tests: every channel-creating test passes an explicit `InputType` (grep `InputType` in `tests/`), and the only no-InputType case (`tests/all/control_plane_validation.rs:285-290`, "OutputLockingMode with no InputType, which defaults to HLS") asserts a 400 refusal, so no test asserts what a successful create without `InputType` reports.

## Steps to Reproduce

1. Create a channel group `tg`.
2. POST `/channelGroup/tg/channel` with body `{"ChannelName": "tc"}` (no `InputType`).
3. Inspect the 200 response. Expected: `"InputType": "HLS"`. Actual: no `InputType` key.
4. GET `/channelGroup/tg/channel/tc/` and GET `/channelGroup/tg/channel?maxResults=10`. Expected: `InputType: "HLS"` on the channel and on its list item. Actual: absent in both.
5. PUT `/channelGroup/tg/channel/tc/` with `{}`. Expected: the update response still reports `InputType: "HLS"` (the field is immutable). Actual: absent.

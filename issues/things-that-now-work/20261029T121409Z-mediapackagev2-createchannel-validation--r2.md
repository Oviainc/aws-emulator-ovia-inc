Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 442a99274869dbb92025fb75ce375f34a4aa7eac

# A MULTIVIEW channel is accepted with an OutputLockingMode (and defaulted to EPOCH_LOCKED), although the model says the setting is only valid when InputType is CMAF

_Filed: 2026-10-29T12:14:09Z_
_Operation: CreateChannel | Class: validation_

> _Note: this issue was identified against the code base at commit `442a99274869dbb92025fb75ce375f34a4aa7eac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate's one table of per-InputType channel restrictions lists MULTIVIEW as an input type that may carry `OutputLockingMode`, so `CreateChannel` (a) accepts a submitted `OutputLockingMode` — including `NON_EPOCH_LOCKED` — on a MULTIVIEW channel and persists it, and (b) materialises the documented `EPOCH_LOCKED` default onto every MULTIVIEW channel, which `CreateChannel`/`GetChannel`/`ListChannels`/`UpdateChannel` then all report.

The model states the restriction in one direction only: `CreateChannelRequest$OutputLockingMode` — the single place the member is an input — says the setting "is only valid when InputType is CMAF", and the model's `ValidationExceptionType` enum names `ONLY_CMAF_INPUT_TYPE_ALLOW_OUTPUT_LOCKING_MODE` for exactly that refusal. The handler already answers that refusal for an HLS channel (and for the absent-InputType case, which defaults to HLS); it is only MULTIVIEW that is wrongly admitted.

The comment that justifies the MULTIVIEW row quotes the model as saying "only configurable when the channel InputType is CMAF or MULTIVIEW". No member of this model carries that sentence: `OutputLockingMode` has no shape-level doc at all, and the only member doc that states a restriction is the `CreateChannelRequest` one quoted above (the `CreateChannelResponse` / `GetChannelResponse` / `ChannelListConfiguration` copies say only "The output locking mode configured for the channel." plus the allowed-values list). The row's own `exception_type` literal, `ONLY_CMAF_INPUT_TYPE_ALLOW_OUTPUT_LOCKING_MODE`, names CMAF alone.

There is a second-order effect inside the crate: `channel_carries_output_timestamp_mode` decides from the channel's stored locking mode, so a MULTIVIEW channel accepted as `NON_EPOCH_LOCKED` makes its origin endpoints eligible for `Segment.OutputTimestampMode` — the member is then materialised onto, and accepted on, endpoints of a channel that the model says may not carry a locking mode in the first place.

## Evidence

Smithy (`smithy_query(query="doc", shape="CreateChannelRequest", member="OutputLockingMode")`, services/service-com-amazonaws-mediapackagev2-mediapackagev2/smithy-model/com-amazonaws-mediapackagev2-mediapackagev2.json):
```
The output locking mode for the channel. This setting is only valid when InputType is CMAF. This value is immutable after channel creation. If you don't specify a value, the default is EPOCH_LOCKED.
```
and `smithy_query(query="doc", shape="OutputLockingMode")` -> `{"documentation": null}` (no shape-level doc), while `ValidationExceptionType` enumerates `ONLY_CMAF_INPUT_TYPE_ALLOW_OUTPUT_LOCKING_MODE`.

The table that contradicts it — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/state.rs:3011-3021:
```
/// The rows are read off the model, not guessed: each member's own doc says which input
/// types may carry it (`OutputLockingMode` — "only configurable when the channel
/// InputType is CMAF or MULTIVIEW"; ...
const CHANNEL_INPUT_TYPE_RULES: &[ChannelInputTypeRule] = &[
    ChannelInputTypeRule {
        member: "outputLockingMode",
        allowed: &["CMAF", "MULTIVIEW"],
        exception_type: "ONLY_CMAF_INPUT_TYPE_ALLOW_OUTPUT_LOCKING_MODE",
```
(the quoted sentence "CMAF or MULTIVIEW" appears nowhere in the model; the discriminator on the next line names CMAF only).

The materialisation that follows the same table — src/state.rs:1444-1466:
```
/// ... so the default is materialised only on a channel whose input type may carry it; a
/// channel that may not has no mode and reports none. The restriction is read off the
/// same table a SUBMITTED mode is refused by ([`channel_input_type_violation`]) ...
pub fn materialized_output_locking_mode(
```
called by src/ops/create_channel.rs:486-487 (`crate::state::materialized_output_locking_mode(input.output_locking_mode, ...)`) and reported at src/ops/create_channel.rs:511 / src/ops/get_channel.rs:246 / src/ops/list_channels.rs:127.

The knock-on reader — src/state.rs:1472-1480: "`Segment$OutputTimestampMode` is \"only configurable on channels with `OutputLockingMode` set to `NON_EPOCH_LOCKED`\" ... `pub fn channel_carries_output_timestamp_mode(channel_output_locking_mode: Option<&str>)`".

Test coverage pins the wrong side, with no model sentence given as the reason:
- tests/all/control_plane_validation.rs:297-312 — `label: "OutputLockingMode on MULTIVIEW input"`, body `{"OutputLockingMode": "NON_EPOCH_LOCKED", "MultiviewConfiguration": {...}}`, `refused_as: None` (the inline comment there only explains why a MultiviewConfiguration is included).
- tests/all/server_materialised_members.rs:186-189 — a MULTIVIEW channel created without the member expects `("/OutputLockingMode", json!("EPOCH_LOCKED"))`; :191-207 — `"a multiview channel naming the non-default locking mode"` expects `("/OutputLockingMode", json!("NON_EPOCH_LOCKED"))`.
- src/state.rs:4356-4361 — the unit case `("output locking mode on MULTIVIEW", "MULTIVIEW", |present| present.output_locking_mode = true, None)`.
The neighbouring HLS rows (tests/all/control_plane_validation.rs:273-290) already expect `ONLY_CMAF_INPUT_TYPE_ALLOW_OUTPUT_LOCKING_MODE`, so only the MULTIVIEW rows change.

## Steps to Reproduce

1. Boot the emulator. POST /channelGroup with {"ChannelGroupName":"g"}.
2. POST /channelGroup/g/channel with {"ChannelName":"source-a","InputType":"CMAF"} (a legal multiview source).
3. POST /channelGroup/g/channel with {"ChannelName":"mv","InputType":"MULTIVIEW","OutputLockingMode":"NON_EPOCH_LOCKED","MultiviewConfiguration":{"AvailableSources":["source-a"],"AvailableLayouts":["LAYOUT_2EH"]}}. Observe 200 with "OutputLockingMode":"NON_EPOCH_LOCKED"; the model says the setting is only valid when InputType is CMAF, i.e. 400 ValidationException / ONLY_CMAF_INPUT_TYPE_ALLOW_OUTPUT_LOCKING_MODE.
4. Repeat step 3 without OutputLockingMode. Observe 200 reporting "OutputLockingMode":"EPOCH_LOCKED" on a channel the model says may not carry the member at all; GET /channelGroup/g/channel/mv and GET /channelGroup/g/channel report the same value.
5. Compare with POST /channelGroup/g/channel with {"ChannelName":"h","InputType":"HLS","OutputLockingMode":"EPOCH_LOCKED"}: 400 ValidationException / ONLY_CMAF_INPUT_TYPE_ALLOW_OUTPUT_LOCKING_MODE — the same member, the same table, refused on one non-CMAF input type and accepted on the other.
6. Then POST /channelGroup/g/channel/mv/originEndpoint with a ContainerType, one manifest and {"Segment":{"OutputTimestampMode":"REBASED_TO_CHANNEL_START"}}: it is accepted because the MULTIVIEW channel was allowed to be NON_EPOCH_LOCKED in step 3.

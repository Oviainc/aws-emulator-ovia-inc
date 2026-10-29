Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6e8c3e9c5f69577c8704496d0849780bb1468b0d

# UpdateOriginEndpoint sets Segment.OutputTimestampMode after creation, because the create never materialises the documented PASSTHROUGH default

_Filed: 2026-10-28T21:00:50Z_
_Operation: UpdateOriginEndpoint | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6e8c3e9c5f69577c8704496d0849780bb1468b0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Segment$OutputTimestampMode` is documented twice over in the model: it has a service-applied default ("If you don't specify a value, the default is PASSTHROUGH") AND it is "immutable after endpoint creation". The crate honours the immutability only when the endpoint already carries a value, and it never materialises the default — so an endpoint created without the member is stored with NO output-timestamp mode, and the FIRST UpdateOriginEndpoint that names one is accepted and stores it. A member the model calls immutable after creation is therefore mutable for one write, and `GetOriginEndpoint` reports a mode the endpoint was never created with.

Root cause is one line missing in `crate::state::materialized_segment` (reached first from `CreateOriginEndpoint`): the helper materialises five segment members from the service's own defaults and deliberately passes `OutputTimestampMode` through untouched, so the update's immutability guard — which compares only when BOTH the request and the store carry a value — has nothing to compare against. The crate applies the opposite (correct) treatment to the two sibling members whose defaults are documented in exactly the same sentence form: `StreamNameOutputMode` is materialised to `INDEX` at create, which is precisely why its own immutability check in this same handler can never be bypassed, and `OutputLockingMode`'s helper quotes the "If you don't specify a value, the default is …" sentence as the reason it materialises.

Fixing the create side closes the update hole without touching the guard: with every endpoint holding a concrete mode, an update naming a different one is the `OUTPUT_TIMESTAMP_MODE_IMMUTABLE` refusal the handler already builds. Note that `tests/all/update_roundtrip.rs:761-790` currently pins the hole and has to move with the fix: its stated reason ("A member with no stored value is not a member an update changes") is the premise the model sentence denies — per the model there is no "no stored value" state for this member.

## Evidence

Smithy — `Segment$OutputTimestampMode` (smithy_query(query="doc", shape="Segment", member="OutputTimestampMode")):
```
The output timestamp mode for the origin endpoint's segments. This setting is only configurable on channels with OutputLockingMode set to NON_EPOCH_LOCKED. This value is immutable after endpoint creation. If you don't specify a value, the default is PASSTHROUGH.
```

The handler's own doc claims the rule — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/ops/update_origin_endpoint.rs:13-17:
```
//! * a member the model documents as immutable after creation, when the request carries
//!   a different value — `ContainerType` (`CONTAINER_TYPE_IMMUTABLE`),
//!   `StreamNameOutputMode` (`STREAM_NAME_OUTPUT_MODE_IMMUTABLE`) and
//!   `Segment.OutputTimestampMode` (`OUTPUT_TIMESTAMP_MODE_IMMUTABLE`), each a 400
//!   `ValidationException` naming the member, with the stored endpoint untouched;
```

The code under it only compares when the endpoint already has a value — src/ops/update_origin_endpoint.rs:438-451:
```
        if let (Some(requested), Some(stored)) = (
            input
                .segment
                .as_ref()
                .and_then(|segment| segment.output_timestamp_mode.as_ref()),
            endpoint.segment.output_timestamp_mode.as_ref(),
        ) {
            if requested != stored {
                return Err(UpdateOriginEndpointErrorModeled::ValidationException(
                    immutable_violation(&OUTPUT_TIMESTAMP_MODE),
                )
                .into());
            }
        }
```
and then stores the submitted mode — src/ops/update_origin_endpoint.rs:510-514:
```
        let mut segment = crate::state::materialized_segment(input.segment);
        segment.output_timestamp_mode = segment
            .output_timestamp_mode
            .or(endpoint.segment.output_timestamp_mode.clone());
        endpoint.segment = segment;
```

Why there is nothing to compare against — src/state.rs:1336-1340 (`materialized_segment`, called by CreateOriginEndpoint at src/ops/create_origin_endpoint.rs:490):
```
/// reports for an endpoint created without a segment configuration. `Scte`, `Encryption`
/// and `OutputTimestampMode` are reported only when they were configured, so they pass
/// through untouched.
pub fn materialized_segment(submitted: Option<Segment>) -> Segment {
    let mut segment = submitted.unwrap_or_default();
```
(the body materialises `segment_duration_seconds`, `segment_name`, `ts_use_audio_rendition_group`, `include_iframe_only_streams` and `ts_include_dvb_subtitles`, src/state.rs:1341-1365, and never touches `output_timestamp_mode`).

The criterion the crate states for exactly this situation one member away — src/state.rs:1302-1311 (`materialized_output_locking_mode`):
```
/// The model documents a service-applied default for the member ("If you don't specify a
/// value, the default is `EPOCH_LOCKED`") AND restricts which input types may carry it at
/// all, so the default is materialised only on a channel whose input type may carry it
```
and the sibling that makes this handler's OTHER immutability check unbypassable — src/state.rs:1382-1386:
```
/// The stream-name output mode an origin endpoint is stored with — the submitted one,
/// else [`DEFAULT_STREAM_NAME_OUTPUT_MODE`].
pub fn materialized_stream_name_output_mode(submitted: Option<String>) -> String {
    submitted.unwrap_or_else(|| DEFAULT_STREAM_NAME_OUTPUT_MODE.to_string())
}
```

Test coverage — tests/all/update_roundtrip.rs:684-686 states the premise the model denies, and 761-790 pins the hole:
```
/// A member
/// with no stored value is not a member an update changes, so the first update that names
/// one sets it — the same shape as `StreamNameOutputMode`.
```
```
    // An endpoint created without the member has nothing to change, so the first update
    // naming one sets it.
...
    assert_eq!(
        at(&read, &["Segment", "OutputTimestampMode"]),
        json!("REBASED_TO_CHANNEL_START"),
        "case \"the create omitted the mode\": the submitted mode must be stored, got {read}"
    );
```
The refusal half (tests/all/update_roundtrip.rs:687-759) only exercises endpoints created WITH a mode, so nothing covers the documented default.

## Steps to Reproduce

1. Boot the emulator. POST /channelGroup with {"ChannelGroupName":"g"}; POST /channelGroup/g/channel with {"ChannelName":"c"}.
2. POST /channelGroup/g/channel/c/originEndpoint with {"OriginEndpointName":"e","ContainerType":"TS","HlsManifests":[{"ManifestName":"index"}]} — no Segment.OutputTimestampMode.
3. Observe the 200 body: Segment reports SegmentDurationSeconds 6, SegmentName "segment" and the three TS toggles (all server-applied), but NO OutputTimestampMode — although the model says "If you don't specify a value, the default is PASSTHROUGH".
4. PUT /channelGroup/g/channel/c/originEndpoint/e with {"ContainerType":"TS","HlsManifests":[{"ManifestName":"index"}],"Segment":{"OutputTimestampMode":"REBASED_TO_CHANNEL_START"}}. Observe 200.
5. GET /channelGroup/g/channel/c/originEndpoint/e: Segment.OutputTimestampMode is now REBASED_TO_CHANNEL_START — a member the model calls "immutable after endpoint creation" has been changed after creation, where the same request against an endpoint created with PASSTHROUGH is refused 400 / OUTPUT_TIMESTAMP_MODE_IMMUTABLE.
6. Contrast with StreamNameOutputMode: create an endpoint without it, then PUT with {"StreamNameOutputMode":"PASSTHROUGH_NAME"} — refused 400 / STREAM_NAME_OUTPUT_MODE_IMMUTABLE, because that member's documented default IS materialised at create.

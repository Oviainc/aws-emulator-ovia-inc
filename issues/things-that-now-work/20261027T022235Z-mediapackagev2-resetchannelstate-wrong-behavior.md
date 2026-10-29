Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3938003190abafdb7ac5d97f1c3372e4bb8e6b5a

# ResetChannelState records the reset instant but never removes the channel's previously ingested content

_Filed: 2026-10-27T02:22:35Z_
_Operation: ResetChannelState | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3938003190abafdb7ac5d97f1c3372e4bb8e6b5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model states what a channel reset DOES: it "refreshes the ingest stream and removes previous content". This crate really does hold a channel's previously ingested content — `Channel::ingested_objects`, written by the ingest data plane and bounded per channel and store-wide — but the `ResetChannelState` handler writes only `channel.reset_at` and leaves `ingested_objects` exactly as it was. Nothing else ever removes those objects short of deleting the whole channel (`DeleteChannel` drops the record), so:

* the content the reset promised to remove is still retained after a 200, and
* it still counts against the per-channel object ceiling and the store-wide retained-bytes ceiling, so a channel that has been pushed `MAX_INGESTED_OBJECTS_PER_CHANNEL` objects keeps answering `507 InsufficientStorage` to every later push even after the caller reset it — the one operation the model says removes that content cannot free it.

The sibling reset handler shows the crate already reasons about this axis: `ResetOriginEndpointState` documents that it persists only the reset instant *because* "the endpoint holds no media in this emulator". The channel DOES hold media, so the same shortcut is not available to `ResetChannelState`.

## Evidence

Smithy model (`smithy_query query=doc shape=com.amazonaws.mediapackagev2#ResetChannelState`):
"Resetting the channel can help to clear errors from misconfigurations in the encoder. A reset refreshes the ingest stream and removes previous content."

The handler — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/ops/reset_channel_state.rs:224-228:
```
        // The reset instant: the moment this call refreshed the channel's ingest stream.
        // It is persisted, and OVERWRITTEN by each later reset, because the field records
        // the LAST reset; the response reports exactly the value that was stored.
        let reset_at = crate::state::now();
        channel.reset_at = Some(reset_at);
```
(no other write; `ingested_objects` is not touched anywhere in this file.)

The content it should have removed — src/state.rs:855-861:
```
    /// What the channel's ingest data plane has accepted, keyed by the request path the
    /// contributing encoder `PUT` it to. Bounded by
    /// [`MAX_INGESTED_OBJECTS_PER_CHANNEL`] / [`MAX_INGESTED_OBJECT_BYTES`].
...
    pub ingested_objects: BTreeMap<String, Vec<u8>>,
```
written by src/state.rs:753 `channel.ingested_objects.insert(target.object.clone(), body);` (reached from src/service.rs:663 `store.record_ingested_object(&region, &target, body.to_vec())`), and the ceiling it keeps occupying — src/service.rs:672-679:
```
        crate::state::IngestOutcome::ObjectCountReached => {
            return error_response(
                507,
                "InsufficientStorage",
                "This channel already holds as many ingested objects as this emulator \
                 retains",
```

The sibling that justifies its own narrower write — src/ops/reset_origin_endpoint_state.rs:221-226:
```
        // The reset itself. The endpoint holds no media in this emulator, so what a reset
        // persists is the instant its content was discarded; `ModifiedAt` and the ETag
        // cover the endpoint's CONFIGURATION, which a reset does not change, so both stay
        // as they were.
        let reset_at = crate::state::now();
        endpoint.reset_at = Some(reset_at);
```

Test coverage: tests/all/channel_reset.rs:9-18 asserts only the reset instant, its last-write-wins behaviour, that no configuration member changes, and the 404 path — no test pushes an object to the ingest data plane and then resets, so no existing test pins the current retention.

## Steps to Reproduce

1. CreateChannelGroup `g`, then CreateChannel `g`/`c` (CMAF or HLS input type), and read the channel's ingest URL from `IngestEndpoints[0].Url`.
2. PUT objects to that ingest URL with SigV4 credentials until the push answers `507 InsufficientStorage` ("This channel already holds as many ingested objects as this emulator retains") — the channel is now at `MAX_INGESTED_OBJECTS_PER_CHANNEL`.
3. Call ResetChannelState on `g`/`c`: it answers 200 with a `ResetAt`.
4. PUT one more object to the same ingest URL with a NEW object path. Observe `507` again: the reset removed no previous content, even though the model says "A reset refreshes the ingest stream and removes previous content". Only deleting the channel frees it.

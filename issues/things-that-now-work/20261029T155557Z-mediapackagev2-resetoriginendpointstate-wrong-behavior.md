Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: cb9254e7b4a839ef1fe220636b4ce55207805929

# ResetOriginEndpointState discards no content, although its own handler doc says the endpoint's stored content is discarded

_Filed: 2026-10-29T15:55:57Z_
_Operation: ResetOriginEndpointState | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `cb9254e7b4a839ef1fe220636b4ce55207805929`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler's module doc states, as the summary of what the operation does, that an endpoint reset DISCARDS the endpoint's stored content and records the instant. The handle body does neither half of that: the only write it makes is `endpoint.reset_at = Some(reset_at)`, and its own inline comment admits the gap in the opposite direction ("The endpoint holds no media in this emulator"). So the file carries two contradictory statements about the same handler — the header promises a discard, the body says there is nothing to discard and performs none — and the operation the model describes as the one that "clears out all previous content" is implemented as a pure timestamp write.

The crate is not contentless: a channel record carries `ingested_objects`, which the ingest data plane fills on every accepted push (`record_ingested_object`), and those objects persist across a reset of any endpoint beneath that channel. Nothing in the crate removes stored content on either reset path, so the header's claim cannot be verified by any caller and misdescribes the handler it documents. (The channel-level half of this — ResetChannelState leaving `ingested_objects` in place — is already filed as a separate open issue against that operation; this one is the ResetOriginEndpointState handler's self-contradicting documentation and the write it never makes.)

## Evidence

Smithy, com.amazonaws.mediapackagev2#ResetOriginEndpointState documentation: "Resetting the origin endpoint can help to resolve unexpected behavior and other content packaging issues. It also helps to preserve special events when you don't want the previous content to be available for viewing. A reset clears out all previous content from the origin endpoint."

Handler doc, services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/ops/reset_origin_endpoint_state.rs:5-10: "//! Resets one origin endpoint: the endpoint's stored content is discarded and the moment\n//! of the reset is recorded on the endpoint as `ResetAt` ... a reset clears content, it does not reconfigure\n//! the resource."

The code under that doc, same file:221-226: "// The reset itself. The endpoint holds no media in this emulator, so what a reset\n// persists is the instant its content was discarded; `ModifiedAt` and the ETag\n// cover the endpoint's CONFIGURATION, which a reset does not change, so both stay\n// as they were.\nlet reset_at = crate::state::now();\nendpoint.reset_at = Some(reset_at);" — `reset_at` is the only field assigned; no content of any kind is removed.

The crate does persist ingested content, so "holds no media" is not a property of the store: services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/state.rs:1081 "pub ingested_objects: BTreeMap<String, Vec<u8>>," on `Channel`, written by the ingest data plane at services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/service.rs:665 "match store.record_ingested_object(&region, &target, body.to_vec())".

Test coverage: tests/all/origin_endpoint_reset.rs asserts only the recorded instant and that the configuration/ETag are untouched; no test asserts that a reset removed anything, and tests/all/multiview_channel_ingest.rs is the suite that pushes content in.

## Steps to Reproduce

1. Read services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/ops/reset_origin_endpoint_state.rs:5-10 — the handler doc says the endpoint's stored content is discarded.
2. Read the same file's handle body at lines 221-226 — the only write is `endpoint.reset_at = Some(reset_at)`, and the comment says the endpoint holds no media.
3. CreateChannelGroup g, CreateChannel g/c (HLS), CreateOriginEndpoint g/c/e, then PUT a credentialed object to the channel's published ingest URL so `Channel::ingested_objects` holds an entry.
4. Call ResetOriginEndpointState on g/c/e: 200 with a fresh ResetAt. The stored object is still retained (it still counts against the per-channel ingest ceiling), so no content was cleared — the operation only stamped a timestamp.

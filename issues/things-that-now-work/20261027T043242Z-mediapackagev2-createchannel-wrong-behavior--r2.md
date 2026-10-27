Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f0793968580971e18796aad3fd0f4b67dc633a3c

# A MULTIVIEW channel is minted with a redundant pair of ingest endpoints and its ingest data plane accepts pushes

_Filed: 2026-10-27T04:32:42Z_
_Operation: CreateChannel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f0793968580971e18796aad3fd0f4b67dc633a3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateChannel materialises `ingest_endpoints` for EVERY channel, unconditionally, including a channel whose `InputType` is MULTIVIEW. The model states that such a channel receives no ingest at all, and this crate itself quotes that sentence in `state::Channel::is_multiview`, whose doc declares it "the one answer every operation that has nothing to act on such a channel branches on". Two sibling handlers (`PutChannelPolicy`, `ResetChannelState`) do branch on it; the create does not.

Consequences, both from the same write:
(1) `CreateChannel`/`GetChannel`/`UpdateChannel` report two `IngestEndpoints[].Url` for a MULTIVIEW channel — a pair of published ingest URLs for a channel that composites its tiles from other channels and takes no ingest of its own. `IngestEndpoints` is NOT a required output member (`CreateChannelResponse$IngestEndpoints` carries no `@required`), so reporting none is representable.
(2) Because the URLs exist and resolve, the ingest data plane accepts a credentialed PUT/POST to them and persists the body against the MULTIVIEW channel: `serve_ingest` resolves the path and calls `store.record_ingested_object(...)`, which only looks the channel up by name (`channel_at_mut`) and never consults `input_type`. So a MULTIVIEW channel accumulates ingested objects it can never have received.

## Evidence

Smithy, `CreateChannelRequest$InputType` doc (smithy_query query="doc" shape="CreateChannelRequest" member="InputType"): "The input type is an immutable field. It defines whether the channel allows CMAF ingest, HLS ingest, or server-side multiview output. Multiview channels receive no ingest of their own." ... "MULTIVIEW – Server-side multiview. The channel receives no ingest of its own. Instead, it composites video from the source channels in its MultiviewConfiguration into a single tiled output stream."

The crate quotes the same sentence and declares the single branch point — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/state.rs:907-919:
  "/// Whether this channel composites video from the source channels in its multiview
   /// configuration instead of receiving ingest of its own, which is what the model's
   /// `InputType` doc says a [`INPUT_TYPE_MULTIVIEW`] channel does (\"The channel receives
   /// no ingest of its own. ...\")"
  "/// a channel branches on — rather than each handler inferring it from a different member."
  "pub fn is_multiview(&self) -> bool { self.input_type == INPUT_TYPE_MULTIVIEW }"

Siblings that DO branch on it:
- src/ops/put_channel_policy.rs:221 `if channel.is_multiview() {` (refuses MULTIVIEW_CHANNEL_POLICY_NOT_ALLOWED)
- src/ops/reset_channel_state.rs:213 `if channel.is_multiview() {` (refuses MULTIVIEW_RESET_NOT_ALLOWED)

The create does not — src/ops/create_channel.rs:464-467:
  "// The ingest URLs are a function of the parent group's egress domain and this
   // channel's name, computed once and stored: a later read returns these bytes.
   let ingest_endpoints =
       crate::state::ingest_endpoints(&group.egress_domain, &group.name, &name);"
and src/state.rs:1752-1762:
  "/// The redundant pair of ingest endpoints a channel is created with ...
   pub fn ingest_endpoints(egress_domain: &str, group: &str, channel: &str) -> Vec<IngestEndpoint> { ... (1..=INGEST_ENDPOINTS_PER_CHANNEL).map(...) }"

The data plane stores against it without consulting the input type — src/service.rs:663 `match store.record_ingested_object(&region, &target, body.to_vec())` and src/state.rs:731-755 (`record_ingested_object` -> `self.channel_at_mut(region, target)` -> `channel.ingested_objects.insert(...)`); `channel_at_mut` (src/state.rs:693-701) resolves by name only.

Test coverage: nothing pins ingest endpoints for a MULTIVIEW channel. tests/all/channel_lifecycle.rs:462-466 asserts "a channel must report at least one ingest endpoint" but its rows are HLS/CMAF only (channel_lifecycle.rs:332, 353, 376 — no MULTIVIEW row). tests/all/published_data_plane_urls.rs:251-253 even shows the exclusion was deliberate: the doc says "each with an input type from the model's `InputType` enum (`CMAF | HLS | MULTIVIEW`)" while the array under it is `const INGEST_CHANNELS: &[(&str, &str)] = &[("in-cmaf", "CMAF"), ("in-hls", "HLS")];`.

## Steps to Reproduce

1. CreateChannelGroup `g`.
2. CreateChannel `g`/`src` with `{"InputType": "CMAF"}` (a legal multiview tile source).
3. CreateChannel `g`/`mv` with `{"InputType": "MULTIVIEW", "MultiviewConfiguration": {"AvailableSources": ["src"], "AvailableLayouts": [...]}}`.
4. Inspect the 200 response: `IngestEndpoints` carries two entries with `Url` `https://<ingest-host>/in/v1/g/1/mv/index` and `.../2/mv/index`, for a channel the model says "receives no ingest of its own". GetChannel reports the same pair.
5. Send a SigV4-signed PUT with a body to one of those URLs: the push is accepted (200, `content-length: 0`) and the object is persisted against the MULTIVIEW channel's `ingested_objects`.

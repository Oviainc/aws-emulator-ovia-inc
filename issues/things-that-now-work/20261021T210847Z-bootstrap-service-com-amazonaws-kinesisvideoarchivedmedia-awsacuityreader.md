Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the Kinesis Video Archived Media emulator

_Filed: 2026-10-21T21:08:47Z_
_Operation: service-com-amazonaws-kinesisvideoarchivedmedia-awsacuityreader | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 6 operations in `src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and the crate holds no state. Nothing about this service works end-to-end today.

Implement a FIRST SLICE of 4 operations that forms the archived-media read loop for one video stream. This service has no create operation of its own (streams and fragments are ingested through the Kinesis Video Streams control/data plane), so the slice is a read-path lifecycle over a fragment archive held in `ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState`, which MUST be seedable (fragments appended to a stream's archive, e.g. by a test/seed helper or by the sibling kinesisvideo service's PutMedia if present). No canned payloads: each response must be computed from the persisted archive.

Happy path, in order (fields per OPERATIONS.md):

1. **ListFragments** — `POST /listFragments`. Input: `StreamName` XOR `StreamARN`, `FragmentSelector{FragmentSelectorType: SERVER_TIMESTAMP|PRODUCER_TIMESTAMP (required), TimestampRange{StartTimestamp, EndTimestamp} (both required)}`, `MaxResults` (1..1000), `NextToken` (1..4096, `^[a-zA-Z0-9+/]+={0,2}$`). Output: `Fragments` (list of `Fragment{FragmentNumber, FragmentSizeInBytes, ProducerTimestamp, ServerTimestamp, FragmentLengthInMilliseconds}`) filtered from the stored archive by the selected timestamp field, plus `NextToken` when more remain. Unknown stream → `ResourceNotFoundException` (404); neither/both of StreamName and StreamARN, or End<Start → `InvalidArgumentException` (400).

2. **GetMediaForFragmentList** — `POST /getMediaForFragmentList`. Input: `StreamName`/`StreamARN`, `Fragments` (list of fragment-number strings, len 1..1000, required). Output: `Payload` (@httpPayload blob, required) built by concatenating the stored media bytes of exactly the requested fragments in the order given, and `ContentType` header (e.g. `video/webm`). Fragment numbers not present in the archive → `InvalidArgumentException`; unknown stream → `ResourceNotFoundException`.

3. **GetClip** — `POST /getClip`. Input: `StreamName`/`StreamARN`, `ClipFragmentSelector{FragmentSelectorType (required), TimestampRange{StartTimestamp, EndTimestamp} (both required)}`. Output: `ContentType` = `video/mp4` and a `Payload` assembled from the fragments the archive actually holds in that range. If the stream exists but has no fragments in the range the real service fails with `ResourceNotFoundException`; a stream with no retention configured → `NoDataRetentionException`.

4. **GetHLSStreamingSessionURL** — `POST /getHLSStreamingSessionURL`. Input: `StreamName`/`StreamARN`, `PlaybackMode` (LIVE|LIVE_REPLAY|ON_DEMAND), `HLSFragmentSelector`, `ContainerFormat`, `DiscontinuityMode`, `DisplayFragmentTimestamp`, `Expires` (300..43200), `MaxMediaPlaylistFragmentResults` (1..5000). Output: `HLSStreamingSessionURL` — a URL for a session RECORDED in state (session token, stream key, playback mode, selector, fragment list snapshot, expiry = now + `Expires`, default 300s), not a random string thrown away. A second call must mint a distinct session, and an unknown stream must 404 rather than return a URL.

All four must resolve the stream the same way (StreamName or StreamARN; exactly one required) and read from the one shared engine store reached via `crate::state::service_state(&*self.engine)` — never a fresh `InMemoryEngine`.

## State Model

- `ArchivedMediaState`
  - `streams: HashMap<String /* stream name */, Stream>`
  - `arn_index: HashMap<String /* StreamARN */, String /* stream name */>` so ARN and name resolve to the same `Stream`
  - `sessions: HashMap<String /* session token */, StreamingSession>`
- `Stream`
  - `name: String`, `arn: String`
  - `data_retention_in_hours: u32` (0 ⇒ `NoDataRetentionException` from GetClip/GetHLS/GetDASH/GetImages)
  - `media_type: String` (e.g. `video/h264`), `codec_private_data: Option<Vec<u8>>`
  - `fragments: Vec<StoredFragment>` kept sorted by ingestion order
- `StoredFragment`
  - `fragment_number: String` (monotonically increasing, `^[0-9]+$`)
  - `producer_timestamp: SystemTime`, `server_timestamp: SystemTime`
  - `duration_millis: u64`, `size_bytes: u64`
  - `media: Vec<u8>` (the bytes GetMediaForFragmentList/GetClip concatenate)
- `StreamingSession`
  - `token: String`, `stream_name: String`, `protocol: HLS|DASH`, `playback_mode`, `container_format`, `discontinuity_mode`, `display_fragment_timestamp`
  - `fragment_numbers: Vec<String>` (the playlist snapshot, capped by MaxMediaPlaylistFragmentResults)
  - `created_at: SystemTime`, `expires_in_secs: u32`
- Pagination: `list_tokens: HashMap<String /* NextToken */, Cursor{stream_name, selector, offset}>` or an opaque base64 cursor that re-derives the offset — tokens must match `^[a-zA-Z0-9+/]+={0,2}$` and a replayed token must continue where the previous page stopped.

Out of scope for this slice (stay 501, filed separately later): GetDASHStreamingSessionURL and GetImages.

## Evidence

OPERATIONS.md (crate root) lists all 6 operations with routes, inputs, outputs and errors: `ListFragments POST /listFragments` with `FragmentSelector`/`MaxResults`/`NextToken` and @paginated(items=Fragments); `GetMediaForFragmentList POST /getMediaForFragmentList` with required `Fragments` list and required @httpPayload `Payload`; `GetClip POST /getClip` with required `ClipFragmentSelector` and `ContentType` @httpHeader; `GetHLSStreamingSessionURL POST /getHLSStreamingSessionURL` with `Expires` (300..43200) and output `HLSStreamingSessionURL`. smithy_query confirms shapes `Fragment{FragmentNumber,FragmentSizeInBytes,ProducerTimestamp,ServerTimestamp,FragmentLengthInMilliseconds}`, `FragmentSelector{FragmentSelectorType required, TimestampRange required}`, `ClipFragmentSelector{FragmentSelectorType required, ClipTimestampRange required}`. grep for `SCAFFOLDED, NOT IMPLEMENTED` matches all 6 files in src/ops/ (get_clip.rs, get_dash_streaming_session_url.rs, get_hls_streaming_session_url.rs, get_images.rs, get_media_for_fragment_list.rs, list_fragments.rs) — every handler answers 501 and persists nothing. README.md confirms restJson1 wire protocol and the shared-engine state API.

## Steps to Reproduce

1. Seed the emulator with one stream (name `demo-stream`, matching ARN, data retention 24h) holding ~5 fragments with increasing fragment numbers, producer/server timestamps and media bytes.
2. POST /listFragments with {StreamName: 'demo-stream', FragmentSelector: {FragmentSelectorType: 'SERVER_TIMESTAMP', TimestampRange: {StartTimestamp: <t0>, EndTimestamp: <t0+1h>}}, MaxResults: 2} — expect 200 with 2 Fragment entries carrying the seeded fragment numbers/timestamps/sizes and a NextToken.
3. Repeat the call with the returned NextToken — expect the NEXT fragments, no repeats, and no NextToken on the final page.
4. POST /getMediaForFragmentList with {StreamName: 'demo-stream', Fragments: [<first two fragment numbers>]} — expect 200, a Content-Type header, and a Payload whose bytes equal the concatenation of those two seeded fragments' media.
5. POST /getClip with {StreamName: 'demo-stream', ClipFragmentSelector: {FragmentSelectorType: 'PRODUCER_TIMESTAMP', TimestampRange: {StartTimestamp: <t0>, EndTimestamp: <t0+1h>}}} — expect 200, ContentType video/mp4, non-empty Payload derived from the fragments in range.
6. POST /getHLSStreamingSessionURL with {StreamName: 'demo-stream', PlaybackMode: 'ON_DEMAND', Expires: 600} — expect 200 and an HLSStreamingSessionURL; call again and expect a DIFFERENT session URL, with both sessions recorded in state.
7. Repeat each call with StreamName: 'no-such-stream' — expect ResourceNotFoundException (404), not 501 and not a success.
8. Call /listFragments with both StreamName and StreamARN set, and again with EndTimestamp before StartTimestamp — expect InvalidArgumentException (400) in both cases.

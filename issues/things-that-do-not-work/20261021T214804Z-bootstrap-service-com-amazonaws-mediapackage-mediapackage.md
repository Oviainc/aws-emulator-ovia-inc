Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the mediapackage emulator

_Filed: 2026-10-21T21:48:04Z_
_Operation: service-com-amazonaws-mediapackage-mediapackage | Class: missing_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 19 MediaPackage operations is an unmodified scaffold: the handlers under `services/service-com-amazonaws-mediapackage-mediapackage/src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and the crate holds no state. Implement a FIRST stateful slice covering the core channel/origin-endpoint lifecycle (restJson1, sigv4 signing name `mediapackage`).

### Happy-path operations to implement (in order)

1. **CreateChannel** — `POST /channels` -> 200. Input: `Id` (required), `Description`, `Tags`. Must persist a new channel keyed by `Id` and return the stored record: `Id`, `Arn` (`arn:aws:mediapackage:<region>:<account>:channels/<Id>`), `Description`, `CreatedAt` (ISO-8601 of creation time), `HlsIngest` with two generated `IngestEndpoint`s (each with `Id`, `Url`, `Username`, `Password`), `EgressAccessLogs`/`IngressAccessLogs`, `Tags`. A duplicate `Id` must fail with `UnprocessableEntityException` (HTTP 422), not silently overwrite.
2. **DescribeChannel** — `GET /channels/{Id}` -> 200. Must return exactly what CreateChannel stored (same `Arn`, `CreatedAt`, `Description`, ingest endpoints, `Tags`); unknown `Id` -> `NotFoundException` (HTTP 404).
3. **CreateOriginEndpoint** — `POST /origin_endpoints` -> 200. Input: `ChannelId` (required), `Id` (required), plus `Description`, `ManifestName`, `HlsPackage`/`DashPackage`/`CmafPackage`/`MssPackage`, `Authorization`, `Origination`, `StartoverWindowSeconds`, `TimeDelaySeconds`, `Whitelist`, `Tags`. Must reject a `ChannelId` that does not exist with `NotFoundException` (404), persist the endpoint, and return the stored record including a derived playback `Url` (containing the endpoint `Id`/`ManifestName`) and `CreatedAt`. All packaging config supplied on input must be stored and echoed back from state on later reads, not dropped.
4. **DescribeOriginEndpoint** — `GET /origin_endpoints/{Id}` -> 200. Returns the persisted endpoint (same `Url`, `ChannelId`, packaging settings); unknown `Id` -> `NotFoundException` (404).
5. **ListOriginEndpoints** — `GET /origin_endpoints` -> 200. Must list endpoints from state, honour the `channelId` query filter, and honour `maxResults` (1..1000) + `nextToken` paging (`@paginated(inputToken=NextToken, outputToken=NextToken, items=OriginEndpoints)`): return `NextToken` only when more items remain, and resume exactly where the token left off.
6. **DeleteOriginEndpoint** / **DeleteChannel** — `DELETE /origin_endpoints/{Id}` and `DELETE /channels/{Id}` -> 202 with empty body. Must remove the record from state so a subsequent Describe returns `NotFoundException` (404); deleting an unknown id is itself a 404.

`ListChannels` (`GET /channels`, paginated on `Channels`) should be included in the slice so a created channel is observable in a listing with the same paging rules.

Every one of these must compute its response from persisted state — no canned bodies, no echoing the request back, and no generating an `Arn`/`Url`/ingest credentials that are not stored for later reads. A create followed by a describe/list must return precisely what was created; a delete must be observable as a 404.

## State Model

Service state (behind the `crate::state::service_state(&*self.engine)` lock):

- `channels: BTreeMap<String /* channel Id */, Channel>` where `Channel { id: String, arn: String, description: Option<String>, created_at: String /* ISO-8601 */, hls_ingest: HlsIngest { ingest_endpoints: Vec<IngestEndpoint { id, url, username, password }> }, egress_access_logs: EgressAccessLogs { log_group_name: Option<String> }, ingress_access_logs: IngressAccessLogs { log_group_name: Option<String> }, tags: BTreeMap<String, String> }`
- `origin_endpoints: BTreeMap<String /* endpoint Id */, OriginEndpoint>` where `OriginEndpoint { id, arn, channel_id, description, manifest_name, url, created_at, hls_package, dash_package, cmaf_package, mss_package, authorization, origination, startover_window_seconds, time_delay_seconds, whitelist: Vec<String>, tags: BTreeMap<String,String> }`
- `harvest_jobs: BTreeMap<String, HarvestJob>` — reserved for the later slice (CreateHarvestJob / DescribeHarvestJob / ListHarvestJobs stay 501 for now).
- Pagination: list tokens are opaque strings encoding the last-returned key, resolved against the `BTreeMap` ordering so paging is stable across calls.

BTreeMap ordering gives deterministic list output. Tags for the bootstrap slice are stored on the resource records so `TagResource`/`ListTagsForResource` can be layered on later by ARN lookup.

## Out of scope (stay 501, filed separately)

UpdateChannel, UpdateOriginEndpoint, ConfigureLogs, RotateChannelCredentials, RotateIngestEndpointCredentials, TagResource, UntagResource, ListTagsForResource, CreateHarvestJob, DescribeHarvestJob, ListHarvestJobs.

## Evidence

`services/service-com-amazonaws-mediapackage-mediapackage/OPERATIONS.md` lists all 19 operations with routes, input/output members and errors — e.g. CreateChannel `POST /channels -> 200` (Input: `Id` required, `Description`, `Tags`; Output: `Arn`, `CreatedAt`, `HlsIngest`, ...), DescribeChannel `GET /channels/{Id} -> 200`, CreateOriginEndpoint `POST /origin_endpoints -> 200` (Output adds `Url`), ListOriginEndpoints `GET /origin_endpoints -> 200` with `@paginated(inputToken=NextToken, outputToken=NextToken, items=OriginEndpoints)` and `MaxResults` range 1..1000, DeleteChannel/DeleteOriginEndpoint `DELETE ... -> 202`, and `NotFoundException (HTTP 404)` / `UnprocessableEntityException (HTTP 422)` on each. `src/ops/create_channel.rs` is representative of all handlers: it carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("CreateChannel"))`, so no state exists today.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI mediapackage client at it.
2. `aws mediapackage create-channel --id demo-channel --description 'bootstrap demo'` — expect 200 with `Id=demo-channel`, an `Arn` ending in `channels/demo-channel`, a `CreatedAt` timestamp and two `HlsIngest.IngestEndpoints` with urls/credentials. Today: HTTP 501 NotImplemented.
3. `aws mediapackage describe-channel --id demo-channel` — expect the identical record (same Arn, CreatedAt, Description, ingest endpoints) read back from state.
4. `aws mediapackage list-channels` — expect `demo-channel` in `Channels`.
5. `aws mediapackage create-origin-endpoint --channel-id demo-channel --id demo-hls --manifest-name index --hls-package '{"SegmentDurationSeconds":6}'` — expect 200 with a playback `Url` and the stored `HlsPackage`.
6. `aws mediapackage create-origin-endpoint --channel-id no-such-channel --id x --hls-package '{}'` — expect `NotFoundException` (HTTP 404).
7. `aws mediapackage describe-origin-endpoint --id demo-hls` — expect the same `Url`, `ChannelId=demo-channel` and `HlsPackage` as created.
8. `aws mediapackage list-origin-endpoints --channel-id demo-channel --max-results 1` — expect exactly one endpoint and a `NextToken` only if more remain; re-issuing with the token must resume without repeats.
9. `aws mediapackage delete-origin-endpoint --id demo-hls` then `describe-origin-endpoint --id demo-hls` — expect 202 then `NotFoundException` (404).
10. `aws mediapackage delete-channel --id demo-channel` then `describe-channel --id demo-channel` — expect 202 then `NotFoundException` (404).

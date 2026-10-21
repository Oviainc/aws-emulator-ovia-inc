Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the MediaPackageV2 emulator

_Filed: 2026-10-21T21:47:58Z_
_Operation: service-com-amazonaws-mediapackagev2-mediapackagev2 | Class: missing_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 30 MediaPackageV2 operations is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST, SMALL slice: the channel-group -> channel -> origin-endpoint creation lifecycle, which is the service's core workflow (restJson1, sigv4 name `mediapackagev2`).

Happy path to implement, in order:

1. `CreateChannelGroup` — `POST /channelGroup`. Input: `ChannelGroupName` (required, len 1..256, `^[a-zA-Z0-9_-]+$`), optional `Description`, `Tags`, `ClientToken` (header `x-amzn-client-token`). Must persist a new channel group and return the stored `ChannelGroupName`, a generated `Arn` (`arn:aws:mediapackagev2:<region>:<account>:channelGroup/<name>`), a generated `EgressDomain`, `CreatedAt`, `ModifiedAt`, `ETag`, `Description`, `Tags`. A second create with the same name must return `ConflictException` (409), not a second group.

2. `CreateChannel` — `POST /channelGroup/{ChannelGroupName}/channel`. Input: `ChannelGroupName` (@httpLabel), `ChannelName` (required), optional `Description`, `InputType` (CMAF|HLS|MULTIVIEW), `Tags`. Must 404 `ResourceNotFoundException` when the parent group does not exist; otherwise persist the channel under its group and return `Arn`, `ChannelName`, `ChannelGroupName`, `CreatedAt`, `ModifiedAt`, `Description`, `InputType`, `ETag`, `Tags`, and a non-empty `IngestEndpoints` list whose `Url` values are derived from the parent group's `EgressDomain` and the channel name (stored, so later reads return the identical URLs).

3. `CreateOriginEndpoint` — `POST /channelGroup/{ChannelGroupName}/channel/{ChannelName}/originEndpoint`. Input: `OriginEndpointName` (required), `ContainerType` (required, CMAF|ISM|TS), optional `Segment`, `Description`, `StartoverWindowSeconds`, `HlsManifests`/`LowLatencyHlsManifests`/`DashManifests`, `Tags`. Must 404 if the group or channel is missing, 409 if the endpoint name already exists. Must persist all submitted fields and return them back, converting each `Create*ManifestConfiguration` into the corresponding `Get*ManifestConfiguration` with a computed `Url`, plus `Arn`, `CreatedAt`, `ModifiedAt`, `ETag`, and a materialized `Segment` (required in the output even when the input omits it — apply the model's defaults).

4. `GetChannel` — `GET /channelGroup/{ChannelGroupName}/channel/{ChannelName}/`. Must return exactly what step 2 stored (same `Arn`, `IngestEndpoints`, `Description`, `InputType`, `CreatedAt`, `ETag`, `Tags`) and `ResourceNotFoundException` (404) for an unknown group or channel.

5. `ListOriginEndpoints` — `GET /channelGroup/{ChannelGroupName}/channel/{ChannelName}/originEndpoint`. Must return one `OriginEndpointListConfiguration` per persisted endpoint of that channel (and nothing from other channels), honouring `maxResults` (1..100, default 10) by truncating the page and returning an opaque `NextToken` that resumes exactly where the previous page stopped; 404 for an unknown group/channel.

6. `DeleteOriginEndpoint` then `DeleteChannel` then `DeleteChannelGroup` — the DELETE routes above. Must remove the resource from state so a following `GetOriginEndpoint`/`GetChannel`/`GetChannelGroup` returns 404, and must be idempotent (@idempotent: deleting an absent resource succeeds). `DeleteChannelGroup`/`DeleteChannel` must reject deletion while children still exist with `ConflictException` (409), matching the real service's refusal to delete a non-empty parent.

Every one of these must compute its response from persisted state: no canned bodies, no echoing the request back without storing it, no generating an ARN/ETag that a later Get cannot reproduce. Ingest/manifest URLs, timestamps and ETags created in step 1-3 must be byte-identical when read back in step 4-5.

## State Model

A single service store behind the usual lock, keyed by region+account:

- `channel_groups: Map<String /*ChannelGroupName*/, ChannelGroup>`
- `ChannelGroup { name: String, arn: String, egress_domain: String, description: Option<String>, created_at: Timestamp, modified_at: Timestamp, etag: String, tags: Map<String,String>, channels: Map<String /*ChannelName*/, Channel> }`
- `Channel { name: String, group_name: String, arn: String, description: Option<String>, input_type: Option<InputType>, ingest_endpoints: Vec<IngestEndpoint{ id: String, url: String }>, input_switch_configuration: Option<InputSwitchConfiguration>, output_header_configuration: Option<OutputHeaderConfiguration>, created_at: Timestamp, modified_at: Timestamp, reset_at: Option<Timestamp>, etag: String, tags: Map<String,String>, origin_endpoints: Map<String /*OriginEndpointName*/, OriginEndpoint>, policy: Option<String> }`
- `OriginEndpoint { name: String, channel_name: String, group_name: String, arn: String, container_type: ContainerType, segment: Segment, description: Option<String>, startover_window_seconds: Option<i32>, hls_manifests: Vec<GetHlsManifestConfiguration>, low_latency_hls_manifests: Vec<GetLowLatencyHlsManifestConfiguration>, dash_manifests: Vec<GetDashManifestConfiguration>, mss_manifests: Vec<GetMssManifestConfiguration>, force_endpoint_error_configuration: Option<ForceEndpointErrorConfiguration>, created_at: Timestamp, modified_at: Timestamp, reset_at: Option<Timestamp>, etag: String, tags: Map<String,String>, policy: Option<String> }`
- Pagination tokens for `ListOriginEndpoints` encode the last-returned endpoint name (sorted order) so pages do not overlap or skip.

Out of scope for this first slice (stays 501, filed separately later): Update*, Reset*State, the channel/origin-endpoint policy operations, harvest jobs, ListChannelGroups/ListChannels, and the Tag/Untag/ListTagsForResource trio.

## Evidence

`services/service-com-amazonaws-mediapackagev2-mediapackagev2/OPERATIONS.md` lists all 30 operations with their restJson1 routes and members, and the routes/fields quoted above are taken verbatim from it (CreateChannelGroup `POST /channelGroup`; CreateChannel `POST /channelGroup/{ChannelGroupName}/channel` with required `ChannelName` and output `IngestEndpoints`; CreateOriginEndpoint with required `ContainerType` and required output `Segment`; GetChannel `GET /channelGroup/{ChannelGroupName}/channel/{ChannelName}/`; ListOriginEndpoints @paginated(inputToken=NextToken, outputToken=NextToken, items=Items) with `MaxResults` range 1..100 default 10; the three DELETE routes marked @idempotent and declaring ConflictException 409). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/ops/` matches all 30 handler files, each returning `crate::scaffolded::Scaffolded::not_implemented(..)` (HTTP 501), so no state is held anywhere and no gap-level behavior exists yet.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI MediaPackageV2 client at it.
2. `aws mediapackagev2 create-channel-group --channel-group-name demo-group --description 'bootstrap'` — expect 200 with an Arn, EgressDomain, CreatedAt/ModifiedAt and ETag (currently 501).
3. `aws mediapackagev2 create-channel --channel-group-name demo-group --channel-name demo-channel --input-type HLS` — expect 200 with IngestEndpoints derived from the group's EgressDomain.
4. `aws mediapackagev2 create-origin-endpoint --channel-group-name demo-group --channel-name demo-channel --origin-endpoint-name demo-ep --container-type TS --hls-manifests '[{"ManifestName":"index"}]'` — expect 200 with a GetHlsManifestConfiguration carrying a Url and a populated Segment.
5. `aws mediapackagev2 get-channel --channel-group-name demo-group --channel-name demo-channel` — expect the exact Arn, IngestEndpoints, Description and InputType stored by step 3.
6. `aws mediapackagev2 list-origin-endpoints --channel-group-name demo-group --channel-name demo-channel --max-results 1` — expect demo-ep in Items; after creating a second endpoint, expect a NextToken that returns the remaining endpoint and no duplicates.
7. `aws mediapackagev2 delete-channel-group --channel-group-name demo-group` while the channel still exists — expect ConflictException (409).
8. Delete in order: delete-origin-endpoint, delete-channel, delete-channel-group — each 2xx; then get-channel and get-channel-group must both return ResourceNotFoundException (404), and re-running the deletes must still succeed (idempotent).

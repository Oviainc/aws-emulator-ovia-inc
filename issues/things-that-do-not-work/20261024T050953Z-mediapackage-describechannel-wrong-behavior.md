Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# MediaPackage state is not partitioned by Region: a channel/endpoint/harvest job created in one Region resolves, lists and tags in every other

_Filed: 2026-10-24T05:09:53Z_
_Operation: DescribeChannel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: MediaPackage is a Region-scoped control plane. A Channel, OriginEndpoint or HarvestJob created in one Region exists only in that Region: `DescribeChannel`/`DescribeOriginEndpoint`/`DescribeHarvestJob` issued against another Region answer `NotFoundException` (HTTP 404), `ListChannels`/`ListOriginEndpoints`/`ListHarvestJobs` in another Region do not report it, and because an id only has to be "unique within the region" a second `CreateChannel` with the SAME `Id` in a different Region SUCCEEDS (it creates a second, independent channel whose ARN names the second Region). The tagging operations are equally scoped: a `ResourceArn` whose region field names a Region in which the resource does not exist does not resolve to the resource.

Actual: the whole service keeps exactly three global maps keyed by id only — `ComAmazonawsMediapackageMediapackageState { channels: BTreeMap<String, ChannelRecord>, origin_endpoints: BTreeMap<String, OriginEndpointRecord>, harvest_jobs: BTreeMap<String, HarvestJobRecord>, .. }` (`src/state.rs` ~line 1213) — and no record carries the Region it was created in. The request Region is read only to MINT a value (`request_region(ctx.parts())` is used for the ARN, the ingest URLs and the playback URL in `create_channel.rs`, `create_origin_endpoint.rs`, `create_harvest_job.rs`); every lookup discards it. Consequences, all observable over HTTP:

1. Cross-Region reads succeed. `DescribeChannel`, `DescribeOriginEndpoint`, `DescribeHarvestJob`, `ListChannels`, `ListOriginEndpoints` and `ListHarvestJobs` do not even look at the request Region (`let _ = ctx;` is the first statement of each handler), so a channel created with a `us-east-1`-signed request is returned verbatim — including its `us-east-1` ARN and `us-east-1` ingest URLs — to a request signed for `eu-west-1`, and it is listed there.
2. Cross-Region id collision. `CreateChannel` refuses a duplicate id globally (`if state.channels.contains_key(&input.id) { resource_already_exists(..) }`, 422), so creating channel `X` in `us-east-1` makes `CreateChannel(Id="X")` in `eu-west-1` fail with `UnprocessableEntityException` where AWS creates a second channel. The same holds for `CreateOriginEndpoint` and `CreateHarvestJob`.
3. Cross-Region mutation and deletion. `UpdateChannel`, `ConfigureLogs`, `RotateChannelCredentials`, `RotateIngestEndpointCredentials`, `UpdateOriginEndpoint`, `DeleteChannel` and `DeleteOriginEndpoint` resolve their `{Id}` label against the same global map, so a request signed for one Region mutates or deletes a resource that lives in another.
4. Cross-Region tagging. `parse_tag_arn` (`src/state.rs` ~line 1559) explicitly throws the ARN's Region away (`let _region = fields.next()?;`) and `tags`/`tags_mut` key on the id alone, so `TagResource`/`UntagResource`/`ListTagsForResource` with an ARN naming the wrong Region resolve to the resource instead of answering not-found.
5. `CreateOriginEndpoint`'s `channelId` reference and `CreateHarvestJob`'s `originEndpointId` reference resolve across Regions, so an endpoint can be created in `eu-west-1` packaging a channel that only exists in `us-east-1` — and its playback URL is then minted for the requesting Region while the parent's ingest URLs name another.

## Evidence

Spec grounding: the model documents `CreateChannelRequest$Id` as "The ID of the Channel. The ID must be unique within the region and it cannot be changed after a Channel is created" (and `CreateOriginEndpointRequest$Id`/`CreateHarvestJobRequest$Id` carry the same per-region uniqueness wording) — i.e. the uniqueness scope the service enforces is the REGION, not the account. MediaPackage's `awsService` trait gives it an `endpointPrefix`/`arnNamespace` of `mediapackage` and every resource ARN this service mints carries a region field (`arn:aws:mediapackage:<region>:<account>:channels/<id>`, `src/state.rs` `channel_arn`/`origin_endpoint_arn`/`harvest_job_arn`), which is only meaningful if the resource belongs to that one Region. `NotFoundException` (httpError 404) is declared on every read in the checklist, so the Region-miss answer is modelled.

Handler/state evidence: `src/state.rs` ~line 1213 — `ComAmazonawsMediapackageMediapackageState` holds `channels`, `origin_endpoints` and `harvest_jobs` as flat `BTreeMap<String, Record>` with no Region key and no `region` field on `ChannelRecord` (~line 972), `OriginEndpointRecord` or `HarvestJobRecord` (~line 1150). `src/state.rs` `request_region` (~line 1495) exists and is called ONLY from the three create handlers to build ARNs/URLs. `src/ops/describe_channel.rs:149`, `describe_origin_endpoint.rs:170`, `describe_harvest_job.rs:161`, `list_channels.rs:147`, `list_origin_endpoints.rs:150`, `list_harvest_jobs.rs:159`, `update_channel.rs:153`, `configure_logs.rs:154`, `delete_channel.rs:139`, `delete_origin_endpoint.rs:138`, `rotate_channel_credentials.rs:158`, `rotate_ingest_endpoint_credentials.rs:163`, `tag_resource.rs:164`, `untag_resource.rs:159`, `list_tags_for_resource.rs:147` all begin by discarding the request context (`let _ = ctx;`), so none of them can be Region-scoped. `src/state.rs` `parse_tag_arn` discards the ARN's region field.

Test coverage: no test asserts Region isolation. Every integration test signs all of its requests for one single `REGION` constant (`tests/all/channel_lifecycle.rs` line 24, `tests/all/error_envelope.rs` line 40, `tests/all/resource_tags.rs`, `tests/all/mutation_persistence.rs`, …) and `grep -n 'region' tests/` turns up only ARN-formatting helpers — nothing creates a resource under one Region and reads it under another.

Precedent in this repo: the identical defect was filed and fixed for the sibling service (`20261024T010403Z-mediapackage-vod-describepackaginggroup-wrong-behavior.md`, "MediaPackage VOD state is not partitioned by Region: a group/configuration/asset created in one Region resolves and lists in every other"), so the expected shape of the fix is established; the v1 MediaPackage store never received the same treatment.

## Steps to Reproduce

1. Boot the emulator and create two boto3 mediapackage clients against it, one with region_name="us-east-1" and one with region_name="eu-west-1" (SigV4 credential scope is what the emulator reads as the Region).
2. us_east.create_channel(Id="region-probe") -> 200, Arn = arn:aws:mediapackage:us-east-1:<acct>:channels/region-probe.
3. eu_west.describe_channel(Id="region-probe") -> observe HTTP 200 returning the us-east-1 channel (ARN and HlsIngest URLs still name us-east-1). AWS answers NotFoundException / 404.
4. eu_west.list_channels() -> observe the us-east-1 channel in Channels. AWS returns an empty list.
5. eu_west.create_channel(Id="region-probe") -> observe UnprocessableEntityException / 422 "channel with id=region-probe already exists". AWS creates an independent eu-west-1 channel with the same Id.
6. eu_west.list_tags_for_resource(ResourceArn="arn:aws:mediapackage:eu-west-1:<acct>:channels/region-probe") -> observe 200 with the us-east-1 channel's tags (the ARN's region is discarded by parse_tag_arn). AWS answers not-found.
7. eu_west.delete_channel(Id="region-probe") -> observe 202 and then us_east.describe_channel(Id="region-probe") -> 404: a request signed for another Region deleted the us-east-1 resource.
8. Repeat steps 2-7 for origin endpoints (create_origin_endpoint/describe_origin_endpoint/list_origin_endpoints) and harvest jobs (create_harvest_job/describe_harvest_job/list_harvest_jobs) to see the same cross-Region leakage.

Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# MediaPackageV2 state is not partitioned by Region: a channel group/channel/endpoint created in one Region resolves and lists in every other

_Filed: 2026-10-24T05:11:59Z_
_Operation: GetChannelGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

MediaPackage v2 resources are regional. The model states it explicitly for the primary identifier: `CreateChannelGroupRequest$ChannelGroupName` — "The name is the primary identifier for the channel group, and must be unique for your account in the AWS Region." Two accounts/Regions therefore have independent namespaces, and a channel group created in us-west-2 must NOT be visible through the eu-west-1 endpoint (real AWS answers `ResourceNotFoundException`, HTTP 404, and `ListChannelGroups` in the other Region returns an empty `Items`).

The emulator keeps ONE global store: `ComAmazonawsMediapackagev2Mediapackagev2State { channel_groups: BTreeMap<String, ChannelGroup> }` keyed by the group NAME only, with no Region in the key and no Region field on `ChannelGroup`, `Channel`, `OriginEndpoint` or `HarvestJobRecord`. `crate::state::request_region(ctx.parts())` is read ONLY to build ARNs at create time; every lookup (`store.channel_groups.get(&input.channel_group_name)`) ignores the Region the request was signed for.

Observable consequences across the whole hierarchy:
- `GetChannelGroup` / `GetChannel` / `GetOriginEndpoint` / `GetHarvestJob` signed for Region B return HTTP 200 for a resource created in Region A — and the body carries the Region-A ARN, so a response served from the eu-west-1 endpoint reports `arn:aws:mediapackagev2:us-west-2:...`.
- `ListChannelGroups` / `ListChannels` / `ListOriginEndpoints` / `ListHarvestJobs` in Region B enumerate Region A's resources.
- `CreateChannelGroup` with a name already used in another Region is refused with `ConflictException` instead of succeeding (the name is only required to be unique *within* a Region).
- `DeleteChannelGroup` / `UpdateChannel` / `PutChannelPolicy` etc. signed for Region B mutate Region A's resources.
- `MAX_CHANNEL_GROUPS` is applied account-wide rather than per Region.

## Evidence

Spec: Smithy `CreateChannelGroupRequest$ChannelGroupName` doc — "must be unique for your account in the AWS Region"; well-established AWS behavior that mediapackagev2 is a regional service (`endpointPrefix`/`sigv4Name` `mediapackagev2`, regional endpoints `mediapackagev2.<region>.amazonaws.com`, and the ARNs this very service mints embed a Region).

Handler/state: `src/state.rs` — `ComAmazonawsMediapackagev2Mediapackagev2State` holds a single `channel_groups: BTreeMap<String, ChannelGroup>`; `ChannelGroup` has no region member. `src/ops/get_channel_group.rs:179` (`store.channel_groups.get(&input.channel_group_name)`), `src/ops/list_channel_groups.rs:204`, `src/ops/get_channel.rs:211`, `src/ops/get_origin_endpoint.rs:214`, `src/ops/list_harvest_jobs.rs:213` all look up by name alone. `request_region` is used only in `create_channel_group.rs:216`, `create_channel.rs:340`, `create_origin_endpoint.rs:308`, `create_harvest_job.rs:223` to build ARNs.

Tests: `tests/all/*` sign every request for a single constant region (`const REGION: &str = "us-west-2"` in channel_lifecycle.rs, channel_policy.rs, error_bodies.rs, rejection_rule_and_negative_paths.rs, …). No test exercises two Regions, so nothing asserts the current global behavior. The equivalent gap was filed and fixed for MediaPackage VOD (20261024T010403Z-mediapackage-vod-describepackaginggroup-wrong-behavior.md).

## Steps to Reproduce

1. Sign a `CreateChannelGroup` request for us-west-2 (`POST /channelGroup`, SigV4 credential scope `.../us-west-2/mediapackagev2/aws4_request`) with `ChannelGroupName: "tg"`; note the returned ARN names us-west-2.
2. Send `GET /channelGroup/tg` signed for eu-west-1. Expected: HTTP 404 `ResourceNotFoundException`. Actual: HTTP 200 with the us-west-2 ARN.
3. Send `GET /channelGroup?maxResults=10` signed for eu-west-1. Expected: `Items: []`. Actual: the us-west-2 group is listed.
4. Send `POST /channelGroup` signed for eu-west-1 with `ChannelGroupName: "tg"`. Expected: HTTP 200 (a distinct eu-west-1 group). Actual: HTTP 409 `ConflictException`.
5. Repeat one level down: create a channel/origin endpoint in us-west-2, then `GET /channelGroup/tg/channel/tc/` and `GET /channelGroup/tg/channel/tc/originEndpoint/te` signed for eu-west-1 — both answer 200 instead of 404.

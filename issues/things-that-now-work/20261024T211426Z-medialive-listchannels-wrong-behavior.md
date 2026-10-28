Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e0e9a90161aff7ec754055c046e25b8f716c6f2d

# MediaLive resource stores are not Region-scoped: a channel/input/multiplex created through one Region's endpoint is listed, read and mutated through every other Region

_Filed: 2026-10-24T21:14:26Z_
_Operation: ListChannels | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e0e9a90161aff7ec754055c046e25b8f716c6f2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

MediaLive is a regional service: a channel, input, input security group, multiplex, cluster, network, node, SDI source, signal map or claimed input device exists only in the Region whose endpoint created it. A `ListChannels` issued against eu-west-1 must not report a channel created against us-east-1, and `DescribeChannel`/`StartChannel`/`DeleteChannel` for that id against another Region must answer the modelled `NotFoundException` (404).

The emulator keeps ONE global store and only the monitor-template hierarchy (CloudWatch-alarm-template groups/templates, EventBridge-rule-template groups/templates) plus the per-region `AccountConfiguration` are Region-keyed. Every other resource is keyed by its minted id alone, with the request's Region used only to build the ARN string at create time. So:

* Expected: `ListChannels` through Region B returns an empty `channels` list for a channel created in Region A; `DescribeChannel`/`StartChannel`/`BatchDelete` through Region B answer 404 for that id.
* Actual: the channel is listed in every Region, `DescribeChannel` returns it with an `arn` that names Region A while the caller addressed Region B, and every mutation (start/stop/delete/tag/BatchUpdateSchedule) succeeds cross-Region.

The same holds for `ListInputs`-class readers over inputs, `ListMultiplexes`, `ListInputSecurityGroups`, `ListClusters`, `ListNetworks`, `ListSdiSources`, `ListSignalMaps`, `ListInputDevices`, `ListChannelPlacementGroups` and every Describe/Update/Delete that resolves one of those ids.

## Evidence

`services/service-com-amazonaws-medialive-medialive/src/state.rs` holds a single `ComAmazonawsMedialiveMedialiveState` reached by `service_state(&*self.engine)` (state.rs ~line 4766) with no Region partition; `channels`, `inputs`, `multiplexes`, `input_security_groups`, `clusters`, `networks`, `nodes`, `sdi_sources`, `signal_maps`, `input_devices` are plain `BTreeMap`s keyed by id. `ChannelRecord` carries no `region` member (only `CloudWatchAlarmTemplateGroupRecord`/`EventBridgeRuleTemplateGroupRecord` do, state.rs ~line 1073). `ops/create_channel.rs` line 231 reads `crate::state::request_region(ctx.parts())` solely to build the ARN (line 267) and never stores it; `ops/list_channels.rs` `handle` never looks at the request Region at all and range-scans `state.channels` directly. Grounded in the Smithy model's regional `awsService`/sigv4 binding (channel ARNs embed a Region) and in MediaLive's documented per-Region resource model. No test asserts cross-Region isolation.

## Steps to Reproduce

1. CreateChannel with SigV4 credentials scoped to us-east-1; note the returned channel Id and the us-east-1 ARN
2. Call ListChannels with SigV4 credentials scoped to eu-west-1
3. Observe the us-east-1 channel in the eu-west-1 response (AWS returns an empty list)
4. Call DescribeChannel for that Id against eu-west-1 and observe 200 with an arn naming us-east-1 (AWS answers 404 NotFoundException)
5. Call StartChannel / CreateTags for that Id against eu-west-1 and observe the mutation succeed

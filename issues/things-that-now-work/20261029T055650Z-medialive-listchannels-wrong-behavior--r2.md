Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 12966d24554ed06eeaba7911ae25ff590cd06862

# ChannelSummary$usedChannelEngineVersions is hardcoded absent, the one member of the channel projections defaulted with no stated reason

_Filed: 2026-10-29T05:56:50Z_
_Operation: ListChannels | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `12966d24554ed06eeaba7911ae25ff590cd06862`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ComAmazonawsMedialiveMedialiveState::channel_summary` is the single projection `ListChannels` answers from. Every one of its members is read off the stored `ChannelRecord` or derived from the rest of the store — except `used_channel_engine_versions`, which is written as a literal `None` with no comment at all. So a RUNNING channel never reports the engine version its pipelines are running, however the channel was pinned.

That is not a fact the crate lacks. The line directly above it derives the sibling member `channelEngineVersion` from the stored pin (`channel.channel_engine_version`), the record knows exactly which pipelines are up (`ChannelRecord::healthy_pipelines` / `pipeline_details`, which the same projection already uses for `pipelinesRunningCount`), and the account's engine-version catalogue (`channel_engine_versions`, keyed by the very build identifier a channel stores) is a public field of the same state struct this method is an inherent method of — `ListVersions` already serves entries out of it.

Two in-crate rules are broken by the silent default:

* the state module states that every channel-shaped response member is either stored verbatim or derived from stored state, and that "no operation builds a response out of its own request";
* the channel-output macro's own doc says the macro exists so that "every member of every one of these shapes is still named exactly once and adding one to the model cannot be defaulted in silently" — and `channel_summary` (which is hand-written rather than going through that macro) does exactly that.

The asymmetry is what makes this a defect rather than a judgement call: every OTHER absent member of the channel projections carries a written justification (the engine version's `expirationDate`, the VPC description's `availabilityZones`/`networkInterfaceIds`, the maintenance window's `maintenanceDeadline`/`maintenanceScheduledDate`, the router destination's `routerOutputArn`, `RestartChannelPipelinesOutput$MaintenanceStatus`). `usedChannelEngineVersions` is the only one that is absent for no recorded reason, so a caller cannot tell a deliberate omission from an unfinished member — and in fact the fact it would report is one the store holds.

## Evidence

The model states what the member reports — `smithy_query(query="doc", shape="ChannelSummary", member="UsedChannelEngineVersions")`:
```
{"shape": "ChannelSummary", "member": "UsedChannelEngineVersions", "documentation": "The engine version that the running pipelines are using."}
```
(target `__listOfChannelEngineVersionResponse`, jsonName `usedChannelEngineVersions`).

The projection that answers `ListChannels`, with the derived sibling member immediately above the hardcoded one — services/service-com-amazonaws-medialive-medialive/src/state.rs:3246-3256:
```
            pipelines_running_count: Some(channel.pipelines_running_count()),
            role_arn: channel.role_arn.clone(),
            state: Some(channel.state.clone()),
            tags: Some(channel.tags.clone()),
            vpc: channel.vpc.as_ref().map(described_vpc),
            anywhere_settings: channel.anywhere_settings.as_ref().map(described_anywhere),
            channel_engine_version: channel
                .channel_engine_version
                .as_deref()
                .map(described_engine_version),
            used_channel_engine_versions: None,
```
and it is the only reader of that shape — services/service-com-amazonaws-medialive-medialive/src/ops/list_channels.rs:181:
```
            .map(|(_, channel)| state.channel_summary(channel))
```

The crate rule it breaks — services/service-com-amazonaws-medialive-medialive/src/state.rs:140-150:
```
//! Every value a channel-, input-, input-security-group-, SdiSource-,
//! signal-map- or template-group-shaped response carries is either stored
//! verbatim from the request that created the resource or derived from the stored
//! state ([`ComAmazonawsMedialiveMedialiveState::channel_description`],
//! ...
//! [`event_bridge_rule_template_group_output`]) - no operation builds a response
//! out of its own request.
```
and the discipline the sibling channel-output macro states for exactly this hazard — src/state.rs:6215-6220:
```
/// One macro rather than five hand-written copies of the same 23-member move, so
/// the operations cannot drift from each other. A shape that carries a member
/// beyond the 23 (`RestartChannelPipelinesOutput$MaintenanceStatus`) passes it as
/// a trailing `member: value` pair, so every member of every one of these shapes
/// is still named exactly once and adding one to the model cannot be defaulted in
/// silently.
```

The facts the member needs are in the store. The notion of a running pipeline — src/state.rs:1416-1422:
```
    /// `PipelineDetail` is documented as "Runtime details of a pipeline when a
    /// channel is running", so there is one entry per healthy pipeline and none
    /// at all when the channel is not running.
    pub fn pipeline_details(&self) -> Vec<PipelineDetail> {
        self.healthy_pipelines()
```
and the version catalogue the same state struct owns, already served elsewhere — src/ops/list_versions.rs:163-165:
```
        Ok(ListVersionsOutput {
            versions: Some(state.channel_engine_versions.available_at(now)),
        })
```

Every other absent member of these projections is justified in a comment, which is what isolates this one. src/state.rs:5937-5939:
```
/// `ChannelEngineVersionRequest` (just a `Version`) -> the response shape.
/// `ExpirationDate` is "The UTC time when the version expires" — an AWS-side
/// release fact, not something this emulator knows, so it stays absent.
```
src/state.rs:5910-5912:
```
/// `AvailabilityZones` and `NetworkInterfaceIds` are left absent: the real
/// service fills them from the ENIs it allocates, which this emulator does not
/// create, so reporting values for them would be fabrication.
```
src/state.rs:5980-5984:
```
/// `MaintenanceDeadline` ("Maintenance is required by the displayed date and
/// time") and `MaintenanceScheduledDate` ("The currently scheduled maintenance
/// date and time") are assigned by the real service's maintenance scheduler,
/// which this emulator does not run, so they stay absent: no maintenance has
/// been scheduled for or required of this channel.
```
src/state.rs:5889-5891 (`routerOutputArn`) and src/ops/restart_channel_pipelines.rs:248-253 (`maintenance_status: None`, "ABSENT, not invented") say the same for their members. `used_channel_engine_versions: None` at src/state.rs:3256 carries no such line.

Test coverage: nothing pins the member — `grep -n 'usedChannelEngineVersions\|used_channel_engine_versions' services/service-com-amazonaws-medialive-medialive/tests/all` returns no matches, while tests/all/engine_version_catalog.rs covers the catalogue and `ListVersions`, and tests/all/channel_lifecycle.rs / tests/all/list_page_size.rs cover the `ListChannels` page itself. So deriving the member changes no asserted value.

## Steps to Reproduce

1. GET /prod/versions (ListVersions) and note a published entry, e.g. {"version": "1.3.0", "expirationDate": <instant>} — the catalogue src/ops/list_versions.rs:163-165 answers from.
2. POST /prod/channels with a runnable configuration and "channelEngineVersion": {"version": "1.3.0"}; note the channel id C.
3. POST /prod/channels/C/start and poll GET /prod/channels/C until "state" is "RUNNING" and "pipelinesRunningCount" is non-zero (so src/state.rs:1366 reports healthy pipelines).
4. GET /prod/channels (ListChannels) and read the summary for C: it carries "channelEngineVersion": {"version": "1.3.0"} but no "usedChannelEngineVersions" key at all, because src/state.rs:3256 hardcodes it to None.
5. Expected, per the member's own documentation ("The engine version that the running pipelines are using") and the crate's rule at src/state.rs:140-150, that the member is derived from the stored pin and the channel's healthy pipelines — the same two facts the adjacent `channelEngineVersion` and `pipelinesRunningCount` members of the SAME projection are already derived from — instead of being defaulted silently, which the channel-output macro's doc at src/state.rs:6215-6220 names as the hazard to avoid.
6. Confirm the omission is unpinned: grep for usedChannelEngineVersions under tests/all — no match — so a derived value breaks no existing assertion.

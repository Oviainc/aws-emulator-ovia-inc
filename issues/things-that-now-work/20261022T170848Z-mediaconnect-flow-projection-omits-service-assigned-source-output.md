Issue-Source: trace-sweep-divergence
Issue-Trace: mediaconnect-flow-lifecycle-basic
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediaconnect-flow-lifecycle-basic mediaconnect-flow-list-pagination mediaconnect-flow-sources-entitlements-update mediaconnect-flow-vpc-interfaces mediaconnect-flow-media-streams-and-outputs mediaconnect-flow-source-metadata-and-thumbnail
Issue-Shape-Id: com.amazonaws.mediaconnect#MediaConnect
Issue-Key: mediaconnect|DescribeFlow|2xx->2xx|$.flow.description
Issue-Key: mediaconnect|DescribeFlow|2xx->2xx|$.flow.mediaStreams
Issue-Key: mediaconnect|DescribeFlow|2xx->2xx|$.flow.source.routerIntegrationState
Issue-Key: mediaconnect|DescribeFlow|2xx->2xx|$.flow.source.transport.maxBitrate
Issue-Key: mediaconnect|DescribeFlow|2xx->2xx|$.flow.sources[*].routerIntegrationState
Issue-Key: mediaconnect|DescribeFlow|2xx->2xx|$.flow.sources[*].transport.maxBitrate
Issue-Key: mediaconnect|CreateFlow|2xx->2xx|$.flow.mediaStreams
Issue-Key: mediaconnect|CreateFlow|2xx->2xx|$.flow.source.routerIntegrationState
Issue-Key: mediaconnect|CreateFlow|2xx->2xx|$.flow.source.transport.maxBitrate
Issue-Key: mediaconnect|CreateFlow|2xx->2xx|$.flow.sources[*].routerIntegrationState
Issue-Key: mediaconnect|CreateFlow|2xx->2xx|$.flow.sources[*].transport.maxBitrate
Issue-Key: mediaconnect|AddFlowOutputs|2xx->2xx|$.outputs[*].routerIntegrationState
Issue-Key: mediaconnect|DescribeFlow|2xx->2xx|$.flow.outputs[*].routerIntegrationState
Issue-Key: mediaconnect|DescribeFlow|2xx->2xx|$.flow.sources[*]
Issue-Key: mediaconnect|UpdateFlow|2xx->2xx|$.flow.mediaStreams
Issue-Key: mediaconnect|UpdateFlow|2xx->2xx|$.flow.source.routerIntegrationState
Issue-Key: mediaconnect|UpdateFlow|2xx->2xx|$.flow.source.transport.maxBitrate
Issue-Key: mediaconnect|UpdateFlow|2xx->2xx|$.flow.sourceFailoverConfig.recoveryWindow
Issue-Key: mediaconnect|UpdateFlow|2xx->2xx|$.flow.sources[*]
Issue-Key: mediaconnect|UpdateFlow|2xx->2xx|$.flow.sources[*].routerIntegrationState
Issue-Key: mediaconnect|UpdateFlow|2xx->2xx|$.flow.sources[*].transport.maxBitrate
Issue-Key: mediaconnect|DescribeFlow|2xx->2xx|$.flow.sourceFailoverConfig.recoveryWindow
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# MediaConnect flow projection omits service-assigned source, output and failover members

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFlow

## Description
Four distinct root causes, all in the shared projection of a stored flow into the modeled `Flow` / `Source` / `Output` shapes used by CreateFlow, DescribeFlow, UpdateFlow and AddFlowOutputs. All four belong to service `mediaconnect`. Eight replayed divergence clusters carry them. Every cause must be closed before this issue is submitted.

=== Cause 1 of 4: a source's `transport.maxBitrate` is never present in any flow projection ===
AWS: Smithy `Source.Transport` -> `Transport.MaxBitrate` (jsonName `maxBitrate`, Integer, "The smoothing max bitrate (in bps) for RIST, RTP, and RTP-FEC streams"); the input member is `SetSourceRequest.MaxBitrate` (jsonName `maxBitrate`). Recorded behavior: for an rtp source AWS always reports a populated `maxBitrate` on `flow.source.transport` and on every entry of `flow.sources[*].transport` (the recordings show 160000000), on the CreateFlow 201 body, on the DescribeFlow 200 body and on the UpdateFlow 202 body.
Emulator: the member is absent (null) from every one of those bodies. Either the request's `maxBitrate` is lost on the way in or the service-assigned value is never filled; `materialise_source` in `src/ops/create_flow.rs` builds `Transport { max_bitrate: request.max_bitrate, .. }` with no fallback, so a request that does not carry one stores none and nothing ever assigns the service default.
Why: a client that reads the configured smoothing bitrate back off a flow (bitrate-budget checks, round-trip config diffing, Terraform/CDK-style drift detection) sees an absent member and either errors or infers the source is unconfigured, although the flow was created with a bitrate.
Fix: the source projection and CreateFlow's source materialisation — carry `MaxBitrate` through to the stored transport and assign the service's default when the request omits it.
Traces: mediaconnect-flow-lifecycle-basic#task#2, mediaconnect-flow-list-pagination#setup#1, mediaconnect-flow-media-streams-and-outputs#task#4, mediaconnect-flow-source-metadata-and-thumbnail#task#5, mediaconnect-flow-sources-entitlements-update#task#3, mediaconnect-flow-sources-entitlements-update#task#6, mediaconnect-flow-sources-entitlements-update#task#7.

=== Cause 2 of 4: `routerIntegrationState` is never projected on sources or on outputs ===
AWS: Smithy `Source.RouterIntegrationState` (jsonName `routerIntegrationState`, enum `State`, "Indicates if router integration is enabled or disabled on the flow source") and `Output.RouterIntegrationState` (jsonName `routerIntegrationState`, enum `State`); the input member is `SetSourceRequest.RouterIntegrationState`. Recorded behavior: AWS reports this member on every source and every output even when the request never mentions router integration, with the disabled value (recordings show `DISABLED`), on CreateFlow 201, AddFlowOutputs 201 and DescribeFlow 200 bodies.
Emulator: the member is absent (null) on `flow.source`, on every `flow.sources[*]`, on `flow.outputs[*]` and on the `outputs[*]` of the AddFlowOutputs response. The stored source record built by `materialise_source` (`src/ops/create_flow.rs`) has no router-integration field at all, so there is nothing to project.
Why: the member is the only way a client can tell whether a source/output is wired to the router plane. Absent, SDK code that branches on `RouterIntegrationState == 'DISABLED'` takes the unknown/None path, and a caller cannot distinguish "router integration is off" from "this service does not report it".
Fix: add the router-integration state to the stored source and output records and project it with its disabled default in the source/output projection used by the flow ops.
Traces: mediaconnect-flow-lifecycle-basic#task#2, mediaconnect-flow-list-pagination#setup#1, mediaconnect-flow-media-streams-and-outputs#task#3 (the AddFlowOutputs 201 body: only `outputs[0].routerIntegrationState` diverges), mediaconnect-flow-media-streams-and-outputs#task#4, mediaconnect-flow-source-metadata-and-thumbnail#task#5, mediaconnect-flow-sources-entitlements-update#task#3.

=== Cause 3 of 4: an empty `mediaStreams` list is omitted instead of serialized as [] ===
AWS: Smithy `Flow.MediaStreams` (jsonName `mediaStreams`, list of MediaStream, not sparse). Recorded behavior: a flow created with no media streams still carries `"mediaStreams": []` in the CreateFlow 201, DescribeFlow 200 and UpdateFlow 202 bodies — the empty list is present, not dropped.
Emulator: the key is missing entirely from the body (replay reports `$.flow.mediaStreams: missing (expected [], got None)`).
Why: a client that iterates `flow['mediaStreams']` without a default raises KeyError, and code that distinguishes "no media streams" from "media streams not reported" cannot. This is a general empty-collection serialization rule, not one flow's value.
Fix: the flow serializer must emit present-but-empty lists for the flow's collection members rather than omitting them.
Traces: mediaconnect-flow-lifecycle-basic#task#2, mediaconnect-flow-list-pagination#setup#1, mediaconnect-flow-source-metadata-and-thumbnail#task#5, mediaconnect-flow-sources-entitlements-update#task#3, mediaconnect-flow-sources-entitlements-update#task#6, mediaconnect-flow-sources-entitlements-update#task#7.

=== Cause 4 of 4: UpdateFlow accepts `SourceFailoverConfig.RecoveryWindow` but the value is lost ===
AWS: Smithy UpdateFlow (PUT /v1/flows/{FlowArn}, success 202) takes `SourceFailoverConfig` targeting `UpdateFailoverConfig`, whose `RecoveryWindow` member (jsonName `recoveryWindow`, Integer) is merged into the flow's `FailoverConfig.RecoveryWindow` (jsonName `recoveryWindow`). Recorded behavior: after an UpdateFlow that sets a recovery window, the 202 body and every later DescribeFlow 200 body report `flow.sourceFailoverConfig.recoveryWindow` with the value that was set (the recordings show 200).
Emulator: the member is absent from both the UpdateFlow 202 body and the following DescribeFlow 200 body, while the sibling failover members of the same object round-trip (only `recoveryWindow` is flagged). `merged_failover_config` in `src/ops/update_flow.rs` does copy `update.recovery_window` when it is `Some`, so the value is being dropped either deserializing `UpdateFailoverConfig` or projecting `FailoverConfig` — Hypothesis: the member never arrives as `Some` because its wire key is not bound.
Why: failover tuning silently does not stick. A client sets the dash-7 recovery window, gets a 202, reads the flow back and sees no window at all, so idempotent apply loops rewrite it forever and operators believe failover is unconfigured.
Fix: the UpdateFlow failover merge path plus the `FailoverConfig` projection (`src/ops/update_flow.rs` merged_failover_config and the shared flow projection) — bind and persist `recoveryWindow` and project it on every flow read.
Traces: mediaconnect-flow-sources-entitlements-update#task#6 (UpdateFlow 202), mediaconnect-flow-sources-entitlements-update#task#7 (DescribeFlow 200).

Notes on members NOT claimed here: the `$.flow.sources[1]: missing` facet in mediaconnect-flow-sources-entitlements-update#task#3/#task#6/#task#7 is downstream of AddFlowSources still being a scaffolded 501 stub (`src/ops/add_flow_sources.rs` carries the SCAFFOLDED marker) — that stub's issue is filed for us by the replay report, so no cause is opened for it here. The `$.flow.description` facet of mediaconnect-flow-lifecycle-basic#task#2 is claimed by the companion flow-description issue filed in this same sweep.

## Evidence
Replay of trace set simparity-wire-aws-v1: 16/50 steps passed (32%), 18 divergence clusters, all service=mediaconnect, bootstrap_signal=false, state_reset_failures=0. None of the clusters below carried a stub_path or an owned_by, so none is already filed.

Cause 1+2+3 — mediaconnect-flow-lifecycle-basic#task#2 (DescribeFlow, expected 200, actual 200):
  $.flow.source.transport.maxBitrate: missing (expected 160000000, got None)
  $.flow.source.routerIntegrationState: missing (expected 'DISABLED', got None)
  $.flow.mediaStreams: missing (expected [], got None)
  $.flow.sources[0].transport.maxBitrate: missing (expected 160000000, got None)
  $.flow.sources[0].routerIntegrationState: missing (expected 'DISABLED', got None)
  (same cluster also covers mediaconnect-flow-lifecycle-basic#task#5 and #task#7)

Cause 1+2+3 — mediaconnect-flow-list-pagination#setup#1 (CreateFlow, expected 201, actual 201): identical set of missing members on the created flow (cluster also covers mediaconnect-flow-sources-entitlements-update#setup#1 and mediaconnect-flow-vpc-interfaces#setup#1).

Cause 2 — mediaconnect-flow-media-streams-and-outputs#task#3 (AddFlowOutputs, expected 201, actual 201):
  $.outputs[0].routerIntegrationState: missing (expected 'DISABLED', got None)

Cause 1+2 — mediaconnect-flow-media-streams-and-outputs#task#4 (DescribeFlow, 200/200): source maxBitrate + source/sources[0]/outputs[0] routerIntegrationState all missing.

Cause 1+2+3 — mediaconnect-flow-source-metadata-and-thumbnail#task#5 (DescribeFlow, 200/200), cluster also covers #task#7.

Cause 1+2+3 — mediaconnect-flow-sources-entitlements-update#task#3 (DescribeFlow, 200/200), cluster also covers #task#5.

Cause 4 — mediaconnect-flow-sources-entitlements-update#task#6 (UpdateFlow, expected 202, actual 202):
  $.flow.sourceFailoverConfig.recoveryWindow: missing (expected 200, got None)
Cause 4 — mediaconnect-flow-sources-entitlements-update#task#7 (DescribeFlow, 200/200): same member still missing after the update.

Status codes and error codes match on all eight clusters; the divergence is purely missing response members.

## Steps to Reproduce
Boot the emulator and replay the wire corpus (replay_wire_corpus), or drive a botocore mediaconnect client at the emulator endpoint:

Cause 1+2+3: mc.create_flow(Name='f1', Source={'Name':'f1-src','Protocol':'rtp','WhitelistCidr':'10.24.43.0/24','MaxBitrate':160000000,'IngestPort':5090}) -> inspect the 201 body: `flow.source.transport.maxBitrate` and `flow.source.routerIntegrationState` are absent and `flow.mediaStreams` is missing rather than []. Then mc.describe_flow(FlowArn=...) -> same three gaps on `flow.source`, `flow.sources[0]` and the flow itself.

Cause 2 (outputs): mc.add_flow_outputs(FlowArn=..., Outputs=[{'Name':'o1','Protocol':'rtp','Destination':'198.51.100.9','Port':5100}]) -> the 201 body's `outputs[0].routerIntegrationState` is absent; DescribeFlow afterwards shows the same on `flow.outputs[0]`.

Cause 4: mc.update_flow(FlowArn=..., SourceFailoverConfig={'FailoverMode':'MERGE','State':'ENABLED','RecoveryWindow':200}) -> the 202 body's `flow.sourceFailoverConfig` has no `recoveryWindow`; mc.describe_flow(FlowArn=...) afterwards still has none.

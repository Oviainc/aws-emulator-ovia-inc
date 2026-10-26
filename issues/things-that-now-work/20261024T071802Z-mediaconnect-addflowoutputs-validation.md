Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# Media stream and VPC interface references are stored without checking the flow actually holds them

_Filed: 2026-10-24T07:18:02Z_
_Operation: AddFlowOutputs | Class: validation_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An output's (and a source's) media stream association is a reference BY NAME to a media stream of the same flow: `MediaStreamOutputConfigurationRequest$MediaStreamName` is "the name of the media stream that is associated with the output", and `MediaStreamSourceConfigurationRequest$MediaStreamName` the source equivalent. The same is true of `VpcInterfaceAttachment$VpcInterfaceName` ("the name of the VPC interface to use for this resource") and `SetSourceRequest$VpcInterfaceName`.

Expected: a request that associates a media stream the flow does not hold (or attaches a VPC interface the flow does not have) is a semantic error — HTTP 400 BadRequestException naming the unknown media stream / VPC interface — and nothing is stored.

Actual: the association list is materialised and stored verbatim with no lookup against the flow's `MediaStreams`/`VpcInterfaces`. `POST /v1/flows/{FlowArn}/outputs` with `mediaStreamOutputConfigurations:[{mediaStreamName:"does-not-exist", ...}]` answers 201 and DescribeFlow then reports an output permanently associated with a media stream that does not exist; the flow is left in a state the real service cannot produce. The same unchecked path is reached through CreateFlow (`outputs`/`source`/`sources`), AddFlowSources, UpdateFlowOutput and UpdateFlowSource, which all materialise these request shapes through the same `crate::state` helpers, so one fix covers every entry point.

Note the asymmetry inside the crate: RemoveFlowMediaStream DOES resolve associations (it refuses to remove a stream something still carries, via `StoredFlow::media_stream_association`), so the store knows about the relationship — only the write side never validates it.

## Evidence

Spec: `smithy_query(query="shape",shape="MediaStreamOutputConfigurationRequest")` — required member `MediaStreamName` = "The name of the media stream that is associated with the output"; `MediaStreamSourceConfigurationRequest$MediaStreamName`; `VpcInterfaceAttachment$VpcInterfaceName`. A media stream exists on a flow only once AddFlowMediaStreams/CreateFlow created it (`AddFlowMediaStreams` doc: "Adds media streams to an existing flow"), so a name no media stream of the flow carries references nothing.

Handler: `src/ops/add_flow_outputs.rs:290-307` materialises each `AddOutputRequest` through `StoredOutput::from_add_request` (`src/state.rs:1090`), which calls `materialise_media_stream_output_configurations` (`src/state.rs:1185`) without consulting `flow.media_streams`; the flow is only read for the 50-output ceiling, the NDI size rule and output-name uniqueness. The same holds for `src/ops/add_flow_sources.rs:267-271`, `src/ops/create_flow.rs:393-429`, `src/ops/update_flow_output.rs:432-440` and `src/ops/update_flow_source.rs:503-511`. The store can resolve the relationship — `StoredFlow::media_stream_association` (`src/state.rs:1488`) is used by `src/ops/remove_flow_media_stream.rs:223` — so the data needed for the check is already there.

Test coverage: none pins the current acceptance. `tests/all/flow_outputs.rs:525-600` and `tests/all/update_flow_output.rs:195-241` only ever associate media streams the flow really holds (`ms-1`/`ms-2` created with the flow); no test sends an unknown media stream name or an unknown vpcInterfaceName.

## Steps to Reproduce

1. POST /v1/flows with {"name":"f-ms","source":{"name":"src","protocol":"rtp","whitelistCidr":"10.0.0.0/16"}} (no mediaStreams) — note the flowArn
2. POST /v1/flows/{FlowArn}/outputs with {"outputs":[{"name":"o1","protocol":"st2110-jpegxs","port":7100,"mediaStreamOutputConfigurations":[{"mediaStreamName":"does-not-exist","encodingName":"jxsv","destinationConfigurations":[{"destinationIp":"198.51.100.60","destinationPort":8000,"interface":{"name":"nic"}}]}]}]} — observe HTTP 201 where AWS answers 400 BadRequestException for the unknown media stream
3. GET /v1/flows/{FlowArn} — the output is reported with an association to a media stream the flow does not have
4. Repeat with {"vpcInterfaceAttachment":{"vpcInterfaceName":"no-such-vpc"}} on the same add, and with the same payloads through CreateFlow / UpdateFlowOutput / AddFlowSources — all accepted

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 825b0e930e0bc7d005dad99ff95af4e6881702a4

# An NDI flow output is stored without the three NDI defaults the model documents (NdiOutputTimecodeSource, NdiAudioReferenceLevel, NdiProgramName)

_Filed: 2026-10-28T20:37:11Z_
_Operation: AddFlowOutputs | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `825b0e930e0bc7d005dad99ff95af4e6881702a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StoredOutput::from_add_request` — the one helper AddFlowOutputs (and CreateFlow's `Outputs`) materialises a flow output through — copies the three NDI transport members straight off the request and stores `None` when the request omits them. The model documents a concrete value the service uses for each one when the request names none, and for two of them it says the field is ALWAYS PRESENT on an NDI output.

So an NDI output added with only `{"name": ..., "protocol": "ndi-speed-hq"}` is answered (and later reported by DescribeFlow) with `transport.ndiOutputTimecodeSource`, `transport.ndiAudioReferenceLevel` and `transport.ndiProgramName` all absent, where the model says they are `EMBEDDED_TIMECODE`, `-20` and the output's own name.

This is also inconsistent with the crate's own handling of every other documented default: the SAME function two lines above defaults `OutputStatus` to `ENABLED`, `create_router_output.rs` materialises the documented `RecoveryLatencyMode` `BALANCED` for a request that carries no `FabricConfiguration`, and `crate::state::default_clock_rate` materialises a media stream's documented rate. The crate also already knows which outputs are NDI outputs — `crate::state::output_request_uses_ndi`, which this very handler calls for the flow-size rule — so the condition the model attaches the defaults to is already computed on this path.

The same omission reaches the stored record through UpdateFlowOutput when a request moves an output onto the NDI protocol without naming these members (`merged_transport` only assigns what the request carries), so the fix belongs in the shared materialisation rather than in one handler.

## Evidence

Model, the request shape this handler reads (smithy_query(query="doc",shape="AddOutputRequest",member="NdiOutputTimecodeSource")):
  "Controls how MediaConnect generates timecodes for NDI output frames. If you don't specify this field, MediaConnect uses EMBEDDED_TIMECODE."
smithy_query(query="doc",shape="AddOutputRequest",member="NdiAudioReferenceLevel"):
  "The audio reference level of the NDI receiver, in decibels relative to full scale (dBFS). ... If you don't specify this field, MediaConnect uses -20."
smithy_query(query="doc",shape="AddOutputRequest",member="NdiProgramName"):
  "A suffix for the name of the NDI® sender that the flow creates. If a custom name isn't specified, MediaConnect uses the output name."
And the reported shape, smithy_query(query="doc",shape="Transport",member="NdiOutputTimecodeSource") / member="NdiAudioReferenceLevel":
  "For NDI outputs, this field is always present and defaults to EMBEDDED_TIMECODE."
  "For NDI outputs, this field is always present and defaults to -20."

The code that stores them verbatim — services/service-com-amazonaws-mediaconnect-mediaconnect/src/state.rs:1728-1742:
```
            transport: request.protocol.clone().map(|protocol| Transport {
                protocol,
...
                ndi_speed_hq_quality: request.ndi_speed_hq_quality,
                ndi_program_name: request.ndi_program_name.clone(),
                ndi_output_timecode_source: request.ndi_output_timecode_source.clone(),
                ndi_audio_reference_level: request.ndi_audio_reference_level,
                ..Transport::default()
            }),
```
The same function DOES materialise the documented default for the member next to them — src/state.rs:1705-1710:
```
            output_status: Some(
                request
                    .output_status
                    .clone()
                    .unwrap_or_else(|| STATUS_ENABLED.to_string()),
            ),
```
and the sibling create does it for its own documented default — src/ops/create_router_output.rs:354-361:
```
                // `RecoveryLatencyMode` documents `BALANCED` as its default, so an output
                // created without a `FabricConfiguration` reports that mode.
                fabric_configuration: input.fabric_configuration.clone().unwrap_or(
```
The NDI condition is already computed on this path — src/ops/add_flow_outputs.rs:242-257 calls `crate::state::output_request_uses_ned`… i.e. `crate::state::output_request_uses_ndi(output)` (src/state.rs:1067: "Whether an `AddOutputRequest` describes an NDI® output: its protocol is the NDI transport").
UpdateFlowOutput assigns only what the request carries — src/ops/update_flow_output.rs:242-247.

Test coverage: no test pins the omitted defaults — every NDI test states the members explicitly (tests/all/update_flow_output.rs:133-143 sends and asserts `ndiProgramName`/`ndiOutputTimecodeSource`/`ndiAudioReferenceLevel`; tests/all/flow_ndi_sizing.rs:86 and :200 always send `"ndiProgramName": "programme"`).

## Steps to Reproduce

1. POST /v1/flows with {"name":"ndi-flow","flowSize":"LARGE","ndiConfig":{"ndiState":"ENABLED"}, ...} — 201; keep FlowArn.
2. POST /v1/flows/{FlowArn}/outputs with {"outputs":[{"name":"ndi-out","protocol":"ndi-speed-hq"}]} — 201.
3. Observe the response output's `transport`: `ndiOutputTimecodeSource`, `ndiAudioReferenceLevel` and `ndiProgramName` are all absent, where the model documents EMBEDDED_TIMECODE, -20 and the output name ("this field is always present").
4. GET /v1/flows/{FlowArn} — the stored output reports the same three members absent, so no later read recovers them.

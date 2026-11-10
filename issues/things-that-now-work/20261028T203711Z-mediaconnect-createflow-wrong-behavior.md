Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 825b0e930e0bc7d005dad99ff95af4e6881702a4

# A flow's NdiConfig is stored with MachineName absent instead of the service-generated 12-character prefix

_Filed: 2026-10-28T20:37:11Z_
_Operation: CreateFlow | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `825b0e930e0bc7d005dad99ff95af4e6881702a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateFlow stores the request's `NdiConfig` verbatim (`ndi_config: input.ndi_config.clone()`), and the flow projection reports it back verbatim. The model states that when a request names no `MachineName`, MediaConnect GENERATES one ("a unique 12-character ID as the prefix"), so this member is a service-assigned value, not a pure echo: a flow created with `{"ndiConfig":{"ndiState":"ENABLED"}}` is reported by CreateFlow and every later DescribeFlow with `ndiConfig.machineName` absent, where the service assigns a value.

Nothing anywhere in the crate mentions `machine_name` other than the struct field (grep for `machine_name` finds only `src/ops/types/shared_types.rs:484`), so no path mints it and no comment records the omission. That is the opposite of how the crate treats every other service-assigned member of a flow: a source with no requested name is named by `crate::state::resolve_source_name`, an output by `resolve_output_name`, a media stream's payload type by `next_media_stream_fmt`, and an ingest address by `assigned_ingest_ip` — and a previously fixed issue in this crate was precisely about the flow projection omitting service-assigned members.

UpdateFlow stores the replacement `NdiConfig` the same way (src/ops/update_flow.rs:294-297, :387), so the same record stays without the generated prefix after an update.

## Evidence

Model (smithy_query(query="shape",shape="NdiConfig"), member MachineName):
  "A prefix for the names of the NDI sources that the flow creates. If a custom name isn't specified, MediaConnect generates a unique 12-character ID as the prefix."

The handler stores the request's object as it stands — services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/create_flow.rs:637:
```
            ndi_config: input.ndi_config.clone(),
```
and the projection reports it as stored — src/state.rs:2612:
```
            ndi_config: self.ndi_config.clone(),
```
No path assigns the prefix: grep for `machine_name` across the crate's `src/` yields only the field declaration, src/ops/types/shared_types.rs:484:
```
    pub machine_name: Option<String>,
```
Contrast the crate's handling of other service-assigned members of the same record — src/state.rs:6316-6335 (`resolve_source_name`: "`SetSourceRequest$Name` ... is OPTIONAL, while `Source$Name` is REQUIRED — so a source whose request named none is named by the service") and src/state.rs:1939-1948 (`next_media_stream_fmt`: "The `Fmt` the service assigns the next media stream of a flow").

Test coverage: every NDI test supplies `machineName` explicitly, so none pins the absent value — tests/all/flow_ndi_sizing.rs:119 `"ndiConfig": { "ndiState": "ENABLED", "machineName": "studio-a" }`, tests/all/update_flow.rs:247, tests/all/flow_source_metadata.rs:199.

## Steps to Reproduce

1. POST /v1/flows with {"name":"ndi-flow","flowSize":"LARGE","ndiConfig":{"ndiState":"ENABLED"},"source":{...}} — 201.
2. Observe the response `flow.ndiConfig`: it carries only `ndiState`; `machineName` is absent, where the model documents a generated unique 12-character ID.
3. GET /v1/flows/{FlowArn} — `ndiConfig.machineName` is still absent, so no read reports the service-assigned prefix.
4. PUT /v1/flows/{FlowArn} with {"ndiConfig":{"ndiState":"ENABLED"}} — same: the replacement config is stored without a generated prefix.

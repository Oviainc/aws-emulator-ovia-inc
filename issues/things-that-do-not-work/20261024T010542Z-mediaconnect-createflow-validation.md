Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# CreateFlow accepts more than two Sources and accepts NdiState ENABLED on a non-LARGE flow

_Filed: 2026-10-24T01:05:42Z_
_Operation: CreateFlow | Class: validation_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two spec-required refusals are missing from CreateFlow, both of which the emulator already enforces on the corresponding update path:

1. Source count / failover pairing. `FailoverConfig$State`: "If the state is inactive, the flow can have only one source. If the state is active, the flow can have one or two sources." A create request carrying three or more `Sources`, or carrying two `Sources` with no active `SourceFailoverConfig`, describes a flow AWS will not create and is refused with BadRequestException (400). The emulator accepts up to 50 `Sources` entries and never looks at `SourceFailoverConfig` while doing so, so it mints flows that cannot exist in AWS and then reports them on every DescribeFlow/ListFlows.

2. NDI needs the larger flow. `CreateFlowRequest$FlowSize`: "Determines the processing capacity and feature set of the flow. Set this optional parameter to LARGE if you want to enable NDI sources or outputs on the flow", and `NdiConfig` is "Required when the flow includes an NDI source or output". A create with `ndiConfig.ndiState = ENABLED` (or an `ndi-speed-hq` source) and `flowSize` MEDIUM or unset is refused with BadRequestException. The emulator stores it and answers 201.

Expected: 400 BadRequestException with nothing stored, in both cases.
Actual: 201 with the flow persisted.

## Evidence

Spec: `smithy_query(query="doc",shape="FailoverConfig",member="State")` (one source when inactive, one or two when active); `smithy_query(query="doc",shape="CreateFlowRequest",member="FlowSize")` ("Set this optional parameter to LARGE if you want to enable NDI sources or outputs on the flow"), quoted in `src/ops/create_flow.rs:59-61`; UpdateFlow's own documentation ("if you have a MEDIUM flow and you want to change the flow source to NDI: first, use the UpdateFlow operation to upgrade the flow size to LARGE").

Handler: `src/ops/create_flow.rs:244-273` collects `Sources`/`Source` and checks only the generic per-request bound `MAX_FLOW_COLLECTION_ENTRIES` (50) per collection; `src/ops/create_flow.rs:440-446` stores `source_failover_config`, `ndi_config` and `flow_size` verbatim with no cross-member check. No NDI/flow-size or source-count/failover validation exists anywhere in the handler.

Contrast in the same crate: `src/ops/update_flow.rs:388-424` refuses `NdiState ENABLED` unless the resulting flow size is in `NDI_CAPABLE_FLOW_SIZES`, and `src/ops/update_flow.rs:426-438` refuses disabling failover on a multi-source flow — both citing the model sentences above. CreateFlow can therefore create exactly the state UpdateFlow refuses to produce.

Test coverage: none for either rule. `tests/all/update_flow.rs:457-466,607-662` covers the NDI/flow-size refusal only for UpdateFlow; `tests/all/flow_sources.rs:243-258` creates a two-source flow with no failover config and expects 201, and no create-side refusal table row (`tests/all/flow_lifecycle.rs`, `tests/all/error_envelopes.rs`) covers source count, failover pairing, or NDI sizing.

## Steps to Reproduce

1. POST /v1/flows with {"name":"f-ndi","flowSize":"MEDIUM","ndiConfig":{"ndiState":"ENABLED","machineName":"studio-a"},"source":{"protocol":"rtp","whitelistCidr":"10.0.0.0/16"}} -> observe 201 with ndiConfig stored, where AWS returns 400 BadRequestException (NDI needs a LARGE flow)
2. PUT /v1/flows/{FlowArn} with {"ndiConfig":{"ndiState":"ENABLED"}} on a MEDIUM flow -> observe the emulator's own 400, showing create and update disagree about the same rule
3. POST /v1/flows with {"name":"f-3src","sources":[{"name":"a","protocol":"rtp","whitelistCidr":"10.1.0.0/16"},{"name":"b","protocol":"rtp","whitelistCidr":"10.2.0.0/16"},{"name":"c","protocol":"rtp","whitelistCidr":"10.3.0.0/16"}]} (no sourceFailoverConfig) -> observe 201 with three sources, where AWS returns 400 BadRequestException
4. GET /v1/flows/{FlowArn} and observe the three-source, failover-less flow reported back

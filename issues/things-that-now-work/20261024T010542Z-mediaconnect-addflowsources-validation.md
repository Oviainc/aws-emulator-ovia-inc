Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# AddFlowSources lets a flow hold more than two sources (and a second source with failover inactive)

_Filed: 2026-10-24T01:05:42Z_
_Operation: AddFlowSources | Class: validation_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A MediaConnect flow can hold at most two sources, and at most ONE while source failover is inactive: `FailoverConfig$State` is documented as "If the state is inactive, the flow can have only one source. If the state is active, the flow can have one or two sources." AWS therefore refuses an AddFlowSources call that would give a flow a second source when its `SourceFailoverConfig` is absent/inactive, and refuses any call that would push a flow past two sources, with BadRequestException (HTTP 400), leaving the flow unchanged.

Expected: AddFlowSources on a flow with one source and no SourceFailoverConfig -> 400 BadRequestException, flow still has one source. AddFlowSources on a flow that already has two sources -> 400 BadRequestException.
Actual: both calls answer 201 and append the sources; the flow's only ceiling is the emulator's generic per-request collection bound of 50 (`crate::state::MAX_FLOW_SOURCES = MAX_FLOW_COLLECTION_ENTRIES`), so a client can build a 50-source flow that AWS could never produce, and every later DescribeFlow/ListFlows reports that impossible record.

The emulator already encodes one half of this invariant elsewhere: UpdateFlow refuses `SourceFailoverConfig.State = DISABLED` when the flow has more than one source (`src/ops/update_flow.rs:426-438`), citing the same model sentence. The add path never applies the rule in the other direction.

## Evidence

Spec: `smithy_query(query="doc",shape="FailoverConfig",member="State")` — "The state of source failover on the flow. If the state is inactive, the flow can have only one source. If the state is active, the flow can have one or two sources." (The same sentence is quoted by `src/ops/update_flow.rs:426-428` to justify UpdateFlow's own refusal.)

Handler: `src/ops/add_flow_sources.rs:209-220` is the ONLY count check — `if held + input.sources.len() > crate::state::MAX_FLOW_SOURCES` — and `src/state.rs:434-444` defines `MAX_FLOW_SOURCES = MAX_FLOW_COLLECTION_ENTRIES` (50) with the comment "AWS publishes no count for a flow's sources". The handler never reads `flow.source_failover_config`, so neither the two-source ceiling nor the failover-inactive single-source rule is enforced.

Test coverage: no test asserts the rule; the suite asserts the permissive behavior instead — `tests/all/flow_sources.rs:160-260` adds a third source to a two-source flow that carries no SourceFailoverConfig and expects 201, and `tests/all/flow_sources.rs:600-645` grows one failover-less flow to five sources expecting 201 on every call.

## Steps to Reproduce

1. POST /v1/flows with {"name":"f1","source":{"name":"primary","protocol":"rtp","whitelistCidr":"10.1.0.0/16"}} (no sourceFailoverConfig) and note the flowArn
2. POST /v1/flows/{FlowArn}/source with {"sources":[{"name":"backup","protocol":"rtp","whitelistCidr":"10.2.0.0/16"}]}
3. Observe 201 and two sources on the flow, where AWS refuses with 400 BadRequestException because failover is inactive
4. Repeat the same POST three more times with distinct names and observe the flow accumulating 5 sources; DescribeFlow reports all of them, where AWS caps a flow at two sources

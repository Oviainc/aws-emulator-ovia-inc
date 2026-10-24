Issue-Severity: MEDIUM
Issue-Gap-Class: missing
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# UpdateProfileWithAgent has no handler and is answered by the unroutable-operation path

_Filed: 2026-10-24T00:03:07Z_
_Operation: UpdateProfileWithAgent | Class: missing_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateProfileWithAgent` is BOUND to the `com.amazonaws.healthlake#HealthLake` service (it appears in the service's operation list) but the crate has no `src/ops/update_profile_with_agent.rs`: the scaffold generator declined it because of its `@streaming` event-stream output, and `src/service.rs` states it 'is answered by the framework's unroutable-operation path'. Expected: an X-Amz-Target of `HealthLake.UpdateProfileWithAgent` reaches a handler that validates `ProfileId`/`InputMessage`, resolves the stored profile (404 for an unknown one) and returns an `AgentOutputMessageStream` event stream. Actual: no handler exists, so the request is refused as an unknown operation.

## Evidence

Spec: `smithy_query(query="list_operations")` includes UpdateProfileWithAgent; `smithy_query(query="operation")` gives input ProfileId/InputMessage/ConversationId and required streaming output member `AgentResponse`. Code: `src/ops/SKIPPED_STUBS.md` records 'SKIPPED: UpdateProfileWithAgent — @streaming event-stream output member'; `src/service.rs` (input_validation_gates, lines ~234-268) lists every other modelled operation and notes this one has no module. Tests: no test exercises it.

## Steps to Reproduce

1. CreateDataTransformationProfile and note the ProfileId
2. POST / with X-Amz-Target: HealthLake.UpdateProfileWithAgent and body {"ProfileId":"<ProfileId>","InputMessage":{...}}
3. Observe the framework's unknown-operation refusal rather than an agent response stream

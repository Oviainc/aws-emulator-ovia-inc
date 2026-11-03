Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.agent-runtime-endpoint-pinned-version-survives-runtime-update bedrock-agentcore-control.delete-agent-runtime-while-endpoint-in-service-blocked bedrock-agentcore-control.update-agent-runtime-endpoint-nonexistent-version-rejected
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|GetAgentRuntimeEndpoint|ResourceNotFoundException
Issue-Key: bedrock-agentcore-control.delete-agent-runtime-while-endpoint-in-service-blocked|_error_detail#14|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: bedrock-agentcore-control.update-agent-runtime-endpoint-nonexistent-version-rejected|run#16|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Commit: 693e56bfaa4159f6333ea367977c7471ac0df2f0

# Agent runtime endpoint gaps: no auto-created DEFAULT endpoint, DeleteAgentRuntime accepted with a live endpoint, UpdateAgentRuntimeEndpoint answers 501 (3 causes)

> _Note: this issue was identified against the code base at commit `693e56bfaa4159f6333ea367977c7471ac0df2f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAgentRuntimeEndpoint

## Description
Three distinct, net-new root causes in the bedrock-agentcore-control agent-runtime/endpoint surface. All three are grounded in failing behavioral checks of this service and none of them is covered by an open issue: the closest neighbours are 20261029T212239Z-bedrock-agentcore-control-accepts-four-requests-aws-refuses.md (which enumerates gateway-delete-with-targets, CreateMemory on a missing Kinesis stream, unauthorized caller, and UpdateAgentRuntime state transition — NOT DeleteAgentRuntime-with-endpoint), 20261102T114238Z-...-updateagentruntime-s-resourcenotfoundexception... and 20261101T213737Z-...-createagentruntime-creates-no-vended-cloudwatch-logs-group... (a Logs-vending gap, not the endpoint model). A fixer must close all three causes below before submitting.

=== Cause 1 of 3: a new AgentRuntime has no DEFAULT endpoint, so GetAgentRuntimeEndpoint(endpointName="DEFAULT") answers 404 ===
AWS: every AgentCore Runtime is created with a service-managed endpoint named DEFAULT that exists from the moment CreateAgentRuntime returns and automatically tracks the runtime's newest published version (its targetVersion/liveVersion advance when a new version is published, without any CreateAgentRuntimeEndpoint call). Smithy: GetAgentRuntimeEndpoint GET /runtimes/{agentRuntimeId}/runtime-endpoints/{endpointName}/ -> 200 with required status/name/id/agentRuntimeArn/agentRuntimeEndpointArn/createdAt/lastUpdatedAt plus optional liveVersion/targetVersion; EndpointName pattern ^[a-zA-Z][a-zA-Z0-9_]{0,47}$ admits "DEFAULT"; ResourceNotFoundException (HTTP 404) is modeled only for an endpoint that does not exist. The model does not itself state that DEFAULT is auto-created — recorded behavior only: the recording reads DEFAULT on a freshly created runtime and observes it already at the runtime's version 1, then advancing to the new version after a publish.
Emulator: CreateAgentRuntime stores the runtime with an empty endpoint map (services/.../src/ops/create_agent_runtime.rs:329-331 — "A runtime is created with no endpoints"), so the first read of the DEFAULT endpoint fails with 404 ResourceNotFoundException and message "Agent endpoint with name DEFAULT not found." (fixed text "Agent endpoint with name " + <endpointName from the request> + " not found."). No DEFAULT endpoint ever appears, and nothing auto-advances on publish.
Why: a client that invokes a runtime through its DEFAULT endpoint — the documented default path for invocation and for reading which version is live — cannot resolve the endpoint at all, and version-promotion logic built on DEFAULT's auto-advance sees a permanent not-found instead of a tracked version.
Fix: CreateAgentRuntime's handler/state must seed the DEFAULT endpoint record (status READY once the runtime settles, targetVersion/liveVersion = the runtime's latest published version) and the version-publish path (UpdateAgentRuntime / CreateAgentRuntimeVersion) must advance it; GetAgentRuntimeEndpoint then serves it like any named endpoint. Citations: services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/create_agent_runtime.rs:321-334, services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/get_agent_runtime_endpoint.rs, and the runtime record's `endpoints` field in src/state.rs.
Traces: bedrock-agentcore-control.agent-runtime-endpoint-pinned-version-survives-runtime-update (error: ResourceNotFoundException on GetAgentRuntimeEndpoint for DEFAULT).

=== Cause 2 of 3: DeleteAgentRuntime is accepted while a READY endpoint still points at the runtime ===
AWS: DeleteAgentRuntime is refused with ConflictException (HTTP 409, modeled on the operation) while the runtime still has endpoints; recorded message, which is entirely fixed text with no request- or state-derived parts: "This agent has endpoints that must be deleted before the agent can be removed." The runtime is left untouched (a subsequent GetAgentRuntime does not report DELETING). Smithy: DeleteAgentRuntime DELETE /runtimes/{agentRuntimeId}/ success 202, errors include ConflictException (409) — "thrown when there is a conflict performing an operation". A runtime with no endpoints is accepted with 202 and status DELETING (that half passes).
Emulator: the call succeeds — HTTP 202 with {"status": "DELETING", "agentRuntimeId": ..., "agentRuntimeVersion": "1"} — and the runtime's versions are cleared while a READY endpoint is still attached. services/.../src/ops/delete_agent_runtime.rs:183-219 looks the runtime up and deletes it with no check of `record.endpoints`.
Why: a client that relies on the conflict as a guard destroys a runtime underneath a live endpoint; the endpoint is left dangling and the error path that AWS uses to tell the caller to tear endpoints down first never fires, so cleanup ordering bugs pass silently.
Fix: DeleteAgentRuntime's handler must check the runtime record's attached endpoints before mutating state and return the modeled ConflictException with AWS's sentence; only an endpoint-free runtime proceeds to 202/DELETING. Citation: services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/delete_agent_runtime.rs:175-219.
Traces: bedrock-agentcore-control.delete-agent-runtime-while-endpoint-in-service-blocked (mismatch: "expected the call to be rejected, but it succeeded").

=== Cause 3 of 3: UpdateAgentRuntimeEndpoint answers 501 NotImplemented instead of the modeled refusal for a never-created version ===
AWS: UpdateAgentRuntimeEndpoint (PUT /runtimes/{agentRuntimeId}/runtime-endpoints/{endpointName}/, success 202) refuses a retarget to a version that was never created for that runtime with ResourceNotFoundException (HTTP 404, modeled on the operation); recorded message template "Agent with agentId: <agentRuntimeId> not found" — fixed text "Agent with agentId: " and " not found" around the agentRuntimeId taken from the request path. The effect is absent: the endpoint still reports its previous targetVersion. Smithy: agentRuntimeVersion is optional, length 1..5, pattern ^([1-9][0-9]{0,4})$ (so "88888" is syntactically valid and must be refused on existence, not on syntax).
Emulator: the request is answered with error code "NotImplemented" (the scaffold's 501 channel) — the operation's handler is still an unimplemented scaffold stub, so no modeled error code, no message and no state check exist. A separate stub issue was filed for UpdateAgentRuntimeEndpoint's bare implementation; this cause records the refusal semantics that implementation must carry, which the stub issue does not state.
Why: a client probing a bad promotion target gets an unmodeled NotImplemented it cannot classify or retry, instead of the 404 ResourceNotFoundException the SDK's error factory models, and version-pinning flows cannot distinguish "bad version" from "service broken".
Fix: implement UpdateAgentRuntimeEndpoint's handler to resolve the runtime and the requested agentRuntimeVersion against state and return the modeled ResourceNotFoundException with AWS's sentence when the version does not exist, leaving the endpoint's targetVersion unchanged. Citation: services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/update_agent_runtime_endpoint.rs.
Traces: bedrock-agentcore-control.update-agent-runtime-endpoint-nonexistent-version-rejected (mismatch: expected ResourceNotFoundException, got 'NotImplemented').

## Evidence
Cause 1 — check agent-runtime-endpoint-pinned-version-survives-runtime-update:
  Expected: GetAgentRuntimeEndpoint(agentRuntimeId=<new runtime>, endpointName="DEFAULT") -> 200 with status READY and targetVersion/liveVersion equal to the runtime's latest version ("1" right after create), advancing on publish.
  Actual: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetAgentRuntimeEndpoint operation: Agent endpoint with name DEFAULT not found.

Cause 2 — check delete-agent-runtime-while-endpoint-in-service-blocked:
  Expected: DeleteAgentRuntime while a READY endpoint is attached -> 409 ConflictException, Message "This agent has endpoints that must be deleted before the agent can be removed."; GetAgentRuntime afterwards does NOT report DELETING.
  Actual: HTTP 202, body {'status': 'DELETING', 'agentRuntimeId': '<runtime-id>', 'agentRuntimeVersion': '1'} (AssertionError: expected the call to be rejected, but it succeeded). Handler delete_agent_runtime.rs performs no endpoint check.

Cause 3 — check update-agent-runtime-endpoint-nonexistent-version-rejected:
  Expected: UpdateAgentRuntimeEndpoint(agentRuntimeVersion="88888") -> 404 ResourceNotFoundException, Message "Agent with agentId: <agentRuntimeId> not found"; endpoint still targets the real version.
  Actual: AssertionError: expected ResourceNotFoundException, got 'NotImplemented' (scaffold 501 channel).

## Steps to Reproduce
Cause 1: CreateAgentRuntime (containerConfiguration artifact, networkMode PUBLIC, an assumable execution role); poll GetAgentRuntime until status != CREATING; then call GetAgentRuntimeEndpoint(agentRuntimeId=<id>, endpointName="DEFAULT") — observe 404 ResourceNotFoundException instead of a READY DEFAULT endpoint on version 1.

Cause 2: CreateAgentRuntime as above; poll to READY; CreateAgentRuntimeEndpoint(agentRuntimeId=<id>, name=<ep>) and poll GetAgentRuntimeEndpoint until status READY; call DeleteAgentRuntime(agentRuntimeId=<id>) — observe 202/DELETING instead of 409 ConflictException, and the runtime's versions removed.

Cause 3: CreateAgentRuntime as above; poll to READY; CreateAgentRuntimeEndpoint(agentRuntimeId=<id>, name=<ep>, agentRuntimeVersion="1"); call UpdateAgentRuntimeEndpoint(agentRuntimeId=<id>, endpointName=<ep>, agentRuntimeVersion="88888") — observe error code NotImplemented instead of 404 ResourceNotFoundException "Agent with agentId: <id> not found".

Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-agent-runtime-not-found-errors
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-agent-runtime-not-found-errors
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|GetAgentRuntimeEndpoint|4xx->4xx|$.message
Issue-Commit: c4151463709a9c5d3f4c5db5b8acfd63028df906

# bedrock-agentcore-control: GetAgentRuntimeEndpoint not-found message names the agent instead of AWS's endpoint-and-agent sentence

> _Note: this issue was identified against the code base at commit `c4151463709a9c5d3f4c5db5b8acfd63028df906`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAgentRuntimeEndpoint

## Description
One net-new cause in the bedrock-agentcore-control agent-runtime endpoint read path. It is not covered by any open issue: the closest neighbour, 20261103T182627Z-bedrock-agentcore-control-agent-runtime-endpoint-gaps-no-auto-created-default.md, is a behavioral issue about a missing auto-created DEFAULT endpoint, DeleteAgentRuntime accepting a live endpoint and UpdateAgentRuntimeEndpoint answering 501 — none of its three causes is the ResourceNotFoundException MESSAGE that GetAgentRuntimeEndpoint serves when the addressed runtime does not exist.

=== Cause 1 of 1: GetAgentRuntimeEndpoint on a runtime that does not exist reports an agent-scoped not-found sentence instead of AWS's endpoint-scoped one ===
AWS: GetAgentRuntimeEndpoint (GET /runtimes/{agentRuntimeId}/runtime-endpoints/{endpointName}/, success 200) answers a read that cannot be resolved with ResourceNotFoundException, HTTP 404, x-amzn-errortype ResourceNotFoundException. Smithy: GetAgentRuntimeEndpoint models ResourceNotFoundException (httpError 404, "This exception is thrown when a resource referenced by the operation does not exist"); both agentRuntimeId (pattern ^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$) and endpointName (pattern ^[a-zA-Z][a-zA-Z0-9_]{0,47}$) are required @httpLabel members. The model does not state the message text — recorded behavior only: AWS reports the failure at the ENDPOINT level and names both labels, with the template "Agent endpoint with name <endpointName> not found for agent <agentRuntimeId>". Its fixed text is "Agent endpoint with name ", " not found for agent " (and no trailing period); the two variable parts are the endpointName and the agentRuntimeId taken verbatim from the request URI. The recording shows this even when the runtime itself is absent: AWS does not expose a separate agent-scoped sentence on this read — a request naming an unknown runtime and an unknown endpoint still gets the endpoint sentence carrying both names.
Emulator: status 404 and the error code are right, but the message is the agent-scoped template "Agent with agentId: <agentRuntimeId> not found" — fixed text "Agent with agentId: " + the request's agentRuntimeId + " not found", with no endpoint name at all. The handler resolves the runtime first and, on a miss, raises state::agent_not_found_for_endpoint(&input.agent_runtime_id); the endpoint-miss branch below it raises state::agent_runtime_endpoint_not_found(&input.endpoint_name), which names the endpoint but omits the " for agent <agentRuntimeId>" half. So no code path in the emulator can produce AWS's sentence for this operation.
Why: the message is the only part of a 404 that tells a caller WHICH identifier failed to resolve. A client (or a human reading a log) that gets AWS's sentence learns the endpoint name and its owning agent in one line; against the emulator it learns only an agent id, so endpoint-resolution diagnostics, log-scraping assertions and error-message-matching retry logic keyed on "Agent endpoint with name ... not found for agent ..." never match, and a tool that reports the unresolved endpoint name to the user cannot recover it from the response.
Fix: in GetAgentRuntimeEndpoint's handler and the shared message builders it calls, serve a single endpoint-scoped ResourceNotFoundException for both the unknown-runtime and the unknown-endpoint branch, formatted from the request's endpointName and agentRuntimeId as AWS words it; keep the 404 status and ResourceNotFoundException code unchanged.
Traces: bedrock-agentcore-control-agent-runtime-not-found-errors#task#2 (mismatch on $.message; status 404 and errortype already agree).

## Evidence
Fixture bedrock-agentcore-control-agent-runtime-not-found-errors, step task#2 — GetAgentRuntimeEndpoint against a runtime id that was never created:
  Expected (recorded AWS): HTTP 404, x-amzn-errortype ResourceNotFoundException, $.message = "Agent endpoint with name missingendpoint not found for agent missingruntime-abcdefghij" (template: "Agent endpoint with name " + <endpointName> + " not found for agent " + <agentRuntimeId>; the two names come from the request path).
  Actual (emulator): HTTP 404, ResourceNotFoundException, $.message = "Agent with agentId: missingruntime-abcdefghij not found" (template: "Agent with agentId: " + <agentRuntimeId> + " not found"; the endpointName is absent from the response).
  Replay verdict: mismatch, expected_status 404 / actual_status 404, diverging key $.message only.
  Source: services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/get_agent_runtime_endpoint.rs:193-202 (runtime miss -> crate::state::agent_not_found_for_endpoint(&input.agent_runtime_id); endpoint miss -> crate::state::agent_runtime_endpoint_not_found(&input.endpoint_name), neither of which builds AWS's two-name sentence).

## Steps to Reproduce
1. Boot the emulator and point an AWS SDK/CLI client for bedrock-agentcore-control at it.
2. Call GetAgentRuntimeEndpoint with an agentRuntimeId that satisfies the modeled pattern but was never created (e.g. a "<name>-<10 alnum>" id such as missingruntime-abcdefghij) and any syntactically valid endpointName (e.g. missingendpoint).
3. Observe HTTP 404 ResourceNotFoundException whose message is "Agent with agentId: <agentRuntimeId> not found" instead of AWS's "Agent endpoint with name <endpointName> not found for agent <agentRuntimeId>" — the endpoint name never appears in the response.
4. Equivalent replay: replay fixture bedrock-agentcore-control-agent-runtime-not-found-errors, step task#2.

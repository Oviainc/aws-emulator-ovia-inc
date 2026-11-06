Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-agent-runtime-control-plane-errors
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-agent-runtime-control-plane-errors
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|ListAgentRuntimeEndpoints|2xx->4xx|$.runtimeEndpoints
Issue-Key: bedrock-agentcore-control|ListAgentRuntimeEndpoints|2xx->4xx|status
Issue-Commit: c0478c2dc62a949b47f12552e33c2facb0cc4b79

# bedrock-agentcore-control ListAgentRuntimeEndpoints refuses an unknown agentRuntimeId with 400 ValidationException instead of answering an empty page

> _Note: this issue was identified against the code base at commit `c0478c2dc62a949b47f12552e33c2facb0cc4b79`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAgentRuntimeEndpoints

## Description
One net-new root cause in the bedrock-agentcore-control agent-runtime endpoint listing surface. It is not covered by any open issue: the closest neighbours are 20261103T182627Z-bedrock-agentcore-control-agent-runtime-endpoint-gaps-no-auto-created-default.md (a missing auto-created DEFAULT endpoint, DeleteAgentRuntime accepted with a live endpoint, and UpdateAgentRuntimeEndpoint answering 501 — all about endpoint lifecycle, none about the listing's response to an unresolvable runtime) and 20261103T193348Z-bedrock-agentcore-control-getagentruntimeendpoint-not-found-message-names.md (the wording of GetAgentRuntimeEndpoint's modeled 404, an operation that DOES model a not-found error). Neither states what the listing must answer for a runtime that is not there.

=== Cause 1 of 1: ListAgentRuntimeEndpoints turns an unresolvable agentRuntimeId into a modeled ValidationException instead of a successful empty listing ===
AWS: ListAgentRuntimeEndpoints (POST /runtimes/{agentRuntimeId}/runtime-endpoints/, success 200) models exactly four errors — AccessDeniedException (403), InternalServerException (500), ThrottlingException (429) and ValidationException (400) — and NO not-found error, while its output member runtimeEndpoints (AgentRuntimeEndpoints) is required and nextToken is optional. Smithy: ListAgentRuntimeEndpoints http POST /runtimes/{agentRuntimeId}/runtime-endpoints/ code 200; input agentRuntimeId @httpLabel @required pattern ^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$, maxResults @range 1..100, nextToken @length 1..2048 @pattern ^\S*$; output runtimeEndpoints required; errors AccessDeniedException/InternalServerException/ThrottlingException/ValidationException only. The recording confirms the consequence: a syntactically valid agentRuntimeId that names no runtime of the account is answered HTTP 200 with runtimeEndpoints: [] (an empty list, no nextToken) — recorded behavior only for the specific choice of an empty page over a refusal, since the model states the absence of a not-found error rather than the page semantics. ValidationException is reserved for input that fails the modeled constraints (a malformed id, an out-of-range maxResults, an unparsable nextToken), not for a well-formed id whose resource does not exist.
Emulator: the handler resolves the runtime out of state FIRST and, when the id is absent from the store, returns HTTP 400 with x-amzn-errortype ValidationException, reason fieldValidationFailed and fieldList [{name: "agentRuntimeId", message: "must name an agent runtime of this account"}], carrying the endpoint family's not-found sentence "Agent with agentId: <agentRuntimeId> not found" (fixed text "Agent with agentId: " and " not found" around the agentRuntimeId taken from the request path). No body member runtimeEndpoints is produced at all. The comment in the handler states the choice deliberately ("refused — never an empty page"), so this is a modelling decision to reverse, not an oversight.
Why: a client enumerating a runtime's endpoints — the normal way to discover endpoints before invoking one, and the normal cleanup/reconciliation probe after a runtime is deleted — gets a 400 it must classify as a malformed request rather than the empty result AWS returns. Because ValidationException is a non-retryable client fault, SDK error handling and reconcilers treat the absent runtime as a bug in their own request: a loop that lists endpoints for each known runtime aborts on the first stale id instead of seeing zero endpoints, and code that distinguishes "runtime gone" (empty page) from "bad input" (ValidationException) can no longer tell the two apart, since the emulator reports the missing resource in fieldList as though the member itself were invalid.
Fix: in ListAgentRuntimeEndpoints' handler, the state lookup must not be a refusal path. Resolve the runtime, and when it is absent serve the modeled success — runtimeEndpoints as an empty list and no nextToken — keeping ValidationException for the modeled input constraints only (id pattern, maxResults range, undecodable nextToken). The existing pagination and summary rendering are unaffected. Citations: services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/list_agent_runtime_endpoints.rs:166-179 (the refusal) and :231-242 (unresolvable_runtime, the ValidationException it builds).
Traces: bedrock-agentcore-control-agent-runtime-control-plane-errors#task#3.

## Evidence
Cause 1 — fixture bedrock-agentcore-control-agent-runtime-control-plane-errors, step task#3, operation ListAgentRuntimeEndpoints:
  Expected (recorded AWS): HTTP 200, body {"runtimeEndpoints": []} — an empty, successful page for a well-formed agentRuntimeId that names no runtime.
  Actual (emulator): HTTP 400. Replay detail: "status: expected 200, got 400" and "$.runtimeEndpoints: missing (expected [], got None)". The error served is the modeled ValidationException (x-amzn-errortype: ValidationException) with reason fieldValidationFailed, fieldList entry {name: "agentRuntimeId", message: "must name an agent runtime of this account"} and message "Agent with agentId: <agentRuntimeId> not found" — fixed text "Agent with agentId: "/" not found" around the id taken from the request path (the same sentence crate::state::agent_not_found_for_endpoint serves for GetAgentRuntimeEndpoint, which unlike this operation DOES model a 404).
  Source of the behavior: list_agent_runtime_endpoints.rs:172-179 does store.agent_runtimes.get(&input.agent_runtime_id).ok_or_else(|| ...ValidationException(unresolvable_runtime(...)))?, i.e. it refuses before any paging happens.
  Model grounding: the operation's error set contains no not-found error, and runtimeEndpoints is a required output member, so the only modeled answer for a resolvable request shape is a 200 page.

## Steps to Reproduce
1. Boot the emulator and target the bedrock-agentcore-control endpoint.
2. Do NOT create any agent runtime (or delete the one you created).
3. Call ListAgentRuntimeEndpoints with an agentRuntimeId that satisfies the modeled pattern ^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$ but names no runtime of the account (e.g. POST /runtimes/missingruntime-abcdefghij/runtime-endpoints/ with an empty body).
4. Observe HTTP 400 with x-amzn-errortype ValidationException and the "Agent with agentId: ... not found" message, instead of HTTP 200 with {"runtimeEndpoints": []}.
Replay check: replay fixture bedrock-agentcore-control-agent-runtime-control-plane-errors step task#3.

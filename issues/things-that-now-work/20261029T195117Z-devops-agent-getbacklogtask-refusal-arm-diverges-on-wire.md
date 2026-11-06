Issue-Source: trace-sweep-divergence
Issue-Trace: devops-agent-get-backlog-task-rejection-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: devops-agent-get-backlog-task-rejection-paths
Issue-Shape-Id: com.amazonaws.devopsagent#DevOpsAgent
Issue-Key: devops-agent|GetBacklogTask|4xx->4xx|$.message
Issue-Key: devops-agent|GetBacklogTask|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: devops-agent|GetBacklogTask|4xx->4xx|status
Issue-Commit: 128b971c27980fd8fd6bfdfd4c5ec95349a617e4

# devops-agent GetBacklogTask refusal arm diverges on wire: emulator-authored not-found message and 404 ResourceNotFoundException for a missing agent space instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `128b971c27980fd8fd6bfdfd4c5ec95349a617e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBacklogTask

## Description
GetBacklogTask's negative paths diverge from AWS on the wire in two distinct ways. Both come from the same defect shape: the handler resolves the agent space and the task in ONE chained lookup and composes its own refusal prose, so (a) the not-found message is emulator-authored rather than AWS's wording and (b) a nonexistent agent space falls into the same not-found arm instead of failing closed on authorization. 2 causes.

Cause 1 of 2 - ResourceNotFoundException message for a missing task inside an existing agent space is emulator prose.
AWS: Smithy (com.amazonaws.devopsagent#GetBacklogTask, GET /backlog/agent-space/{agentSpaceId}/tasks/{taskId}, @readonly, success 200) takes agentSpaceId (AgentSpaceIdentifier, @httpLabel, @required, @length 1..2048) and taskId (ResourceId, @httpLabel, @required, @length 1..128, @pattern ^[a-zA-Z0-9_.-]+$), and models ResourceNotFoundException (client, httpError 404). The model does not state the Message text, so: recorded behavior only - for a taskId that does not exist inside an EXISTING agent space the service answers HTTP 404, x-amzn-errortype ResourceNotFoundException, message 'Task not found: <taskId>'. Message template: the fixed text is 'Task not found: ' and the ONLY variable part is the taskId value taken verbatim from the request path; the agent space id does NOT appear in it.
Emulator: returns the correct status (404) and code (ResourceNotFoundException) but the message 'task <taskId> was not found in agent space <agentSpaceId>' - different fixed text, lower-cased, and interpolating a second value (the resolved internal agent space id) that AWS's message never contains. Replay detail for this step: "$.message: value_mismatch (expected 'Task not found: <taskId>', got 'task <taskId> was not found in agent space <internal-space-id>')".
Why: clients, SDK error handlers, log scrapers and operator runbooks that key on AWS's 'Task not found' refusal text never match, so a benign missing-task result is misclassified; the message additionally leaks the internal agent-space identifier into a refusal where AWS discloses only the requested task id.
Fix: the ResourceNotFoundException construction in the GetBacklogTask handler's task-lookup arm - build the message from the fixed prefix 'Task not found: ' plus the request's taskId and nothing else.
Traces: devops-agent-get-backlog-task-rejection-paths#task#2.

Cause 2 of 2 - a nonexistent agent space is answered 404 ResourceNotFoundException instead of 403 AccessDeniedException.
AWS: Smithy models AccessDeniedException (client, httpError 403) on GetBacklogTask alongside ResourceNotFoundException (404), and agentSpaceId is the authorization scope in the URI. The model does not state which error is chosen, so: recorded behavior only - when agentSpaceId names an agent space that does not exist in the caller's account, the service fails CLOSED on the containing resource and discloses nothing about the task: HTTP 403, error code AccessDeniedException, message 'Access denied'. That message is entirely fixed text - neither the agent space id nor the task id is interpolated.
Emulator: resolves space and task in one chained lookup, so an unknown agent space lands in the SAME not-found arm as an unknown task: HTTP 404 with x-amzn-errortype ResourceNotFoundException and the Cause-1 composed message naming BOTH ids. Replay detail for this step: "status: expected 403, got 404", "$.message: value_mismatch (expected 'Access denied', got 'task <taskId> was not found in agent space <missing-space-id>')", "$headers.x-amzn-errortype: shape_mismatch (... got 'ResourceNotFoundException')". The two negative classes are indistinguishable on the wire.
Why: SDK error handling branches on the error code and status class, so a client cannot tell 'you may not touch this agent space' (credentials/permissions path, non-retryable without new auth) from 'that task is gone' (create/retry path). It is also an information-disclosure divergence: the emulator confirms whether agent spaces outside the caller's scope exist, which AWS deliberately hides behind a fixed 'Access denied'.
Fix: the GetBacklogTask handler's lookup/authorization ordering - resolve and authorize agentSpaceId FIRST and return AccessDeniedException (403) with the fixed message 'Access denied' when the space is not present for the caller's account, then look the task up inside the resolved space and only then return ResourceNotFoundException (404).
Traces: devops-agent-get-backlog-task-rejection-paths#task#3.

Related open issue: '20261029T180757Z-devops-agent-getbacklogtask-refusals-diverge-emulator-authored-not.md' describes the same two causes from the BEHAVIORAL corpus (Issue-Corpus: simparity-behavioral-aws-v1). It carries only check ids and no wire fixture binding (the replay reports owned_by: [] for both of these clusters), so it does not gate a fix against the wire corpus; this issue supplies the fixture/step replay keys. The other open devops-agent issues are ALREADY-FILED unimplemented-operation stubs (CreateAsset, GetOperatorApp, GetRecommendation, GetTrigger) and cover neither cause.

## Evidence
Fixture: devops-agent-get-backlog-task-rejection-paths (trace set simparity-wire-aws-v1), service devops-agent, operation GetBacklogTask.

Cause 1 - step task#2 (verdict mismatch, affected: devops-agent-get-backlog-task-rejection-paths#task#2):
  Expected (recorded AWS): HTTP 404, error code ResourceNotFoundException, message 'Task not found: <taskId>'
                           (fixed text 'Task not found: ' + the request path's taskId; no agent space id)
  Actual (emulator):       HTTP 404, error code ResourceNotFoundException, message 'task <taskId> was not found in agent space <agentSpaceId>'
  Replay detail: $.message: value_mismatch (expected 'Task not found: sim2real-probe-devops-agent-missing-task-e0168b97', got 'task sim2real-probe-devops-agent-missing-task-e0168b97 was not found in agent space as-0000000000000001')
  (the task id and the 'as-...' space id shown are per-run values; the divergence is the message TEMPLATE and the extra interpolated space id)

Cause 2 - step task#3 (verdict mismatch, affected: devops-agent-get-backlog-task-rejection-paths#task#3):
  Expected (recorded AWS): HTTP 403, error code AccessDeniedException, message 'Access denied' (wholly fixed text)
  Actual (emulator):       HTTP 404, x-amzn-errortype ResourceNotFoundException, message 'task <taskId> was not found in agent space <agentSpaceId>'
  Replay detail: status: expected 403, got 404 | $.message: value_mismatch (expected 'Access denied', got 'task sim2real-probe-devops-agent-missing-task-e0168b97 was not found in agent space sim2real-probe-devops-agent-missing-space-85ea2d77') | $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')

Smithy (services/service-com-amazonaws-devopsagent-devopsagent/smithy-model/com-amazonaws-devopsagent-devopsagent.json):
  GetBacklogTask GET /backlog/agent-space/{agentSpaceId}/tasks/{taskId}, success 200, @readonly, endpoint hostPrefix 'dp.'
  errors include AccessDeniedException (client, 403) and ResourceNotFoundException (client, 404)
  input: agentSpaceId @httpLabel @required @length 1..2048; taskId @httpLabel @required @length 1..128 @pattern ^[a-zA-Z0-9_.-]+$

Handler location to inspect: services/service-com-amazonaws-devopsagent-devopsagent/src/ops/get_backlog_task.rs (the single chained agent_spaces.get(..).and_then(|space| space.tasks.get(..)) lookup feeding one ResourceNotFoundException).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus: replay_wire_corpus() over trace set simparity-wire-aws-v1.
2. Cause 1 - fixture devops-agent-get-backlog-task-rejection-paths, step task#2: create an agent space, then GET /backlog/agent-space/{existing agentSpaceId}/tasks/{taskId that was never created, matching ^[a-zA-Z0-9_.-]+$}. Observe HTTP 404 ResourceNotFoundException with message 'task <taskId> was not found in agent space <agentSpaceId>'; AWS answers HTTP 404 with message 'Task not found: <taskId>'.
3. Cause 2 - same fixture, step task#3: GET /backlog/agent-space/{an agentSpaceId that does not exist}/tasks/{the same unknown taskId}. Observe HTTP 404 with x-amzn-errortype ResourceNotFoundException and the same composed message; AWS answers HTTP 403, error code AccessDeniedException, message 'Access denied'.
4. Re-replay devops-agent-get-backlog-task-rejection-paths#task#2 AND #task#3 to confirm BOTH causes are closed - fixing the message alone leaves the 403/404 arm diverging.

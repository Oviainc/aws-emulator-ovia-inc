Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: devops-agent.get-backlog-task-rejection-distinguishes-missing-space-from-missing-task
Issue-Shape-Id: com.amazonaws.devopsagent#DevOpsAgent
Issue-Key: devops-agent.get-backlog-task-rejection-distinguishes-missing-space-from-missing-task|run#10|AssertionError|assert message == f"Task not found: {missing_task_id}", f"unexpected message {message!r}"
Issue-Commit: ab6490681e1ab26d5567ff83a1d290924eb5dab0

# devops-agent GetBacklogTask refusals diverge: emulator-authored not-found message, and a missing agent space is answered ResourceNotFoundException instead of a fail-closed AccessDeniedException (2 causes)

> _Note: this issue was identified against the code base at commit `ab6490681e1ab26d5567ff83a1d290924eb5dab0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBacklogTask

## Description
GetBacklogTask's two negative classes both diverge from AWS. The emulator uses one lookup path (`agent_spaces.get(agentSpaceId).and_then(|space| space.tasks.get(taskId))`) and emits one emulator-invented ResourceNotFoundException for both, so it neither words the not-found refusal as AWS does nor distinguishes a missing agent space from a missing task inside a real space.

Cause 1 of 2 — ResourceNotFoundException Message for a missing task is emulator prose.
AWS: Smithy (com.amazonaws.devopsagent#GetBacklogTask): input members `agentSpaceId` (AgentSpaceIdentifier, httpLabel, required, @length 1..2048) and `taskId` (ResourceId, httpLabel, required, @length 1..128, @pattern ^[a-zA-Z0-9_.-]+$); modeled error ResourceNotFoundException (client, HTTP 404) documented as "The requested resource could not be found." The model does not state the Message text, so: recorded behavior only — for a task id that does not exist inside an existing agent space the service answers HTTP 404, Error.Code `ResourceNotFoundException`, Error.Message `Task not found: <taskId>`. Message template: the fixed text is `Task not found: ` and the only variable part is the `taskId` value taken verbatim from the request path. The agent space id does NOT appear in it.
Emulator: answers the right code (ResourceNotFoundException) but Message `task <taskId> was not found in agent space <agentSpaceId>` — different fixed text, lower-cased, and interpolating a second value (the resolved agent space id) that AWS's message does not contain. Replay observed: `unexpected message 'task sim2real-behav-devops-agent-missing-task-18189ad6d6a6 was not found in agent space as-0000000000000001'`.
Why: clients and operator tooling that match on the refusal text (log scrapers, retry/classification rules, assertion-based tests, and support runbooks that key on "Task not found") never match, and the message leaks the internal agent-space identifier into a refusal where AWS discloses only the requested task id.
Fix: the ResourceNotFoundException construction in the GetBacklogTask handler — build the message from the fixed `Task not found: ` prefix plus the request's `taskId`, and nothing else.
Traces: devops-agent.get-backlog-task-rejection-distinguishes-missing-space-from-missing-task (the first `_error(...)` call, on an existing space with an unknown task id).

Cause 2 of 2 — a nonexistent agent space is reported as a missing task (404) instead of failing closed with AccessDeniedException (403).
AWS: Smithy models AccessDeniedException (client, HTTP 403, "Access to the requested resource is denied due to insufficient permissions.") alongside ResourceNotFoundException (404) for GetBacklogTask, and `agentSpaceId` is the authorization scope in the URI `/backlog/agent-space/{agentSpaceId}/tasks/{taskId}`. The model does not state which is chosen, so: recorded behavior only — when `agentSpaceId` names an agent space that does not exist in the caller's account, the service fails CLOSED on the containing resource rather than disclosing anything about the task: HTTP 403, Error.Code `AccessDeniedException`, Error.Message `Access denied`. That message is entirely fixed text with no variable parts — neither the agent space id nor the task id is interpolated.
Emulator: resolves the space and the task in a single chained lookup, so an unknown agent space falls into the SAME ResourceNotFoundException arm as an unknown task and answers HTTP 404 `ResourceNotFoundException` with the Cause-1 message naming both ids. The two negative classes are indistinguishable, and the refusal confirms the space does not exist instead of refusing on permissions.
Why: a client cannot tell "you may not touch this agent space" from "that task is gone" — SDK error handling branches on the code, so a 404 invites a create/retry path where AWS requires a credentials/permissions path. It is also an information-disclosure divergence: the emulator reveals the existence (or non-existence) of agent spaces outside the caller's scope, which AWS deliberately hides behind a fixed `Access denied`.
Fix: the GetBacklogTask handler's lookup/authorization ordering — resolve and authorize `agentSpaceId` FIRST and return AccessDeniedException with the fixed message `Access denied` when the space is not present for the caller's account, then look the task up inside the resolved space and only then return ResourceNotFoundException.
Traces: devops-agent.get-backlog-task-rejection-distinguishes-missing-space-from-missing-task (the second `_error(...)` call, with a generated nonexistent `agentSpaceId`; the check aborts on Cause 1 before reaching this assertion, and the single chained lookup in the handler is what routes it to the same arm). No separate open issue covers either cause — `list_existing_issues(query='devops-agent')` returns none, and the suite's other devops-agent failures are all ALREADY FILED stub blockers (CreateAsset, GetOperatorApp, GetRecommendation, GetTrigger).

## Evidence
Check: devops-agent.get-backlog-task-rejection-distinguishes-missing-space-from-missing-task [mismatch]

Cause 1 — missing task in an existing agent space:
  Expected (AWS): HTTP 404, Error.Code='ResourceNotFoundException', Error.Message='Task not found: <taskId>'
                  (fixed text 'Task not found: ' + the request's taskId)
  Actual (emulator): Error.Code='ResourceNotFoundException', Error.Message='task <taskId> was not found in agent space <agentSpaceId>'
  Replay assertion: AssertionError: unexpected message 'task sim2real-behav-devops-agent-missing-task-18189ad6d6a6 was not found in agent space as-0000000000000001'
  (the id values shown are per-run; the divergence is the message TEMPLATE and the extra interpolated agent-space id)

Cause 2 — nonexistent agent space:
  Expected (AWS): HTTP 403, Error.Code='AccessDeniedException', Error.Message='Access denied' (wholly fixed text)
  Actual (emulator): HTTP 404, Error.Code='ResourceNotFoundException' with the Cause-1 message — same arm as a missing task

Handler evidence (services/service-com-amazonaws-devopsagent-devopsagent/src/ops/get_backlog_task.rs:157-169):
  store.agent_spaces.get(&input.agent_space_id)
       .and_then(|space| space.tasks.get(&input.task_id))
       .ok_or_else(|| ...ResourceNotFoundException { message: format!("task {} was not found in agent space {}", input.task_id, input.agent_space_id) })?
One chained Option collapses 'space absent' and 'task absent' into one 404, and the message is composed prose.

Smithy (services/service-com-amazonaws-devopsagent-devopsagent/smithy-model/com-amazonaws-devopsagent-devopsagent.json):
  GetBacklogTask GET /backlog/agent-space/{agentSpaceId}/tasks/{taskId}, success 200, @readonly
  errors include AccessDeniedException (client, 403) and ResourceNotFoundException (client, 404)

## Steps to Reproduce
1. CreateAgentSpace with a fresh name; keep the returned agentSpace.agentSpaceId (and confirm with GetAgentSpace as a positive control).
2. Call GetBacklogTask(agentSpaceId=<that space>, taskId=<a task id that was never created, matching ^[a-zA-Z0-9_.-]+$>).
   Observe Error.Code ResourceNotFoundException but Error.Message 'task <taskId> was not found in agent space <agentSpaceId>'.
   AWS returns HTTP 404 with Error.Message 'Task not found: <taskId>'.
3. Call GetBacklogTask(agentSpaceId=<a generated agent space id that does not exist>, taskId=<the same unknown task id>).
   Observe HTTP 404 ResourceNotFoundException with the same composed message.
   AWS returns HTTP 403, Error.Code AccessDeniedException, Error.Message 'Access denied'.
4. Replay the check to confirm: devops-agent.get-backlog-task-rejection-distinguishes-missing-space-from-missing-task

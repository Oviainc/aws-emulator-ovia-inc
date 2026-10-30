Issue-Source: trace-sweep-divergence
Issue-Trace: devops-agent-get-trigger-identifier-validation
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: devops-agent-get-trigger-identifier-validation
Issue-Shape-Id: com.amazonaws.devopsagent#DevOpsAgent
Issue-Key: devops-agent|GetTrigger|4xx->4xx|$.message
Issue-Key: devops-agent|GetTrigger|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: devops-agent|GetTrigger|4xx->4xx|status
Issue-Commit: 99a6bf4e8a9dd64f73b61f622317c845b92ac2b7

# devops-agent GetTrigger wire refusals diverge: emulator-authored validation and not-found messages, plus 404 for a missing agent space

> _Note: this issue was identified against the code base at commit `99a6bf4e8a9dd64f73b61f622317c845b92ac2b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTrigger

## Description
Replaying the wire fixture devops-agent-get-trigger-identifier-validation against the devops-agent emulator (services/service-com-amazonaws-devopsagent-devopsagent) shows that ALL THREE of GetTrigger's refusal arms are emulator-authored rather than AWS-shaped. THREE distinct root causes, each with its own fixture step; all three must be closed before this issue is resolved.

Note on overlap: an open BEHAVIORAL issue, 20261030T062154Z-devops-agent-gettrigger-refusal-arms-diverge-emulator-authored.md, describes the same three arms, but it is grounded in a behavioral check id (devops-agent.get-trigger-requires-goal-shaped-identifier) and carries NO wire replay key, so it does not cover these fixture steps and the replay reports them unowned. This issue supplies the wire bindings. A fix must satisfy both.

Cause 1 of 4... (there are 3 causes; numbered below as 1..3)

Cause 1 of 3 - the ValidationException message, its member path and the pattern it cites all diverge (fixture devops-agent-get-trigger-identifier-validation, step task#2).
AWS: a triggerId that is not goal-shaped is refused with ValidationException, HTTP 400, message `1 validation error detected. Value at '<json-pointer path>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>`. That is the fixed text of the class - note the SENTENCE-ENDING PERIOD after `detected`, NO echo of the offending value, and a JSON-pointer-style path. Its variable parts are the path and the pattern, taken from the member AWS validated: the path is `/goalId` and the pattern is the UUID-shaped goal form `^goal-[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`. Smithy: GetTrigger.triggerId targets ResourceId (@length 1..128, @pattern ^[a-zA-Z0-9_.-]+$), httpLabel, required; ValidationException is modelled (client error, httpError 400, defaulted). The narrower goal-shaped rule and the `/goalId` member naming are recorded behavior only - the model states only the loose ResourceId pattern, while the recorded response shows AWS validating the UUID-shaped goal form and naming the member `/goalId`.
Emulator: ValidationException, HTTP 400, but the message is `1 validation error detected: Value '<rejected value>' at 'triggerId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^goal-[a-zA-Z0-9_.-]+$` - a COLON instead of a period after `detected`, the rejected value ECHOED back, the wire member name `triggerId` instead of the JSON pointer `/goalId`, and a LOOSE `^goal-<ResourceId chars>$` pattern instead of the UUID-shaped one.
Why: a client that parses, logs or asserts on the validation message cannot match AWS's text. Worse, because the emulator's pattern is looser than AWS's, identifiers AWS REJECTS as malformed (any `goal-<anything>`) are ACCEPTED by the emulator and fall through to a resource lookup - so the emulator answers ResourceNotFoundException/404 where AWS answers ValidationException/400, and the message advertises the wrong rule for the client to satisfy.
Fix: the pre-lookup validation arm of the GetTrigger handler (its identifier guard and the ValidationException it constructs, including the ValidationExceptionField path) and the shared goal/trigger id pattern constant it cites in the service state module.
Traces: devops-agent-get-trigger-identifier-validation#task#2.

Cause 2 of 3 - a well-formed but unknown identifier is reported as a missing Trigger rather than a missing Goal (fixture devops-agent-get-trigger-identifier-validation, step task#3).
AWS: a syntactically valid goal-shaped id that names nothing is ResourceNotFoundException, HTTP 404, message `Goal not found: <requested identifier>`. Fixed text `Goal not found: `; the single variable part is the identifier copied from the request. Smithy: ResourceNotFoundException is modelled for GetTrigger (client error, httpError 404). The resource NOUN in the message is recorded behavior only - the model does not state it. Hypothesis: AWS reports the underlying Goal because a Trigger is a goal's schedule and the goal is the entity actually resolved.
Emulator: ResourceNotFoundException, HTTP 404 (status and code are right), but the message is `Trigger not found: <id>` - the wrong resource noun.
Why: a client keying off the message text, or an operator reading it, is told the wrong kind of resource is absent, and the message does not match AWS for identical input.
Fix: the not-found arm of the GetTrigger handler - it must name the Goal, not the Trigger.
Traces: devops-agent-get-trigger-identifier-validation#task#3.

Cause 3 of 3 - a missing agent space is answered as a not-found resource instead of failing closed (fixture devops-agent-get-trigger-identifier-validation, step task#4).
AWS: GetTrigger against an agentSpaceId that does not exist fails CLOSED: AccessDeniedException, HTTP 403, message `Access denied` - entirely fixed text, no variable part. The caller is never told whether the space exists. Smithy: AccessDeniedException is modelled for GetTrigger (client error, httpError 403); agentSpaceId is a required httpLabel targeting AgentSpaceIdentifier (@length 1..2048). The fail-closed CHOICE for an unresolvable agent space is recorded behavior only - the model lists both AccessDeniedException and ResourceNotFoundException, and the recording shows AWS picking the 403.
Emulator: ResourceNotFoundException, HTTP 404, x-amzn-errortype `ResourceNotFoundException`, message `Agent space not found: <agentSpaceId>` - wrong HTTP status, wrong error code, wrong errortype header, and an emulator-authored message that DISCLOSES the non-existence of a space the caller has no access to.
Why: client error handling branches on the error code and the status. Code that retries or creates on ResourceNotFoundException but surfaces a permissions failure on AccessDeniedException takes the opposite branch against the emulator, and retry/backoff policies keyed to 403 vs 404 diverge. It also turns the emulator into an existence oracle for agent spaces outside the caller's authorization, which AWS deliberately is not. The same fail-closed defect is already filed for sibling operations (20261029T195117Z-devops-agent-getbacklogtask-refusal-arm-diverges-on-wire.md for GetBacklogTask and 20261030T021252Z-devops-agent-getoperatorapp-missing-agentspace-resourcenotfoundexception-carries-emulator.md for GetOperatorApp); neither covers GetTrigger, which has its own copy of this arm.
Fix: the agent-space resolution arm of the GetTrigger handler - an unresolvable space must raise the modelled AccessDeniedException with the fixed `Access denied` message.
Traces: devops-agent-get-trigger-identifier-validation#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture devops-agent-get-trigger-identifier-validation, all three steps verdict=mismatch, service=devops-agent, operation=GetTrigger, none owned by an open issue and none blocked by a stub.

Cause 1 - step task#2 (expected_status 400, actual_status 400):
  $.message value_mismatch
  expected: "1 validation error detected. Value at '/goalId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^goal-[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"
  actual:   "1 validation error detected: Value 'not-a-trigger' at 'triggerId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^goal-[a-zA-Z0-9_.-]+$"
  The error code (ValidationException) and status (400) match; the message text diverges in its fixed text (`detected.` vs `detected:`, value echoed), in its member path (`/goalId` vs `triggerId`) and in the cited pattern (UUID-shaped vs loose). `not-a-trigger` is the request's own input filling the template, not part of the requirement.

Cause 2 - step task#3 (expected_status 404, actual_status 404):
  $.message value_mismatch
  expected: "Goal not found: goal-00000000-0000-0000-0000-000000000000"
  actual:   "Trigger not found: goal-00000000-0000-0000-0000-000000000000"
  Fixed text is `Goal not found: `; the all-zero goal id is the request's input filling the template.

Cause 3 - step task#4 (expected_status 403, actual_status 404):
  status: expected 403, got 404
  $.message value_mismatch - expected "Access denied" (wholly fixed text), got "Agent space not found: sim2real-probe-devops-agent-missing-space-e3c01c83"
  $headers.x-amzn-errortype: shape_mismatch - the emulator emits `ResourceNotFoundException` where the recording carries the 403 AccessDeniedException arm.
  The space id is a per-run generated value filling the emulator's message; it is not part of any requirement - the requirement is the 403/AccessDeniedException/"Access denied" triple.

## Steps to Reproduce
1. Boot the emulator from this checkout and point a devops-agent client (or raw HTTPS) at its data listener.
2. Create an agent space to work in: space_id = create_agent_space(name=<unique>)["agentSpace"]["agentSpaceId"].
3. Cause 1 (fixture devops-agent-get-trigger-identifier-validation, step task#2): GET /trigger/agent-space/{space_id}/triggers/not-a-trigger (get_trigger(agentSpaceId=space_id, triggerId="not-a-trigger")). Observe HTTP 400 ValidationException whose message uses `detected:`, echoes the value, names `triggerId` and cites `^goal-[a-zA-Z0-9_.-]+$`, instead of AWS's `1 validation error detected. Value at '/goalId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^goal-[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`. Additionally call get_trigger(agentSpaceId=space_id, triggerId="goal-not-a-uuid") to see the loose pattern ACCEPT an id AWS rejects with 400.
4. Cause 2 (step task#3): get_trigger(agentSpaceId=space_id, triggerId="goal-00000000-0000-0000-0000-000000000000"). Observe 404 ResourceNotFoundException with message `Trigger not found: <id>` instead of `Goal not found: <id>`.
5. Cause 3 (step task#4): get_trigger(agentSpaceId=<an agent space id that was never created>, triggerId="goal-00000000-0000-0000-0000-000000000000"). Observe HTTP 404, x-amzn-errortype ResourceNotFoundException, message `Agent space not found: <id>`, instead of HTTP 403 AccessDeniedException with message `Access denied`.
6. Re-replay the wire corpus and confirm devops-agent-get-trigger-identifier-validation steps task#2, task#3 and task#4 all pass.

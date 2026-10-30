Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: devops-agent.get-trigger-requires-goal-shaped-identifier
Issue-Shape-Id: com.amazonaws.devopsagent#DevOpsAgent
Issue-Key: devops-agent.get-trigger-requires-goal-shaped-identifier|run#9|AssertionError|assert message == (
Issue-Commit: 8a5e62a415deb94ea31f987a6913244850fd3b2f

# devops-agent GetTrigger refusal arms diverge: emulator-authored ValidationException wording/path/pattern, "Trigger not found" instead of AWS's Goal wording, and a missing agent space answered 404 ResourceNotFoundException instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `8a5e62a415deb94ea31f987a6913244850fd3b2f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTrigger

## Description
GetTrigger on the devops-agent emulator (services/service-com-amazonaws-devopsagent-devopsagent) refuses requests with emulator-authored shapes in all three of its rejection arms. Three distinct root causes, all in the GetTrigger handler's refusal paths, all grounded in the single failing check devops-agent.get-trigger-requires-goal-shaped-identifier (the check aborted at the first assertion, so causes 2 and 3 are the remaining asserted arms of that same check, confirmed against the handler source).

Cause 1 of 3 — ValidationException message, field path and cited pattern all diverge.
AWS: the goal-shaped-identifier rejection is reported as a standard AWS validation refusal: ValidationException, HTTP 400, message `1 validation error detected. Value at '/goalId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^goal-[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`. Fixed text of that class: `1 validation error detected. Value at '<json-pointer path>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>` — note the sentence-ending period after `detected`, no echo of the offending value, a JSON-pointer-style path, and the UUID-shaped goal pattern. Variable parts: the path (`/goalId`) and the pattern. Smithy: GetTrigger.triggerId targets ResourceId (@length 1..128, @pattern ^[a-zA-Z0-9_.-]+$) and ValidationException is modelled (HTTP 400, defaulted); the narrower goal-id form is recorded behavior only — the model does not state it, and the recording shows AWS validating against the UUID-shaped goal pattern and naming the member `/goalId`, not `triggerId`.
Emulator: ValidationException with message `1 validation error detected: Value 'not-a-trigger' at 'triggerId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^goal-[a-zA-Z0-9_.-]+$` — a colon instead of a period after `detected`, the rejected value echoed, the wire member name `triggerId` instead of `/goalId`, and a loose `^goal-<ResourceId chars>$` pattern rather than the UUID-shaped one. The ValidationExceptionField path is likewise `triggerId`.
Why: a client that asserts on, logs or parses the validation message cannot match it; worse, the loose pattern means identifiers AWS rejects (e.g. `goal-anything`) are ACCEPTED by the emulator and fall through to a lookup, so the emulator classifies a malformed id as a missing resource where AWS classifies it as a validation error. The advertised pattern in the message also tells a client the wrong rule to satisfy.
Fix: the pre-lookup validation arm of the GetTrigger handler (the `is_trigger_id` guard and the ValidationException it constructs) and the shared trigger/goal id pattern it cites in the service state module.
Traces: devops-agent.get-trigger-requires-goal-shaped-identifier (first assertion — the one that actually failed).

Cause 2 of 3 — a well-formed but unknown identifier is reported as a missing Trigger, not a missing Goal.
AWS: a syntactically valid goal-shaped id that names nothing is ResourceNotFoundException, HTTP 404, message `Goal not found: <the requested identifier>` — fixed text `Goal not found: `, variable part the identifier taken from the request. Smithy: ResourceNotFoundException is modelled for GetTrigger (HTTP 404); the resource NOUN in the message is recorded behavior only (AWS reports the underlying Goal, since a Trigger is a goal's schedule).
Emulator: ResourceNotFoundException with message `Trigger not found: <id>`.
Why: a client keying off the message (or a human reading it) is told the wrong resource kind is absent, and the message does not match AWS for the same input.
Fix: the not-found arm of the GetTrigger handler.
Traces: devops-agent.get-trigger-requires-goal-shaped-identifier (second assertion block).

Cause 3 of 3 — a missing agent space is answered as a not-found resource instead of failing closed.
AWS: GetTrigger against an agentSpaceId that does not exist fails CLOSED: AccessDeniedException, HTTP 403, message `Access denied` (fixed text, no variable part) — the caller is not told whether the space exists. Smithy: AccessDeniedException is modelled for GetTrigger (HTTP 403, "Access to the requested resource is denied due to insufficient permissions"); the fail-closed CHOICE for an unknown agent space is recorded behavior only.
Emulator: ResourceNotFoundException, HTTP 404, message `Agent space not found: <agentSpaceId>` — an emulator-authored message, the wrong error code and the wrong HTTP status, and it discloses the non-existence of the space.
Why: a client's error handling branches on the error code and status; code that retries/creates on ResourceNotFoundException but surfaces a permissions problem on AccessDeniedException takes the opposite branch against the emulator. It also makes the emulator an existence oracle for agent spaces the caller cannot access, which AWS deliberately is not. This is the same fail-closed defect already filed for GetBacklogTask (20261029T180757Z-devops-agent-getbacklogtask-refusals-diverge-emulator-authored-not.md) and GetOperatorApp (20261030T021252Z-...); that issue does NOT cover GetTrigger, whose handler has its own copy of the arm.
Fix: the agent-space resolution arm of the GetTrigger handler (the space lookup must fail closed with the modelled AccessDeniedException).
Traces: devops-agent.get-trigger-requires-goal-shaped-identifier (third assertion block).

## Evidence
Failing check: devops-agent.get-trigger-requires-goal-shaped-identifier (verdict: mismatch).

Cause 1 — assertion that failed in the replay:
  expected message: "1 validation error detected. Value at '/goalId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^goal-[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"
  actual message:   "1 validation error detected: Value 'not-a-trigger' at 'triggerId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^goal-[a-zA-Z0-9_.-]+$"
  (error code ValidationException matched; message text, member path and cited pattern all diverge)
  AssertionError raised at check_devops_agent_get_trigger_requires_goal_shaped_identifier.py line 50.
  Emulator source: services/service-com-amazonaws-devopsagent-devopsagent/src/ops/get_trigger.rs lines 155-174 build the message with "1 validation error detected: Value '{}' at 'triggerId' ..." and cite crate::state::TRIGGER_ID_PATTERN; ValidationExceptionField.path is "triggerId" (line 165).

Cause 2 — asserted by the same check, not reached because cause 1 aborted it:
  input: triggerId = "goal-00000000-0000-0000-0000-000000000000" in an existing agent space
  expected: ResourceNotFoundException / "Goal not found: goal-00000000-0000-0000-0000-000000000000"
  emulator: ResourceNotFoundException / "Trigger not found: <triggerId>" — get_trigger.rs lines 188-192.

Cause 3 — asserted by the same check, not reached:
  input: agentSpaceId = a never-created space id, triggerId = a well-formed goal id
  expected: AccessDeniedException, HTTP 403, message "Access denied"
  emulator: ResourceNotFoundException, HTTP 404, message "Agent space not found: <agentSpaceId>" — get_trigger.rs lines 179-187.

Values such as "not-a-trigger", the all-zero goal id and the generated space/trigger ids are the request's own inputs filling the message templates; they are not part of the requirement.

## Steps to Reproduce
1. Boot the emulator and point a botocore devops-agent client at it.
2. space_id = create_agent_space(name=<unique>, tags={"sim2real":"probe"})["agentSpace"]["agentSpaceId"].
3. Cause 1: call get_trigger(agentSpaceId=space_id, triggerId="not-a-trigger"). Observe ValidationException whose message reads "1 validation error detected: Value 'not-a-trigger' at 'triggerId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^goal-[a-zA-Z0-9_.-]+$" instead of AWS's "1 validation error detected. Value at '/goalId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^goal-[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$". Also call get_trigger(agentSpaceId=space_id, triggerId="goal-not-a-uuid") to see the loose pattern ACCEPT an id AWS rejects.
4. Cause 2: call get_trigger(agentSpaceId=space_id, triggerId="goal-00000000-0000-0000-0000-000000000000"). Observe "Trigger not found: ..." instead of "Goal not found: ...".
5. Cause 3: call get_trigger(agentSpaceId=<a never-created space id>, triggerId="goal-00000000-0000-0000-0000-000000000000"). Observe 404 ResourceNotFoundException "Agent space not found: ..." instead of 403 AccessDeniedException "Access denied".
6. Replay: run_behavioral_suite and re-run devops-agent.get-trigger-requires-goal-shaped-identifier; it must pass end to end.

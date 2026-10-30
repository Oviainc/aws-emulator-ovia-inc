Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: devops-agent.get-operator-app-returns-empty-state-without-enablement
Issue-Shape-Id: com.amazonaws.devopsagent#DevOpsAgent
Issue-Key: devops-agent.get-operator-app-returns-empty-state-without-enablement|run#11|AssertionError|assert message == (
Issue-Commit: d0aae8ae3b786349fe7939f0729ff56210cff281

# devops-agent GetOperatorApp missing-AgentSpace ResourceNotFoundException carries emulator-authored message instead of AWS's not-found wording

> _Note: this issue was identified against the code base at commit `d0aae8ae3b786349fe7939f0729ff56210cff281`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOperatorApp

## Description
GetOperatorApp's missing-AgentSpace refusal returns the right HTTP status and error code but composes its own refusal prose, so the ResourceNotFoundException message a client sees is emulator-authored rather than AWS's wording. 1 cause.

Related open issue: '20261030T011714Z-devops-agent-getoperatorapp-missing-agentspace-refusal-uses-emulator.md' describes this same class of defect for GetOperatorApp, but it was filed from the WIRE corpus and carries only the wire replay key (Issue-Trace devops-agent-get-operator-app-default-state, Issue-Step task#3); the behavioral replay reports this check as neither ALREADY FILED nor OWNED, so that issue does not carry this check's replay key. The other open devops-agent issues ('20261029T195117Z-devops-agent-getbacklogtask-refusal-arm-diverges-on-wire.md', '20261029T180757Z-devops-agent-getbacklogtask-refusals-diverge-emulator-authored-not.md') are the same class but for GetBacklogTask, and the remaining open issues are unimplemented-operation stubs (CreateAsset, GetRecommendation, GetTrigger); none cover GetOperatorApp.

Cause 1 of 1 - ResourceNotFoundException message for an agentSpaceId that names no AgentSpace is emulator prose and omits the account scope.
AWS: Smithy (com.amazonaws.devopsagent#GetOperatorApp, GET /v2/agentspaces/{agentSpaceId}/operator, @readonly, success 200, endpoint hostPrefix 'cp.') takes agentSpaceId (AgentSpaceId, @httpLabel, @required, @pattern ^[a-zA-Z0-9-]{1,64}$) and models ResourceNotFoundException (client, httpError 404) among its errors (also AccessDeniedException 403, ValidationException 400, InternalServerException 500). The model does not state the message text, so: recorded behavior only - when agentSpaceId names no AgentSpace in the caller's account the service answers HTTP 404 ResourceNotFoundException with the message 'Could not find Agent Space with provided agentSpaceId "<agentSpaceId>" in accountId "<accountId>"'. Message template: the fixed text is 'Could not find Agent Space with provided agentSpaceId ' and ' in accountId ', with both variable parts double-quoted - the agentSpaceId taken verbatim from the request path and the AWS account id of the calling identity taken from the request's credentials/state. AWS's wording capitalizes 'Agent Space' as two words and always names the account scope.
Emulator: answers HTTP 404 with code ResourceNotFoundException (status and code agree) but the message 'AgentSpace not found: <agentSpaceId>' - different fixed text, no quoting of the value, 'AgentSpace' as one word, and no accountId clause at all. The check's positive arm (GetOperatorApp on a space that never enabled an app returning an empty state) passes; only the refusal message diverges.
Why: a client, SDK error handler, log scraper or operator runbook that keys on AWS's refusal text never matches, so a benign 'this agent space does not exist in this account' result is misclassified or surfaced with text no AWS-targeting tooling recognizes. Dropping the account scope also removes the part that tells an operator the lookup ran against a different account than expected - the usual cause of this refusal - rather than that the agent space was deleted.
Fix: the ResourceNotFoundException construction in the GetOperatorApp handler's AgentSpace-lookup arm; build the message from AWS's fixed text plus the request's agentSpaceId and the caller's account id, both double-quoted, instead of the locally composed 'AgentSpace not found: ...' string. File and line citation under Traces.
Traces: behavioral check devops-agent.get-operator-app-returns-empty-state-without-enablement; code: services/service-com-amazonaws-devopsagent-devopsagent/src/ops/get_operator_app.rs lines 163-168.

## Evidence
Behavioral check devops-agent.get-operator-app-returns-empty-state-without-enablement, verdict mismatch.

The check creates an AgentSpace, asserts GetOperatorApp on it returns an empty state (this arm passes), then calls GetOperatorApp with an agentSpaceId that was never created (matching ^[a-zA-Z0-9-]{1,64}$).

  Expected (AWS): HTTP 404, Error.Code ResourceNotFoundException, Error.Message
      'Could not find Agent Space with provided agentSpaceId "<agentSpaceId>" in accountId "<accountId>"'
      (fixed text 'Could not find Agent Space with provided agentSpaceId ' + ' in accountId ', both values double-quoted; agentSpaceId from the request path, accountId from the caller's identity)
  Actual (emulator): HTTP 404, Error.Code ResourceNotFoundException, Error.Message
      'AgentSpace not found: <agentSpaceId>'

Replay failure:
  File "check_devops_agent_get_operator_app_returns_empty_state_without_enablement.py", line 52, in run
    assert message == (
  AssertionError: unexpected message 'AgentSpace not found: sim2real-behav-devops-agent-missing-space-14086395a9ba'

The probe agent-space id and the account id are per-run values filling the template; the divergence is the message TEMPLATE - its fixed text, its quoting of the value, and the entirely missing accountId clause - not those values. Status (404) and error code (ResourceNotFoundException) agree.

Smithy (services/service-com-amazonaws-devopsagent-devopsagent/smithy-model/com-amazonaws-devopsagent-devopsagent.json):
  GetOperatorApp GET /v2/agentspaces/{agentSpaceId}/operator, success 200, @readonly, endpoint hostPrefix 'cp.'
  input: agentSpaceId @httpLabel @required @pattern ^[a-zA-Z0-9-]{1,64}$
  errors: ResourceNotFoundException (client, 404), AccessDeniedException (403), ValidationException (400), InternalServerException (500), ConflictException (409), ThrottlingException (429), ...

Code: services/service-com-amazonaws-devopsagent-devopsagent/src/ops/get_operator_app.rs lines 164-168 -
  store.agent_spaces.get(&input.agent_space_id).ok_or_else(|| GetOperatorAppErrorModeled::ResourceNotFoundException(ResourceNotFoundException { message: format!("AgentSpace not found: {}", input.agent_space_id) }))

## Steps to Reproduce
1. Boot the emulator from this checkout and run the behavioral suite (simparity-behavioral-aws-v1).
2. Replay check devops-agent.get-operator-app-returns-empty-state-without-enablement: create an AgentSpace via CreateAgentSpace, call GetOperatorApp on it (expect an empty state - this passes), then call GetOperatorApp with an agentSpaceId that was never created and matches ^[a-zA-Z0-9-]{1,64}$.
3. Observe HTTP 404 ResourceNotFoundException with message 'AgentSpace not found: <agentSpaceId>'; AWS answers HTTP 404 ResourceNotFoundException with 'Could not find Agent Space with provided agentSpaceId "<agentSpaceId>" in accountId "<accountId>"'.
4. Re-replay devops-agent.get-operator-app-returns-empty-state-without-enablement to confirm the message template (fixed text, double-quoting of both values, and the accountId clause) now matches.

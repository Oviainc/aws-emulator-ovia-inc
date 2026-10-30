Issue-Source: trace-sweep-divergence
Issue-Trace: devops-agent-get-operator-app-default-state
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: devops-agent-get-operator-app-default-state
Issue-Shape-Id: com.amazonaws.devopsagent#DevOpsAgent
Issue-Key: devops-agent|GetOperatorApp|4xx->4xx|$.message
Issue-Commit: 68c23b152149323c5b4a39e6e1afab7c6a9b73aa

# devops-agent GetOperatorApp missing-AgentSpace refusal uses emulator-authored message instead of AWS not-found wording

> _Note: this issue was identified against the code base at commit `68c23b152149323c5b4a39e6e1afab7c6a9b73aa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOperatorApp

## Description
GetOperatorApp's not-found arm answers the right status and error code but composes its own refusal prose, so the ResourceNotFoundException message a client sees is emulator-authored rather than AWS's wording. 1 cause.

Cause 1 of 1 - ResourceNotFoundException message for an agentSpaceId that names no AgentSpace is emulator prose and omits the account scope.
AWS: Smithy (com.amazonaws.devopsagent#GetOperatorApp, GET /v2/agentspaces/{agentSpaceId}/operator, @readonly, success 200, endpoint hostPrefix 'cp.') takes agentSpaceId (AgentSpaceId, @httpLabel, @required, @pattern ^[a-zA-Z0-9-]{1,64}$) and models ResourceNotFoundException (client, httpError 404) among its errors. The model does not state the message text, so: recorded behavior only - when agentSpaceId names no AgentSpace in the caller's account the service answers HTTP 404 with the message 'Could not find Agent Space with provided agentSpaceId "<agentSpaceId>" in accountId "<accountId>"'. Message template: the fixed text is 'Could not find Agent Space with provided agentSpaceId ' and ' in accountId ', with both variable parts double-quoted - the agentSpaceId taken verbatim from the request path, and the 12-digit AWS account id of the calling identity taken from the request's credentials/state. AWS's wording is capitalized 'Agent Space' (two words) and always names the account scope.
Emulator: returns HTTP 404 ResourceNotFoundException (status and code match) but with the message 'AgentSpace not found: <agentSpaceId>' - different fixed text, no quoting, 'AgentSpace' as one word, and no accountId part at all. Replay detail for this step: "$.message: value_mismatch (expected 'Could not find Agent Space with provided agentSpaceId \"<agentSpaceId>\" in accountId \"<accountId>\"', got 'AgentSpace not found: <agentSpaceId>')".
Why: a client, SDK error handler, log scraper or operator runbook that keys on AWS's refusal text never matches, so a benign 'this agent space does not exist in this account' result is misclassified or reported with text no AWS-targeting tooling recognizes; the message also drops the account scope, which is the part that tells an operator the lookup was performed against a different account than they expected (the usual cause of this refusal) rather than that the agent space was deleted.
Fix: the ResourceNotFoundException construction in the GetOperatorApp handler's AgentSpace-lookup arm - build the message from AWS's fixed text plus the request's agentSpaceId and the caller's account id, both double-quoted, instead of the locally composed 'AgentSpace not found: ...' string.
Traces: devops-agent-get-operator-app-default-state#task#3.

Related open issues: '20261029T195117Z-devops-agent-getbacklogtask-refusal-arm-diverges-on-wire.md' and '20261029T180757Z-devops-agent-getbacklogtask-refusals-diverge-emulator-authored-not.md' describe the same CLASS of defect (emulator-authored refusal prose) but only for GetBacklogTask; the replay reports owned_by: [] for this cluster, so neither covers this operation and neither carries this fixture/step replay key. The other open devops-agent issues are already-filed unimplemented-operation stubs (CreateAsset, GetRecommendation, GetTrigger) and do not cover it either.

## Evidence
Fixture: devops-agent-get-operator-app-default-state (trace set simparity-wire-aws-v1), service devops-agent, operation GetOperatorApp, step task#3, verdict mismatch, affected: devops-agent-get-operator-app-default-state#task#3 (affected_count 1), divergence_key service=devops-agent|GetOperatorApp|status=404->404|keys=$.message.

  Expected (recorded AWS): HTTP 404, ResourceNotFoundException, message
      'Could not find Agent Space with provided agentSpaceId "<agentSpaceId>" in accountId "<accountId>"'
      (fixed text 'Could not find Agent Space with provided agentSpaceId ' + ' in accountId ', both values double-quoted; agentSpaceId from the request path, accountId from the caller's identity)
  Actual (emulator):       HTTP 404, ResourceNotFoundException, message
      'AgentSpace not found: <agentSpaceId>'
  Replay detail: $.message: value_mismatch (expected 'Could not find Agent Space with provided agentSpaceId "sim2real-probe-devops-agent-missing-space-5bf1b4a5" in accountId "000000000000"', got 'AgentSpace not found: sim2real-probe-devops-agent-missing-space-5bf1b4a5')
  (the probe agent-space id and the account id shown are per-run values filling the template; the divergence is the message TEMPLATE - its fixed text, its quoting and the missing accountId clause - not those values.)

Status (404) and error code (ResourceNotFoundException) agree; only the message diverges.

Smithy (services/service-com-amazonaws-devopsagent-devopsagent/smithy-model/com-amazonaws-devopsagent-devopsagent.json):
  GetOperatorApp GET /v2/agentspaces/{agentSpaceId}/operator, success 200, @readonly, endpoint hostPrefix 'cp.'
  input: agentSpaceId @httpLabel @required @pattern ^[a-zA-Z0-9-]{1,64}$
  errors include ResourceNotFoundException (client, 404), AccessDeniedException (403), ValidationException (400), InternalServerException (500)

Code location to inspect: services/service-com-amazonaws-devopsagent-devopsagent/src/ops/get_operator_app.rs lines 164-168 - store.agent_spaces.get(&input.agent_space_id).ok_or_else(|| ... ResourceNotFoundException { message: format!("AgentSpace not found: {}", input.agent_space_id) }).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus: replay_wire_corpus() over trace set simparity-wire-aws-v1.
2. Fixture devops-agent-get-operator-app-default-state, step task#3: issue GET /v2/agentspaces/{an agentSpaceId that was never created, matching ^[a-zA-Z0-9-]{1,64}$}/operator against the devops-agent control-plane endpoint.
3. Observe HTTP 404 with ResourceNotFoundException and message 'AgentSpace not found: <agentSpaceId>'; AWS answers HTTP 404 ResourceNotFoundException with 'Could not find Agent Space with provided agentSpaceId "<agentSpaceId>" in accountId "<accountId>"'.
4. Re-replay devops-agent-get-operator-app-default-state#task#3 to confirm the message template (fixed text, quoting, and the accountId clause) now matches.

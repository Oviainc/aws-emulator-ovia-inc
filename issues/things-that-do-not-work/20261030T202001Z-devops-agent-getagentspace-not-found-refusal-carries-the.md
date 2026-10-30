Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: devops-agent.delete-agent-space-orphans-contained-assets-behind-access-denied
Issue-Shape-Id: com.amazonaws.devopsagent#DevOpsAgent
Issue-Key: devops-agent.delete-agent-space-orphans-contained-assets-behind-access-denied|run#14|AssertionError|assert message == f"AgentSpace with agentSpaceId {space_id} not found.", f"unexpected message {message!r}"
Issue-Commit: a12986f4241917f433abd9b7a93c682fd0b73a1b

# devops-agent GetAgentSpace not-found refusal carries the shared "Could not find Agent Space …" wording instead of AWS's "AgentSpace with agentSpaceId <id> not found."

> _Note: this issue was identified against the code base at commit `a12986f4241917f433abd9b7a93c682fd0b73a1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAgentSpace

## Description
GetAgentSpace's ResourceNotFoundException refusal returns the right HTTP status (404) and the right error code (ResourceNotFoundException), but its message is composed by a state-module helper shared with the service's other control-plane lookups, so the text a client sees is a different template from the one AWS answers for GetAgentSpace itself. 1 cause.

Related open issues: '20261030T021252Z-devops-agent-getoperatorapp-missing-agentspace-resourcenotfoundexception-carries-emulator.md' and the resolved '20261030T011714Z-devops-agent-getoperatorapp-missing-agentspace-refusal-uses-emulator.md' cover the same CLASS of defect for GetOperatorApp, whose recorded missing-space wording IS the 'Could not find Agent Space with provided agentSpaceId "…" in accountId "…"' template; neither covers GetAgentSpace, whose own recorded not-found wording is different. The other open devops-agent issues ('…getbacklogtask-refusals-diverge…', '…getbacklogtask-refusal-arm-diverges-on-wire…', '…gettrigger-refusal-arms-diverge…', '…gettrigger-wire-refusals-diverge…', '…getrecommendation-stub…') are the same class but for GetBacklogTask, GetTrigger and GetRecommendation. This is NOT a reopen: no resolved issue claimed to fix GetAgentSpace.

Cause 1 of 1 — GetAgentSpace's ResourceNotFoundException message template is the shared control-plane not-found sentence, not GetAgentSpace's own.
AWS: Smithy (com.amazonaws.devopsagent#GetAgentSpace, GET /v1/agentspaces/{agentSpaceId}, @readonly, success 200, endpoint hostPrefix 'cp.') takes agentSpaceId (AgentSpaceId, @httpLabel, @required, @pattern ^[a-zA-Z0-9-]{1,64}$) and models ResourceNotFoundException (client, httpError 404) among its errors (also ValidationException 400, AccessDeniedException 403, InternalServerException 500, ConflictException 409, ThrottlingException 429). The model documents ResourceNotFoundException only as "The requested resource could not be found." and states no message text, so: recorded behavior only — when agentSpaceId names no AgentSpace (including one just removed by DeleteAgentSpace) GetAgentSpace answers HTTP 404 ResourceNotFoundException with the message 'AgentSpace with agentSpaceId <agentSpaceId> not found.'. Message template: the fixed text is 'AgentSpace with agentSpaceId ' + ' not found.' — 'AgentSpace' as one word, the value NOT quoted, a terminal period, and NO account clause; the single variable part is the agentSpaceId taken verbatim from the request path.
Emulator: answers HTTP 404 with code ResourceNotFoundException (status and code agree) but the message 'Could not find Agent Space with provided agentSpaceId "<agentSpaceId>" in accountId "<accountId>"' — entirely different fixed text, 'Agent Space' as two words, the id double-quoted, no terminal period, and an extra accountId clause carrying the caller's account id that AWS's GetAgentSpace wording does not contain. The message is built by crate::state::agent_space_not_found_message, a single helper the control plane shares, so GetAgentSpace inherits the wording recorded for a DIFFERENT operation (GetOperatorApp) rather than its own.
Why: a client, SDK error handler, log scraper or operator runbook that keys on AWS's GetAgentSpace not-found text never matches, so the ordinary 'this agent space does not exist / was deleted' outcome is misclassified or surfaced with text no AWS-targeting tooling recognizes. The spurious accountId clause additionally leaks the serving account into a message AWS keeps free of it, and makes a plain deleted-space result read like a cross-account authorization problem. The other arms of the same scenario are correct and must stay so: after DeleteAgentSpace the data-plane GetAsset on the orphaned asset does fail closed with AccessDeniedException / 'Access denied', which this check verifies and which passes.
Fix: the ResourceNotFoundException construction in the GetAgentSpace handler's AgentSpace-lookup arm — it must word its own refusal rather than call the shared control-plane helper, since the two operations' recorded templates differ. Give GetAgentSpace a message built from its fixed text plus the request's agentSpaceId (unquoted, terminal period, no account clause), and leave the shared helper to the operations whose recorded wording it actually matches. File and line citations under Traces.
Traces: behavioral check devops-agent.delete-agent-space-orphans-contained-assets-behind-access-denied; code: services/service-com-amazonaws-devopsagent-devopsagent/src/ops/get_agent_space.rs lines 149-153 (the ok_or_else that builds the error) and services/service-com-amazonaws-devopsagent-devopsagent/src/state.rs lines 244-258 (agent_space_not_found_message, the shared template).

## Evidence
Behavioral check devops-agent.delete-agent-space-orphans-contained-assets-behind-access-denied, verdict mismatch.

The check creates an AgentSpace, creates an artifact asset in it, confirms GetAsset reads the asset back (positive control — passes), calls DeleteAgentSpace, then asserts (a) the data-plane GetAsset on the orphaned asset is refused AccessDeniedException / 'Access denied' (this arm PASSES) and (b) the control-plane GetAgentSpace reports the space itself as not found.

Arm (b) diverges:

  Expected (AWS): HTTP 404, Error.Code ResourceNotFoundException, Error.Message
      'AgentSpace with agentSpaceId <agentSpaceId> not found.'
      (fixed text 'AgentSpace with agentSpaceId ' + ' not found.'; the one variable part is the
       agentSpaceId taken verbatim from the request path; value unquoted, terminal period, no account clause)
  Actual (emulator): HTTP 404, Error.Code ResourceNotFoundException, Error.Message
      'Could not find Agent Space with provided agentSpaceId "<agentSpaceId>" in accountId "<accountId>"'

Replay failure:
  File "check_devops_agent_delete_agent_space_orphans_contained_assets_behind_access_denied.py", line 63, in run
    assert message == f"AgentSpace with agentSpaceId {space_id} not found.", f"unexpected message {message!r}"
  AssertionError: unexpected message 'Could not find Agent Space with provided agentSpaceId "as-0000000000000001" in accountId "000000000000"'

The agent-space id and the account id in that line are per-run/per-environment values filling a template and are NOT the requirement. The divergence is the message TEMPLATE itself: its fixed text, its quoting of the value, its missing terminal period, and the whole extra 'in accountId "…"' clause. Status (404) and error code (ResourceNotFoundException) agree.

Smithy (services/service-com-amazonaws-devopsagent-devopsagent/smithy-model/com-amazonaws-devopsagent-devopsagent.json):
  GetAgentSpace GET /v1/agentspaces/{agentSpaceId}, success 200, @readonly, endpoint hostPrefix 'cp.'
  input: agentSpaceId @httpLabel @required @pattern ^[a-zA-Z0-9-]{1,64}$; includedData @httpQuery (ALL_DATA | METADATA_ONLY)
  errors: ResourceNotFoundException (client, 404) 'The requested resource could not be found.', ValidationException (400), AccessDeniedException (403), ConflictException (409), InternalServerException (500), ThrottlingException (429), ...
  DeleteAgentSpace DELETE /v1/agentspaces/{agentSpaceId}, success 204, @idempotent

Code:
  services/service-com-amazonaws-devopsagent-devopsagent/src/ops/get_agent_space.rs lines 149-153 —
    store.agent_spaces.get(&input.agent_space_id).ok_or_else(|| GetAgentSpaceErrorModeled::ResourceNotFoundException(ResourceNotFoundException { message: crate::state::agent_space_not_found_message(&input.agent_space_id) }))
  services/service-com-amazonaws-devopsagent-devopsagent/src/state.rs lines 253-258 —
    pub fn agent_space_not_found_message(agent_space_id: &str) -> String { format!("Could not find Agent Space with provided agentSpaceId \"{agent_space_id}\" in accountId \"{account_id}\"") }
  This one helper is the shared control-plane template; GetAgentSpace needs its own wording.

## Steps to Reproduce
1. Boot the emulator from this checkout and run the behavioral suite (simparity-behavioral-aws-v1).
2. Replay check devops-agent.delete-agent-space-orphans-contained-assets-behind-access-denied: CreateAgentSpace; CreateAsset (assetType 'artifact', one file) in that space; GetAsset on it to confirm the positive control; DeleteAgentSpace on the space.
3. Call GetAsset on the orphaned asset — observe HTTP 403 AccessDeniedException with message 'Access denied' (this arm is correct and must stay correct).
4. Call GetAgentSpace with the deleted space's agentSpaceId. Observe HTTP 404 ResourceNotFoundException with message 'Could not find Agent Space with provided agentSpaceId "<agentSpaceId>" in accountId "<accountId>"'; AWS answers HTTP 404 ResourceNotFoundException with 'AgentSpace with agentSpaceId <agentSpaceId> not found.'
5. The same refusal is reachable without any delete: call GetAgentSpace with an agentSpaceId that was never created and matches ^[a-zA-Z0-9-]{1,64}$.
6. Re-replay devops-agent.delete-agent-space-orphans-contained-assets-behind-access-denied to confirm the GetAgentSpace message template now matches (fixed text, unquoted id, terminal period, no accountId clause) while the AccessDeniedException arm still passes.

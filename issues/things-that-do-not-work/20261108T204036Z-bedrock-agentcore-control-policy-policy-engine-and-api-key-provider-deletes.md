Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-policy-engine-lifecycle
Issue-Step: teardown#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-policy-engine-lifecycle bedrock-agentcore-control-policy-error-paths bedrock-agentcore-control-policy-lifecycle bedrock-agentcore-control-api-key-credential-provider-lifecycle bedrock-agentcore-control-apikey-credential-provider-lifecycle
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|GetPolicyEngine|4xx->2xx|$
Issue-Key: bedrock-agentcore-control|GetPolicyEngine|4xx->2xx|status
Issue-Key: bedrock-agentcore-control|GetPolicy|4xx->2xx|$
Issue-Key: bedrock-agentcore-control|GetPolicy|4xx->2xx|status
Issue-Key: bedrock-agentcore-control|GetPolicy|4xx->2xx|$.message
Issue-Key: bedrock-agentcore-control|GetPolicy|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|GetPolicyEngine|4xx->2xx|$.message
Issue-Key: bedrock-agentcore-control|GetPolicyEngine|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|GetApiKeyCredentialProvider|4xx->2xx|$.message
Issue-Key: bedrock-agentcore-control|GetApiKeyCredentialProvider|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|GetApiKeyCredentialProvider|4xx->2xx|status
Issue-Commit: f392f82803b1854878505550560cb55ee4e9c61d

# Policy, policy-engine and API-key-provider deletes leave the resource readable forever

> _Note: this issue was identified against the code base at commit `f392f82803b1854878505550560cb55ee4e9c61d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPolicyEngine

## Description
This bundle has 5 causes. A delete of a policy engine, a policy or an API-key credential provider is accepted, but the emulator never removes the resource from its state: every subsequent read keeps answering 200 with the resource still in a settled status, so the resource is immortal. The replay polls each read for 180s (37 attempts) and it never becomes a 404.

Cause 1 of 5 - GetPolicyEngine after DeletePolicyEngine (fixture bedrock-agentcore-control-policy-engine-lifecycle, step teardown#4).
AWS: DeletePolicyEngine is DELETE /policy-engines/{policyEngineId} returning 202 (Smithy: DeletePolicyEngine http code 202, output status PolicyEngineStatus with values CREATING/ACTIVE/UPDATING/DELETING/CREATE_FAILED/UPDATE_FAILED/DELETE_FAILED); once the deletion settles the engine is gone and GET /policy-engines/{policyEngineId} answers 404 ResourceNotFoundException (Smithy: GetPolicyEngine errors include ResourceNotFoundException @httpError 404). The recorded response for this step is a 404.
Emulator: answers 200 with the full engine body and status 'ACTIVE' (replay: "status: expected 404, got 200"; body carries policyEngineId/policyEngineArn/status ACTIVE), and still does so after the poll is capped at 180s.
Why: a client that deletes an engine and waits for the resource to disappear never converges - cleanup loops and CloudFormation-style delete waiters hang, and the name is never freed for re-creation.
Fix: DeletePolicyEngine's handler/state - the delete must actually retire the engine from state (and show status DELETING while it does), so the GetPolicyEngine read path resolves nothing afterwards.

Cause 2 of 5 - GetPolicy after DeletePolicy (fixture bedrock-agentcore-control-policy-error-paths, step teardown#9).
AWS: DeletePolicy is DELETE /policy-engines/{policyEngineId}/policies/{policyId} returning 202; afterwards GET .../policies/{policyId} answers 404 ResourceNotFoundException (Smithy: GetPolicy errors include ResourceNotFoundException @httpError 404). Recorded status for this step: 404.
Emulator: 200 with the policy body, status 'ACTIVE', enforcementMode 'ACTIVE' and its definition intact; poll capped at 180s with "status 200 not in [404] after 37 attempt(s)".
Why: the policy keeps being reported as enforcing after the caller deleted it, so a client cannot tell a live policy from a deleted one.
Fix: DeletePolicy's handler/state - remove the policy from the owning engine's policy collection.

Cause 3 of 5 - GetPolicy after DeletePolicy, message asserted (fixture bedrock-agentcore-control-policy-lifecycle, step task#6).
AWS: the settled read answers 404 ResourceNotFoundException whose message is the fixed text 'Policy with ID ' + the requested policyId + ' not found' (ResourceNotFoundException.message; the policy id is a value taken from the request, not a constant).
Emulator: 200 with the policy body and no error at all - replay reports "$.message: missing", "$headers.x-amzn-errortype: missing" and "poll: error code None not in ['ResourceNotFoundException'] after 37 attempt(s)".
Why: the modeled ResourceNotFoundException is unreachable on this path, so SDK error handling for a deleted policy never fires.
Fix: DeletePolicy's state plus GetPolicy's not-found path - emit ResourceNotFoundException with x-amzn-errortype: ResourceNotFoundException and the message template above.

Cause 4 of 5 - GetPolicyEngine after DeletePolicyEngine, message asserted (fixture bedrock-agentcore-control-policy-lifecycle, step teardown#8).
AWS: 404 ResourceNotFoundException with the fixed text 'Policy engine with ID ' + the requested policyEngineId + ' not found'.
Emulator: 200 with the engine body, status 'ACTIVE'; "$.message: missing", "$headers.x-amzn-errortype: missing", poll exhausted after 37 attempts.
Why: same as cause 3, for the engine.
Fix: DeletePolicyEngine's state plus GetPolicyEngine's not-found path.

Cause 5 of 5 - GetApiKeyCredentialProvider after DeleteApiKeyCredentialProvider (fixture bedrock-agentcore-control-api-key-credential-provider-lifecycle, step teardown#8).
AWS: DeleteApiKeyCredentialProvider (POST /identities/DeleteApiKeyCredentialProvider, 204, empty output) retires the provider; GetApiKeyCredentialProvider then answers 404 ResourceNotFoundException (Smithy: GetApiKeyCredentialProvider errors include ResourceNotFoundException @httpError 404) with the fixed text 'ApiKeyCredentialProvider not found for ' + the requested provider name.
Emulator: 200 with the provider body, no $.message and no x-amzn-errortype header (replay: "status: expected 404, got 200").
Why: a deleted credential provider still resolves, so name reuse and delete-then-verify flows both misbehave.
Fix: DeleteApiKeyCredentialProvider's handler/state so the identity store drops the entry. Hypothesis: the delete is the unimplemented scaffold already tracked by open issue 20261108T132909Z-bedrock-agentcore-control-deleteapikeycredentialprovider-stub.md, which would explain why nothing is removed; this entry records the observable read-path divergence, which that stub issue's replay key does not cover. Related but distinct open issue: 20261030T221315Z-bedrock-agentcore-control-browser-lifecycle-statuses-post-delete-reads-and-conflict.md covers post-delete reads for browsers only.

Traces: bedrock-agentcore-control-policy-engine-lifecycle#teardown#4, bedrock-agentcore-control-policy-error-paths#teardown#9, bedrock-agentcore-control-policy-lifecycle#task#6, bedrock-agentcore-control-policy-lifecycle#teardown#8, bedrock-agentcore-control-api-key-credential-provider-lifecycle#teardown#8.

## Evidence
Cause 1 (policy-engine-lifecycle#teardown#4) GetPolicyEngine - expected 404, got 200; body {policyEngineId, policyEngineArn, status: 'ACTIVE', statusReasons: []}; 'poll: status 200 not in [404] after 37 attempt(s)', poll capped at 180s.
Cause 2 (policy-error-paths#teardown#9) GetPolicy - expected 404, got 200; body {policyId, policyEngineId, status: 'ACTIVE', enforcementMode: 'ACTIVE', definition.cedar.statement present}; 'poll: status 200 not in [404] after 37 attempt(s)'.
Cause 3 (policy-lifecycle#task#6) GetPolicy - expected 404 with $.message 'Policy with ID <policyId> not found' and x-amzn-errortype present; got 200, $.message missing, x-amzn-errortype missing, 'poll: error code None not in [ResourceNotFoundException] after 37 attempt(s)'.
Cause 4 (policy-lifecycle#teardown#8) GetPolicyEngine - expected 404 with $.message 'Policy engine with ID <policyEngineId> not found'; got 200 with the engine body, $.message missing, x-amzn-errortype missing.
Cause 5 (api-key-credential-provider-lifecycle#teardown#8) GetApiKeyCredentialProvider - expected 404 / x-amzn-errortype 'ResourceNotFoundException' / $.message 'ApiKeyCredentialProvider not found for <name>'; got 200 with the provider body and neither field.

## Steps to Reproduce
1. Replay fixture bedrock-agentcore-control-policy-engine-lifecycle: CreatePolicyEngine, DeletePolicyEngine, then GetPolicyEngine (step teardown#4) - the read answers 200 ACTIVE instead of 404 ResourceNotFoundException and keeps doing so for the whole 180s poll.
2. Replay bedrock-agentcore-control-policy-error-paths step teardown#9 and bedrock-agentcore-control-policy-lifecycle steps task#6 / teardown#8 for the policy and policy-engine reads after their deletes.
3. Replay bedrock-agentcore-control-api-key-credential-provider-lifecycle step teardown#8: CreateApiKeyCredentialProvider, DeleteApiKeyCredentialProvider, GetApiKeyCredentialProvider - the provider is still returned.

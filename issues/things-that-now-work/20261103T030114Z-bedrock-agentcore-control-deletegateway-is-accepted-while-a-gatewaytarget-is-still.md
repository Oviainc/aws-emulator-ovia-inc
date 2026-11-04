Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.gateway-delete-with-target-conflict-rejected
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control.gateway-delete-with-target-conflict-rejected|_error#12|AssertionError|raise AssertionError("expected the call to raise a ClientError, but it succeeded")
Issue-Commit: fb8daf436f9fd9e940d981e3a914142bad2da5ee

# DeleteGateway is accepted while a GatewayTarget is still attached, instead of being refused with the has-targets ValidationException (1 cause)

> _Note: this issue was identified against the code base at commit `fb8daf436f9fd9e940d981e3a914142bad2da5ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteGateway

## Description
Cause 1 of 1: DeleteGateway does not enforce the dependent-children precondition — a gateway that still has at least one GatewayTarget attached is deleted instead of refused.

NOTE: the open issue 20261029T212239Z-bedrock-agentcore-control-accepts-four-requests-aws-refuses.md describes this same accept/refuse divergence as its Cause 1 and lists this check among its trace ids, but the replay suite does NOT report this check as owned by it (the recorded replay key for the check no longer matches that issue's Issue-Key line, which names `_error_code#12` while the check now fails in `_error#12`). It is therefore filed here so the failure is not lost; a fixer closing either should re-replay the check id below.

AWS: Smithy DeleteGateway — input `gatewayIdentifier` (@required, @httpLabel, pattern ^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$), DELETE /gateways/{gatewayIdentifier}/, success code 202 with {gatewayId, status}; modeled errors include ValidationException (httpError 400) and ConflictException (httpError 409). GatewayTarget is a child resource of the gateway (ListGatewayTargets is bound under /gateways/{gatewayIdentifier}/targets/). Recorded behavior: while at least one GatewayTarget exists on the gateway, DeleteGateway is refused — ValidationException (HTTP 400) with the message "Gateway with ID: <gatewayId> has targets associated with it. Delete all targets before deleting the gateway." (or ConflictException). The fixed text is "Gateway with ID: " ... " has targets associated with it. Delete all targets before deleting the gateway."; the only variable part is the gateway id taken from the request path. The refused call changes nothing: the gateway and its target both stay READY. After every target is deleted, the identical DeleteGateway call is accepted and the gateway eventually stops reading (ResourceNotFoundException).

Emulator: the call succeeds — no ClientError is raised at all (the check's `_error` helper reports "expected the call to raise a ClientError, but it succeeded"); the emulator answers the 202 success shape and begins deleting the gateway while the target is still attached, so no 400/409 refusal is ever produced and the child target is orphaned or silently cascaded.

Why: a client relying on AWS's documented ordering (delete all targets, then the gateway) loses its protection — a mistaken or racing DeleteGateway destroys gateway configuration AWS would have preserved. Code that catches the has-targets ValidationException to drive a "delete children first" cleanup loop (exactly what the suite's own teardown helper does) never takes that branch, and any test asserting the precondition passes against the emulator and fails against AWS.

Fix: a dependent-children guard in the DeleteGateway handler's validation for bedrock-agentcore-control — enumerate the gateway's targets from gateway-target state before the delete is accepted and emit the modeled ValidationException (400) with AWS's has-targets message (gateway id interpolated from the request) while any target exists, leaving gateway and target status untouched.

Traces: bedrock-agentcore-control.gateway-delete-with-target-conflict-rejected.

## Evidence
Check bedrock-agentcore-control.gateway-delete-with-target-conflict-rejected [mismatch]:
  File "check_bedrock_agentcore_control_gateway_delete_with_target_conflict_rejected.py", line 77, in run
    code, message = _error(lambda: control.delete_gateway(gatewayIdentifier=gateway_id))
  File "...", line 196, in _error
    raise AssertionError("expected the call to raise a ClientError, but it succeeded")
  AssertionError: expected the call to raise a ClientError, but it succeeded

Expected (AWS): HTTP 400 ValidationException, message "Gateway with ID: <gatewayId> has targets associated with it. Delete all targets before deleting the gateway." (fixed text as quoted; <gatewayId> is the id from the request path), or HTTP 409 ConflictException; gateway and target both remain status READY.
Actual (emulator): DeleteGateway returns its 202 success response (no error), so the gateway delete proceeds with the target still attached.
The generated gateway/target names and ids in any single run are per-run values filling the message template, not part of the requirement.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (check bedrock-agentcore-control.gateway-delete-with-target-conflict-rejected).
2. Create an IAM role trusted by bedrock-agentcore.amazonaws.com.
3. CreateGateway(name=<unique>, roleArn=<role>, authorizerType=AWS_IAM) and poll GetGateway until status is READY.
4. CreateGatewayTarget(gatewayIdentifier=<gatewayId>, name=<unique>, targetConfiguration={http:{passthrough:{endpoint:"https://example.com", protocolType:"CUSTOM"}}}, credentialProviderConfigurations=[{credentialProviderType:"GATEWAY_IAM_ROLE", credentialProvider:{iamCredentialProvider:{service:"execute-api"}}}]); poll GetGateway and GetGatewayTarget until both are READY.
5. Call DeleteGateway(gatewayIdentifier=<gatewayId>) while the target still exists — observe the 202 success instead of a 400 ValidationException carrying the has-targets message (and observe the gateway no longer staying READY).
6. For contrast: DeleteGatewayTarget, wait until the target no longer reads, then DeleteGateway — that identical call is the one AWS accepts.

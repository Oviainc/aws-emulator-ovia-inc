Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.gateway-target-lifecycle-transitions
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|UpdateGatewayTarget|ValidationException
Issue-Commit: 928c2eb9d028400e3e243ee9e4c9886012c2d3bc

# UpdateGatewayTarget refuses a request that omits the optional credentialProviderConfigurations (ValidationException "Credential provider configurations is not defined"), so a gateway target created without one can never be updated

> _Note: this issue was identified against the code base at commit `928c2eb9d028400e3e243ee9e4c9886012c2d3bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateGatewayTarget

## Description
Cause 1 of 1: `UpdateGatewayTarget` treats the OPTIONAL input member `credentialProviderConfigurations` as mandatory and refuses any update that omits it, which makes every gateway target created without an explicit credential provider permanently unupdatable.

AWS: Smithy model services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json — `UpdateGatewayTarget` input (`UpdateGatewayTargetRequest`) marks only `gatewayIdentifier`, `targetId` and `targetConfiguration` as `@required`; `credentialProviderConfigurations` (CredentialProviderConfigurations, @length min 1 max 1) is NOT required on the input. It is `@required` only on the RESPONSE shape, exactly as it is on `CreateGatewayTargetRequest`/`CreateGatewayTargetResponse` — i.e. the same optional-in / required-out pair that `CreateGatewayTarget` already resolves by substituting the service's effective (default) credential provider. The modeled doc for the member says only "The updated credential provider configurations for the gateway target"; nothing in the model states the member must be present. AWS therefore accepts an `UpdateGatewayTarget` whose body omits it: HTTP 202 with the target moving through `UPDATING` back to `READY` and the response reporting the target's effective credential provider configuration (recorded behavior: the check performs exactly this update and then expects the target to settle at `READY` with the new `targetConfiguration.http.passthrough.endpoint` visible on `GetGatewayTarget`).

Emulator: `UpdateGatewayTarget` answers HTTP 400 `ValidationException` with message `Credential provider configurations is not defined` for a body that carries `gatewayIdentifier`, `targetId`, `name` and `targetConfiguration` but no `credentialProviderConfigurations`. The whole message is fixed text authored by the emulator — there is no variable part taken from the request or state. The refusal is unconditional for that shape of request, and it is raised BEFORE any state is written, so the update is wholly lost. Note the internal inconsistency: `CreateGatewayTarget` in the same crate accepts the identical omission and stores `crate::state::effective_credential_provider_configurations(input.credential_provider_configurations)`, while `UpdateGatewayTarget` refuses it.

Why: a client that creates a target with just a `targetConfiguration` (the common MCP passthrough case) gets a `READY` target it can never reconfigure — every subsequent `UpdateGatewayTarget` is refused 400, so endpoint/protocol/description changes, and any client or CDK/Terraform-style reconcile loop that re-sends the resource's declared (credential-provider-free) body, fail permanently. Callers also see a refusal AWS never emits, with an error message AWS does not use for this path, so error handling keyed on AWS's wording never matches.

Fix: the operation's validation in its handler — services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/update_gateway_target.rs. Drop the required-member check on `credential_provider_configurations` and resolve the omitted member the way `CreateGatewayTarget` already does (the shared `crate::state::effective_credential_provider_configurations` helper), so the required-on-output member is always renderable without refusing a modeled-optional input. Keep the `@length 1..1` constraint check for a list that IS supplied.

Traces: failing check bedrock-agentcore-control.gateway-target-lifecycle-transitions (error: `botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the UpdateGatewayTarget operation: Credential provider configurations is not defined`). The check's later assertions (target status independence from the parent gateway update, the post-delete `ResourceNotFoundException` read and mutation contracts) were never reached because of this refusal and are not claimed here.

## Evidence
Check: bedrock-agentcore-control.gateway-target-lifecycle-transitions (verdict: error)

Sequence performed by the check:
  1. CreateGateway(name=<unique>, roleArn=<role>, authorizerType=AWS_IAM) -> accepted, polled out of CREATING.
  2. CreateGatewayTarget(gatewayIdentifier=<gw>, name=<unique>, targetConfiguration={"http":{"passthrough":{"endpoint":"https://example.com/mcp-v1","protocolType":"MCP"}}})
     -> ACCEPTED by the emulator with NO credentialProviderConfigurations; target settled at READY.
  3. UpdateGateway(... description=...) -> accepted.
  4. UpdateGatewayTarget(gatewayIdentifier=<gw>, targetId=<tgt>, name=<same name>, targetConfiguration={"http":{"passthrough":{"endpoint":"https://example.com/mcp-v2","protocolType":"MCP"}}})

Expected (AWS, per the model's optional input member and the check's recorded expectation):
  HTTP 202, status transitions UPDATING -> READY, and
  GetGatewayTarget(...)["targetConfiguration"]["http"]["passthrough"]["endpoint"] == "https://example.com/mcp-v2"

Actual (emulator):
  HTTP 400
  Code:    ValidationException
  Message: "Credential provider configurations is not defined"   (entire string is fixed emulator-authored text; no request- or state-derived part)
  botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the UpdateGatewayTarget operation: Credential provider configurations is not defined

Model evidence (smithy_query query="operation" operation="UpdateGatewayTarget,CreateGatewayTarget"):
  UpdateGatewayTargetRequest.credentialProviderConfigurations -> {"target":"CredentialProviderConfigurations","length":{"min":1,"max":1},"type":"list"}  (no "required": true)
  UpdateGatewayTargetResponse.credentialProviderConfigurations -> {... "required": true ...}
  CreateGatewayTargetRequest.credentialProviderConfigurations -> optional, likewise; response required.

Code evidence:
  update_gateway_target.rs: `let Some(credential_provider_configurations) = input.credential_provider_configurations.filter(|configured| !configured.is_empty()) else { return Err(... ValidationException(crate::state::member_not_defined(crate::state::CREDENTIAL_PROVIDER_CONFIGURATIONS_PROSE)) ...) };`
  create_gateway_target.rs: `credential_provider_configurations: crate::state::effective_credential_provider_configurations(input.credential_provider_configurations),`  (same omission defaulted, not refused)

## Steps to Reproduce
1. Build and boot the emulator; point a botocore bedrock-agentcore-control client at it.
2. iam.CreateRole(...) a role trusted by bedrock-agentcore.amazonaws.com and attach a permissive policy; take its Arn.
3. bac.create_gateway(name=<unique<=48 chars>, roleArn=<role arn>, authorizerType="AWS_IAM"); poll get_gateway until status != "CREATING".
4. bac.create_gateway_target(gatewayIdentifier=<gatewayId>, name=<unique>, targetConfiguration={"http": {"passthrough": {"endpoint": "https://example.com/mcp-v1", "protocolType": "MCP"}}})  — note: NO credentialProviderConfigurations. This is accepted; poll get_gateway_target until status == "READY".
5. bac.update_gateway_target(gatewayIdentifier=<gatewayId>, targetId=<targetId>, name=<same name>, targetConfiguration={"http": {"passthrough": {"endpoint": "https://example.com/mcp-v2", "protocolType": "MCP"}}})  — again no credentialProviderConfigurations.
6. Observe HTTP 400 ValidationException "Credential provider configurations is not defined" instead of HTTP 202 and the target transitioning UPDATING -> READY with the v2 endpoint.

Equivalently: run the behavioral check bedrock-agentcore-control.gateway-target-lifecycle-transitions.

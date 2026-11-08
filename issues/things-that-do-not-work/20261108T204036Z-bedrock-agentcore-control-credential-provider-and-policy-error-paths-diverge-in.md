Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-apikey-credential-provider-errors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-apikey-credential-provider-errors bedrock-agentcore-control-payment-credential-provider-errors bedrock-agentcore-control-policy-error-paths bedrock-agentcore-control-not-found-error-paths bedrock-agentcore-control-gateway-rate-limit-workflow
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|CreateApiKeyCredentialProvider|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|CreateApiKeyCredentialProvider|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|CreateApiKeyCredentialProvider|4xx->4xx|status
Issue-Key: bedrock-agentcore-control|CreatePaymentCredentialProvider|4xx->2xx|$.message
Issue-Key: bedrock-agentcore-control|CreatePaymentCredentialProvider|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|CreatePaymentCredentialProvider|4xx->2xx|status
Issue-Key: bedrock-agentcore-control|CreatePolicy|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|GetPolicy|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|GetGatewayRateLimit|4xx->4xx|$.message
Issue-Commit: f392f82803b1854878505550560cb55ee4e9c61d

# Credential-provider and policy error paths diverge in status, error code and message

> _Note: this issue was identified against the code base at commit `f392f82803b1854878505550560cb55ee4e9c61d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApiKeyCredentialProvider

## Description
This bundle has 5 causes, all on refusal paths of the identity, policy and gateway rate-limit surfaces: one answers the wrong HTTP status AND error code, one does not refuse at all, and three reword or mis-target the error message.

Cause 1 of 5 - CreateApiKeyCredentialProvider on a duplicate name (fixture bedrock-agentcore-control-apikey-credential-provider-errors, step task#2).
AWS: creating a second API-key credential provider with a name already in the account is refused with 400 ValidationException, x-amzn-errortype: ValidationException, message = fixed text 'Credential provider with name: ' + the requested name + ' already exists' (Smithy: ValidationException @httpError 400 is modeled on CreateApiKeyCredentialProvider; ConflictException 409 is modeled too, but the recording shows AWS classifies the duplicate name as a validation failure).
Emulator: 409 with x-amzn-errortype: ConflictException and the message 'an API key credential provider named <name> already exists in this account'.
Why: a client branching on status/code (retry vs. surface-to-user) takes the conflict branch for what AWS reports as an input validation error, and the human-facing sentence does not match.
Fix: CreateApiKeyCredentialProvider's duplicate-name check - raise ValidationException (400) with AWS's sentence; only the name is a value from the request, the rest of the sentence is fixed text.

Cause 2 of 5 - CreatePaymentCredentialProvider with a malformed secret (fixture bedrock-agentcore-control-payment-credential-provider-errors, step task#1).
AWS: a providerConfigurationInput whose apiKeySecret is not a base64-encoded Ed25519 private key is refused with 400 ValidationException, x-amzn-errortype: ValidationException, message 'Invalid apiKeySecret format: Expected base64-encoded Ed25519 private key' (fixed text, no request values; Smithy: CreatePaymentCredentialProvider input providerConfigurationInput -> PaymentProviderConfigurationInput union {coinbaseCdpConfiguration, stripePrivyConfiguration}, @required; ValidationException @httpError 400 modeled).
Emulator: accepts the request and answers 201 with a created provider - no $.message, no x-amzn-errortype.
Why: a client's negative test for a bad key passes, and a provider holding an unusable secret is created, so the failure surfaces later (or never) instead of at the create call.
Fix: CreatePaymentCredentialProvider's input validation - decode and check the vendor secret before creating state.

Cause 3 of 5 - CreatePolicy on a duplicate name (fixture bedrock-agentcore-control-policy-error-paths, step task#5).
AWS: 409 ConflictException (Smithy: CreatePolicy errors include ConflictException @httpError 409) with the message 'Policy with the same name already exists' - entirely fixed text, naming no identifiers.
Emulator: right status and code, but the message is 'a policy named <name> already exists in this account (policyId <policyId> of policy engine <policyEngineId>)', i.e. a reworded sentence that additionally leaks the existing policy's and engine's ids.
Why: callers and tests that match AWS's sentence do not match, and the emulator discloses ids AWS deliberately omits from this refusal.
Fix: CreatePolicy's conflict path message.

Cause 4 of 5 - GetPolicy for a policy that does not exist (fixture bedrock-agentcore-control-not-found-error-paths, step task#4).
AWS: 404 ResourceNotFoundException whose message names the POLICY that was not found: fixed text 'Policy with ID ' + the requested policyId + ' not found'.
Emulator: 404 but the message names the policy ENGINE from the path instead - 'Policy engine with ID <policyEngineId> not found'.
Why: the refusal points the caller at the wrong resource, so a client cannot tell 'this engine is gone' from 'this policy is gone' and retries against the wrong id.
Fix: GetPolicy's not-found path - once the request reaches the policy lookup, report the policy id with the policy template.

Cause 5 of 5 - GetGatewayRateLimit for a missing rate limit (fixture bedrock-agentcore-control-gateway-rate-limit-workflow, step task#4).
AWS: 404 ResourceNotFoundException with the message 'Rate limit not found: ' + the requested rateLimitId (fixed text plus one value from the request; Smithy: GetGatewayRateLimit errors include ResourceNotFoundException @httpError 404).
Emulator: 404 with a generic sentence - "Failed to retrieve rate limit because it doesn't exist. Retry the request with a different resource identifier." - which names no identifier.
Why: the message loses the rate-limit id, so logs and client-side matching on AWS's wording break.
Fix: GetGatewayRateLimit's not-found path message template.

Related open issue (different paths, not a duplicate): 20261029T212239Z-bedrock-agentcore-control-refusal-messages-diverge-from-aws.md covers runtime not-found, gateway duplicate-name, gateway invalid-state and DoubleEncoded paths.

Traces: bedrock-agentcore-control-apikey-credential-provider-errors#task#2, bedrock-agentcore-control-payment-credential-provider-errors#task#1, bedrock-agentcore-control-policy-error-paths#task#5, bedrock-agentcore-control-not-found-error-paths#task#4, bedrock-agentcore-control-gateway-rate-limit-workflow#task#4.

## Evidence
Cause 1 (apikey-credential-provider-errors#task#2) CreateApiKeyCredentialProvider - expected 400 / x-amzn-errortype 'ValidationException' / $.message 'Credential provider with name: <name> already exists'; got 409 / 'ConflictException' / 'an API key credential provider named <name> already exists in this account'.
Cause 2 (payment-credential-provider-errors#task#1) CreatePaymentCredentialProvider - expected 400 / 'ValidationException' / 'Invalid apiKeySecret format: Expected base64-encoded Ed25519 private key'; got 201 with $.message missing and x-amzn-errortype missing.
Cause 3 (policy-error-paths#task#5) CreatePolicy - status 409 matches; $.message expected 'Policy with the same name already exists', got 'a policy named <name> already exists in this account (policyId <policyId> of policy engine <policyEngineId>)'.
Cause 4 (not-found-error-paths#task#4) GetPolicy - status 404 matches; $.message expected 'Policy with ID <policyId> not found', got 'Policy engine with ID <policyEngineId> not found'.
Cause 5 (gateway-rate-limit-workflow#task#4) GetGatewayRateLimit - status 404 matches; $.message expected 'Rate limit not found: <rateLimitId>', got "Failed to retrieve rate limit because it doesn't exist. Retry the request with a different resource identifier.".
In each expected message the quoted sentence is fixed text; the names and ids are values taken from the request (provider name, policy id, policy-engine id, rate-limit id) and are not part of the template.

## Steps to Reproduce
1. Replay bedrock-agentcore-control-apikey-credential-provider-errors step task#2: create an API-key credential provider, then create it again with the same name - the emulator answers 409 ConflictException instead of 400 ValidationException.
2. Replay bedrock-agentcore-control-payment-credential-provider-errors step task#1: CreatePaymentCredentialProvider with an apiKeySecret that is not a base64-encoded Ed25519 private key - the emulator answers 201 instead of 400 ValidationException.
3. Replay bedrock-agentcore-control-policy-error-paths step task#5 (duplicate CreatePolicy), bedrock-agentcore-control-not-found-error-paths step task#4 (GetPolicy for a missing policy) and bedrock-agentcore-control-gateway-rate-limit-workflow step task#4 (GetGatewayRateLimit for a missing limit) and compare $.message to the recorded sentences.

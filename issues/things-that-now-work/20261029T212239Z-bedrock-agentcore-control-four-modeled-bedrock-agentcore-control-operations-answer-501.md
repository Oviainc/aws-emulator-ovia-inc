Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.batch-put-gateway-rate-limits-concurrent-writers-consistent bedrock-agentcore-control.create-policy-nonexistent-engine-rejected bedrock-agentcore-control.delete-payment-credential-provider-nonexistent-not-found bedrock-agentcore-control.delete-resource-policy-nonexistent-resource-not-found
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control.batch-put-gateway-rate-limits-concurrent-writers-consistent|run#14|AssertionError|assert any(ok for ok, _ in results), f"both concurrent batch writers were rejected: {results}"
Issue-Key: bedrock-agentcore-control.create-policy-nonexistent-engine-rejected|run#8|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: bedrock-agentcore-control.delete-payment-credential-provider-nonexistent-not-found|run#4|AssertionError|assert get_code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {get_code}"
Issue-Key: bedrock-agentcore-control.delete-resource-policy-nonexistent-resource-not-found|run#10|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# Four modeled bedrock-agentcore-control operations answer 501 NotImplemented: BatchPutGatewayRateLimits, CreatePolicy, GetPaymentCredentialProvider, DeleteResourcePolicy (4 causes)

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchPutGatewayRateLimits

## Description
Four operations that the service model binds are still scaffold stubs: each answers HTTP 501 with error code NotImplemented and the message 'operation <Name> is scaffolded but not implemented in this emulator'. These four are NOT the stubs the suite auto-filed — in these checks the 501 was swallowed by the check's own error handler, so the replay reported a code mismatch instead of a stub hit, and no issue names them. (Checks blocked by CreateBrowser, CreateApiKeyCredentialProvider, CreateConfigurationBundle, CreateDataset, CreateOauth2CredentialProvider, DeleteMemory, PutResourcePolicy and UpdateMemory are deliberately NOT included here: those operations already have their own auto-filed stub issues.) 4 causes must all be fixed before this issue is closed.

Cause 1 of 4: BatchPutGatewayRateLimits is unimplemented.
AWS: Smithy BatchPutGatewayRateLimits — PUT /gateways/{gatewayIdentifier}/rate-limits/batch, HTTP 200, input {gatewayIdentifier @required @httpLabel, rateLimits @required (BatchPutLimitEntries, @length 1..50), clientToken @idempotencyToken}, output {rateLimits @required}; modeled errors AccessDeniedException 403, ConflictException 409, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ThrottlingException 429, ValidationException 400, InternalServerException 500. It upserts the named rate limits on the gateway; concurrent writers of the same rateLimitId either both succeed or one is refused 409 ConflictException, and the stored entries always equal exactly one submitted list (never a merge).
Emulator: 501 NotImplemented, so no rate limit is ever written and the concurrency contract cannot be exercised at all.
Why: a caller cannot configure gateway rate limits through the batch path; 501/NotImplemented is not a modeled error, so SDK retry and error handling see an unknown failure class.
Fix: implement the operation handler, services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/batch_put_gateway_rate_limits.rs, over the gateway rate-limit state (upsert by rateLimitId, last-writer-wins per whole entry list).
Traces: bedrock-agentcore-control.batch-put-gateway-rate-limits-concurrent-writers-consistent.

Cause 2 of 4: CreatePolicy is unimplemented.
AWS: Smithy CreatePolicy — POST /policy-engines/{policyEngineId}/policies, HTTP 202, input {name @required @pattern ^[A-Za-z][A-Za-z0-9_]*$, definition @required, policyEngineId @required @httpLabel, validationMode default FAIL_ON_ANY_FINDINGS, enforcementMode default ACTIVE, clientToken}, output {policyId, name, policyEngineId, policyArn, status, definition, statusReasons, createdAt, updatedAt}; modeled errors include ResourceNotFoundException 404 and ValidationException 400. Recorded behavior: a policyEngineId that references no policy engine is refused 404 ResourceNotFoundException with the message "Policy engine with ID <policyEngineId> not found" — fixed text "Policy engine with ID " and " not found", the variable part being the id from the request path — and a real engine accepts the same definition.
Emulator: 501 NotImplemented for every CreatePolicy, so neither the refusal nor the create exists.
Why: the whole policy plane is unusable, and the distinctive missing-engine 404 a client keys on is unreachable.
Fix: implement services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/create_policy.rs, including the policy-engine existence check that produces the modeled ResourceNotFoundException.
Traces: bedrock-agentcore-control.create-policy-nonexistent-engine-rejected.

Cause 3 of 4: GetPaymentCredentialProvider is unimplemented.
AWS: Smithy GetPaymentCredentialProvider — POST /identities/GetPaymentCredentialProvider, HTTP 200, @readonly, input {name @required @length 1..128 @pattern ^[a-zA-Z0-9\\-_]+$}, output {name, credentialProviderArn, credentialProviderVendor, providerConfigurationOutput, createdTime, lastUpdatedTime, tags}; modeled errors include ResourceNotFoundException 404. Recorded behavior: a name that was never created is refused 404 ResourceNotFoundException with the fixed message "PaymentCredentialProvider not found" (no variable parts).
Emulator: 501 NotImplemented, so a caller cannot read a payment credential provider or distinguish a missing one.
Why: the not-found signal is how a client decides whether to create the provider; 501 makes both the read and that decision impossible.
Fix: implement services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/get_payment_credential_provider.rs over token-vault credential-provider state, emitting the modeled ResourceNotFoundException for an unknown name.
Traces: bedrock-agentcore-control.delete-payment-credential-provider-nonexistent-not-found.

Cause 4 of 4: DeleteResourcePolicy is unimplemented.
AWS: Smithy DeleteResourcePolicy — DELETE /resourcepolicy/{resourceArn}, HTTP 204 with an empty output, input {resourceArn @required @httpLabel @length 20..1011}; modeled errors AccessDeniedException 403, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400, InternalServerException 500. Recorded behavior: for a resource that exists but has no attached resource-based policy the call is refused 404 ResourceNotFoundException with the fixed message "Resource Policy doesn't exist for requested arn" (no variable parts), and it succeeds once a policy has been attached.
Emulator: 501 NotImplemented, so an attached resource policy can never be removed and the no-policy-vs-no-resource distinction is unobservable.
Why: resource-policy lifecycle (put/get/delete) is left half-open; a client cannot revoke access it granted.
Fix: implement services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/delete_resource_policy.rs over the resource-policy store, returning 204 on success and the modeled 404 when no policy is attached.
Traces: bedrock-agentcore-control.delete-resource-policy-nonexistent-resource-not-found.

## Evidence
Every cause below is the same emulator response: HTTP 501, Error.Code 'NotImplemented', Error.Message 'operation <Name> is scaffolded but not implemented in this emulator'. NotImplemented is not in any of these operations' modeled error sets.
Cause 1 — bedrock-agentcore-control.batch-put-gateway-rate-limits-concurrent-writers-consistent: 'both concurrent batch writers were rejected: [(False, "NotImplemented"), (False, "NotImplemented")]'; expected at least one 200 and any loser refused ConflictException.
Cause 2 — bedrock-agentcore-control.create-policy-nonexistent-engine-rejected: "expected ResourceNotFoundException, got 'NotImplemented'"; expected 404 with 'Policy engine with ID <policyEngineId> not found'.
Cause 3 — bedrock-agentcore-control.delete-payment-credential-provider-nonexistent-not-found: 'expected ResourceNotFoundException, got NotImplemented' on GetPaymentCredentialProvider(name=<never-created name>); expected 404 'PaymentCredentialProvider not found'.
Cause 4 — bedrock-agentcore-control.delete-resource-policy-nonexistent-resource-not-found: 'expected ResourceNotFoundException, got NotImplemented' on DeleteResourcePolicy(resourceArn=<existing gateway ARN with no policy>); expected 404 'Resource Policy doesn't exist for requested arn'.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Cause 1: create a gateway, wait for READY, then PUT /gateways/{gatewayIdentifier}/rate-limits/batch (BatchPutGatewayRateLimits) with one rateLimits entry (check batch-put-gateway-rate-limits-concurrent-writers-consistent).
3. Cause 2: call CreatePolicy with policyEngineId set to an id no policy engine has (check create-policy-nonexistent-engine-rejected).
4. Cause 3: call GetPaymentCredentialProvider with a name that was never created (check delete-payment-credential-provider-nonexistent-not-found).
5. Cause 4: create a gateway, then call DeleteResourcePolicy with its ARN before any PutResourcePolicy (check delete-resource-policy-nonexistent-resource-not-found).

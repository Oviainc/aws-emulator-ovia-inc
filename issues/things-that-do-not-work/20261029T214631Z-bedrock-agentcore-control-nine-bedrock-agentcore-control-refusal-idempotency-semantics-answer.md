Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.browser-vpc-network-mode-requires-real-vpc-config bedrock-agentcore-control.create-api-key-credential-provider-decryption-failure-on-bad-secret bedrock-agentcore-control.create-configuration-bundle-parallel-create-idempotency bedrock-agentcore-control.create-dataset-version-invalid-schema-rejected bedrock-agentcore-control.create-oauth2-credential-provider-invalid-scope-rejected bedrock-agentcore-control.create-oauth2-credential-provider-malformed-input-rejected bedrock-agentcore-control.delete-memory-nonexistent-returns-resource-not-found bedrock-agentcore-control.put-resource-policy-nonexistent-resource-not-found bedrock-agentcore-control.update-memory-invalid-state-transition-rejected
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control.browser-vpc-network-mode-requires-real-vpc-config|run#5|AssertionError|assert code == "ValidationException", (
Issue-Key: bedrock-agentcore-control.create-api-key-credential-provider-decryption-failure-on-bad-secret|run#14|AssertionError|assert code == "DecryptionFailure", f"expected DecryptionFailure, got {code}"
Issue-Key: bedrock-agentcore-control.create-configuration-bundle-parallel-create-idempotency|run#8|AssertionError|assert not failures, f"expected every replayed idempotent create to succeed, got failures: {failures}"
Issue-Key: bedrock-agentcore-control.create-dataset-version-invalid-schema-rejected|run#6|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: bedrock-agentcore-control.create-oauth2-credential-provider-invalid-scope-rejected|run#4|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: bedrock-agentcore-control.create-oauth2-credential-provider-malformed-input-rejected|run#4|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: bedrock-agentcore-control.delete-memory-nonexistent-returns-resource-not-found|run#8|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: bedrock-agentcore-control.put-resource-policy-nonexistent-resource-not-found|run#6|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: bedrock-agentcore-control.update-memory-invalid-state-transition-rejected|run#10|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Commit: 7240e32f2500d20910730c71d4ad0714ff1f3217

# Nine bedrock-agentcore-control refusal/idempotency semantics answer 501 NotImplemented instead of the modeled error code, message and effect

> _Note: this issue was identified against the code base at commit `7240e32f2500d20910730c71d4ad0714ff1f3217`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBrowser, CreateApiKeyCredentialProvider, CreateConfigurationBundle, CreateDataset, CreateOauth2CredentialProvider, DeleteMemory, PutResourcePolicy, UpdateMemory

## Description
Nine behavioral checks in bedrock-agentcore-control fail because the operation under test answers HTTP 501 with error code `NotImplemented` and message `operation <Op> is scaffolded but not implemented in this emulator`, where AWS answers a modeled, request-shaped refusal (or, for the idempotency cause, a success). None of these nine checks is marked ALREADY FILED or OWNED in the replay report, so none is carried by an existing work item. Open scaffold-stub issues exist for most of these operations (20261029T212239Z-bedrock-agentcore-control-createbrowser-stub.md, -createapikeycredentialprovider-stub.md, -createconfigurationbundle-stub.md, -createdataset-stub.md, -createoauth2credentialprovider-stub.md, -deletememory-stub.md, -putresourcepolicy-stub.md, -updatememory-stub.md); those issues ask only that the operation stop being a stub and do NOT state the error class, the HTTP status or the AWS message wording each of these paths must produce. This issue carries those requirements. There are NINE separate causes below; each must be closed and its named checks re-replayed before submitting.

--- Cause 1 of 9: CreateBrowser with networkMode=VPC and no vpcConfig must be refused ValidationException ---
AWS: Smithy CreateBrowser (PUT /browsers, success code 202) models ValidationException (httpError 400) among its errors; `networkConfiguration` is @required and targets BrowserNetworkConfiguration, whose member documentation states the vpcConfig "configuration is required when the network mode is set to VPC". A CreateBrowser whose networkConfiguration.networkMode is VPC with vpcConfig omitted is therefore refused with HTTP 400 / `ValidationException`, and no browser is created (the name does not appear in ListBrowsers). Message: AWS's constraint-violation wording for a missing conditionally-required member; the fixed text is the validation sentence and the variable part is the member path (`networkConfiguration.vpcConfig`) taken from the request.
Emulator: HTTP 501, code `NotImplemented`, message `operation CreateBrowser is scaffolded but not implemented in this emulator`. Replay assertion: `expected ValidationException for networkMode=VPC with no vpcConfig, got 'NotImplemented'`.
Why: a client that validates its network configuration by catching ValidationException sees an unmodeled 501 instead; botocore raises a generic ClientError that no retry/validation branch matches, and the caller cannot distinguish "my request is malformed" from "the service is broken".
Fix: the CreateBrowser handler and its input validation (reject networkMode=VPC without vpcConfig before any state mutation, so the effect-absent arm also holds).
Traces: bedrock-agentcore-control.browser-vpc-network-mode-requires-real-vpc-config

--- Cause 2 of 9: CreateApiKeyCredentialProvider over an unusable KMS key must be refused DecryptionFailure ---
AWS: Smithy CreateApiKeyCredentialProvider (POST /identities/CreateApiKeyCredentialProvider, success 201) models `DecryptionFailure` (errorKind client, httpError 400). When `apiKeySecretSource` is EXTERNAL and `apiKeySecretConfig.secretId` names a Secrets Manager secret whose KMS key cannot be used to decrypt (key disabled), the call is refused HTTP 400 / `DecryptionFailure` with the message `Unable to decrypt the provided secret. Verify KMS key permissions.` — that message is entirely fixed text, it carries no request- or state-derived values. No provider is left behind under that name, so a subsequent GetApiKeyCredentialProvider for it returns ResourceNotFoundException.
Emulator: HTTP 501, code `NotImplemented`. Replay assertion: `expected DecryptionFailure, got NotImplemented`.
Why: the KMS-unusable condition is the one failure a caller is expected to recover from by fixing key permissions; collapsed into 501 the caller has no signal that the secret's key, not the request, is at fault.
Fix: the CreateApiKeyCredentialProvider handler plus its secret/KMS pre-condition check (resolve the referenced secret's key state before creating the provider).
Traces: bedrock-agentcore-control.create-api-key-credential-provider-decryption-failure-on-bad-secret

--- Cause 3 of 9: CreateConfigurationBundle is not idempotent over clientToken (it refuses every call) ---
AWS: Smithy CreateConfigurationBundle (POST /configuration-bundles/create, success 201) carries `clientToken` with @idempotencyToken. Concurrent calls that share one clientToken and identical `bundleName`/`components` all SUCCEED and all resolve to the same bundle: identical `bundleId` and identical `versionId` in every response, and exactly one matching bundle in ListConfigurationBundles. Recorded behavior only for the duplicate-resolution detail; the model supplies the idempotency contract.
Emulator: every one of the three racing calls is refused HTTP 501 `NotImplemented`. Replay assertion: `expected every replayed idempotent create to succeed, got failures: [(False, 'NotImplemented'), (False, 'NotImplemented'), (False, 'NotImplemented')]`.
Why: a client retrying a create under one idempotency token either cannot create at all, or — once the operation is implemented without token de-duplication — silently gets several bundles for one logical create.
Fix: the CreateConfigurationBundle handler and the bundle state store (clientToken de-duplication keyed on the token plus request parameters, returning the first result).
Traces: bedrock-agentcore-control.create-configuration-bundle-parallel-create-idempotency

--- Cause 4 of 9: CreateDataset must reject a schema-violating inline example batch wholesale with ValidationException ---
AWS: Smithy CreateDataset (POST /datasets, success 202) models ValidationException (httpError 400); `schemaType` and `source` are @required. With schemaType AGENTCORE_EVALUATION_PREDEFINED_V1 and a batch of inlineExamples where one example omits the schema's required `scenario_id`, the whole call is refused HTTP 400 / `ValidationException` and NO dataset is created (not even one holding the valid example). Message: fixed text `Example at index <i> failed schema validation: Required field '<field>' is missing from example content`, where `<i>` is the zero-based position of the offending example in the submitted batch and `<field>` is the schema field it omits — both taken from the request, not from any one recording.
Emulator: HTTP 501, code `NotImplemented`. Replay assertion: `expected ValidationException, got NotImplemented`.
Why: a caller uploading evaluation batches cannot tell which example is malformed, and cannot rely on all-or-nothing commit semantics for the batch.
Fix: the CreateDataset handler's schema validation for the inline-examples source, applied across the whole batch before any dataset is persisted.
Traces: bedrock-agentcore-control.create-dataset-version-invalid-schema-rejected

--- Cause 5 of 9: CreateOauth2CredentialProvider must reject a vendor/config discriminator mismatch with ValidationException ---
AWS: Smithy CreateOauth2CredentialProvider (POST /identities/CreateOauth2CredentialProvider, success 201) models ValidationException (httpError 400); `credentialProviderVendor` (enum) and `oauth2ProviderConfigInput` (the Oauth2ProviderConfigInput union) are both @required. When the declared vendor does not match the union member actually supplied — e.g. vendor `GoogleOauth2` with a `customOauth2ProviderConfig` payload — the call is refused HTTP 400 / `ValidationException`, never reinterpreted, and the provider never appears in ListOauth2CredentialProviders. Message: fixed text `Provided configuration does not match selected type: <vendor>`, where `<vendor>` is the credentialProviderVendor value from the request.
Emulator: HTTP 501, code `NotImplemented`. Replay assertion: `expected ValidationException, got 'NotImplemented'`.
Why: a client that sends a mismatched union arm gets no indication the discriminator is wrong, and cannot distinguish that from a transport failure.
Fix: the CreateOauth2CredentialProvider handler's input validation (cross-check credentialProviderVendor against the supplied Oauth2ProviderConfigInput member).
Traces: bedrock-agentcore-control.create-oauth2-credential-provider-invalid-scope-rejected

--- Cause 6 of 9: CreateOauth2CredentialProvider must enforce the discoveryUrl pattern with a constraint-violation ValidationException ---
AWS: same operation and modeled ValidationException (httpError 400). `oauth2ProviderConfigInput.customOauth2ProviderConfig.oauthDiscovery.discoveryUrl` carries a @pattern requiring a /.well-known/(openid-configuration|oauth-authorization-server) suffix; a URL that does not satisfy it is refused HTTP 400 / `ValidationException` and no provider is created. Message is AWS's standard constraint-violation template: fixed text `<n> validation error detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>`, where `<n>` is the number of violations, `<member path>` is the full member path from the request (`oauth2ProviderConfigInput.customOauth2ProviderConfig.oauthDiscovery.discoveryUrl`) and `<pattern>` is the member's modeled pattern. Note this is a DIFFERENT refusal class from Cause 5 on the same operation: a member-constraint violation rather than a union/discriminator mismatch, and the fix for one does not produce the other's message.
Emulator: HTTP 501, code `NotImplemented`. Replay assertion: `expected ValidationException, got NotImplemented`.
Why: a client cannot learn which member failed which constraint; OAuth onboarding flows that surface the member path to users surface nothing.
Fix: the CreateOauth2CredentialProvider handler's modeled-constraint validation and the shared constraint-violation message serializer.
Traces: bedrock-agentcore-control.create-oauth2-credential-provider-malformed-input-rejected

--- Cause 7 of 9: DeleteMemory on an absent memoryId must be refused ResourceNotFoundException ---
AWS: Smithy DeleteMemory (DELETE /memories/{memoryId}/delete, success 202) models ResourceNotFoundException (errorKind client, httpError 404); `memoryId` is an @httpLabel with pattern ^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$. A well-formed id that does not exist is refused HTTP 404 / `ResourceNotFoundException`, and an unrelated existing memory is untouched and still deletable. Message: fixed text `Resource not found during DeleteMemory: Failed to retrieve memory with ID: <memoryId> for account: <accountId>`, where `<memoryId>` is the id from the request path and `<accountId>` is the caller's account from the signed request — never a literal from one run.
Emulator: HTTP 501, code `NotImplemented`. Replay assertion: `expected ResourceNotFoundException, got NotImplemented`.
Why: delete-if-exists cleanup logic, which keys on ResourceNotFoundException, breaks: the 501 is indistinguishable from a real server fault, so teardown either aborts or leaks.
Fix: the DeleteMemory handler and the memory state lookup (404 path before any mutation).
Traces: bedrock-agentcore-control.delete-memory-nonexistent-returns-resource-not-found

--- Cause 8 of 9: PutResourcePolicy (and GetResourcePolicy) on an absent resource ARN must be refused ResourceNotFoundException ---
AWS: Smithy PutResourcePolicy (PUT /resourcepolicy/{resourceArn}, success 201) models ResourceNotFoundException (errorKind client, httpError 404); `resourceArn` is an @httpLabel (length 20..1011) and `policy` is @required. A syntactically valid ARN naming a resource that does not exist is refused HTTP 404 / `ResourceNotFoundException` with the message `ResourceArn provided in the request does not exist` — entirely fixed text, no interpolated values. GetResourcePolicy for the same ARN is refused identically, proving no policy was stored.
Emulator: HTTP 501, code `NotImplemented`. Replay assertion: `expected ResourceNotFoundException, got NotImplemented`.
Why: resource-policy management tooling cannot tell a missing target resource from an emulator fault, and cannot verify the effect-absent property that no policy was attached.
Fix: the PutResourcePolicy handler (and the paired GetResourcePolicy read) plus the resource-ARN existence check against service state.
Traces: bedrock-agentcore-control.put-resource-policy-nonexistent-resource-not-found

--- Cause 9 of 9: UpdateMemory must refuse a namespaceKeys replacement that orphans a referenced namespaceTemplate with ValidationException ---
AWS: Smithy UpdateMemory (PUT /memories/{memoryId}/update, success 202) models ValidationException (httpError 400); `namespaceKeys` (list, max length 5) REPLACES the full set. Omitting a namespaceKey that an existing memory strategy's namespaceTemplate still references is refused HTTP 400 / `ValidationException` — a state-dependent validation distinct from ResourceNotFoundException and from ConflictException — and the memory's original namespaceKeys are left untouched. Once the referencing strategy is retargeted so its namespaceTemplate no longer mentions the key, the identical omission succeeds. Message: AWS's validation wording naming the orphaned namespace key and the referencing strategy/template; the key name, strategy id and template string are values from the request and from the memory's stored state.
Emulator: HTTP 501, code `NotImplemented` (CreateMemory succeeds; UpdateMemory is the stub). Replay assertion: `expected ValidationException, got NotImplemented`.
Why: a caller editing namespace keys gets no feedback that the edit would break a live strategy, and cannot rely on the memory being left unmodified after a rejected update.
Fix: the UpdateMemory handler's state-dependent validation over namespaceKeys versus the memory's strategies' namespaceTemplates, rejecting before any state is written.
Traces: bedrock-agentcore-control.update-memory-invalid-state-transition-rejected

## Evidence
Replay report (simparity-behavioral-aws-v1, 8/125 passed). Each of the nine checks below is verdict `mismatch`, none marked ALREADY FILED or OWNED.

Expected vs actual, per cause:
1. CreateBrowser networkMode=VPC, vpcConfig omitted — expected HTTP 400 `ValidationException` + no browser listed; actual HTTP 501 `NotImplemented` ("operation CreateBrowser is scaffolded but not implemented in this emulator"). Assertion: `expected ValidationException for networkMode=VPC with no vpcConfig, got 'NotImplemented'`.
2. CreateApiKeyCredentialProvider, EXTERNAL secret whose KMS key is disabled — expected HTTP 400 `DecryptionFailure` + message "Unable to decrypt the provided secret. Verify KMS key permissions."; actual HTTP 501 `NotImplemented`. Assertion: `expected DecryptionFailure, got NotImplemented`.
3. CreateConfigurationBundle x3 concurrent, one shared clientToken + identical bundleName/components — expected all three to succeed with one bundleId and one versionId; actual all three refused. Assertion: `expected every replayed idempotent create to succeed, got failures: [(False, 'NotImplemented'), (False, 'NotImplemented'), (False, 'NotImplemented')]`.
4. CreateDataset, schemaType AGENTCORE_EVALUATION_PREDEFINED_V1, batch [valid, example-missing-scenario_id] — expected HTTP 400 `ValidationException` + "Example at index 1 failed schema validation: Required field 'scenario_id' is missing from example content" (index and field name from the request) + no dataset created; actual HTTP 501 `NotImplemented`. Assertion: `expected ValidationException, got NotImplemented`.
5. CreateOauth2CredentialProvider, credentialProviderVendor=GoogleOauth2 with customOauth2ProviderConfig — expected HTTP 400 `ValidationException` + "Provided configuration does not match selected type: GoogleOauth2" (vendor from the request); actual HTTP 501 `NotImplemented`. Assertion: `expected ValidationException, got 'NotImplemented'`.
6. CreateOauth2CredentialProvider, CustomOauth2 with discoveryUrl "https://example.com/not-a-discovery-endpoint" — expected HTTP 400 `ValidationException` + "1 validation error detected: Value at 'oauth2ProviderConfigInput.customOauth2ProviderConfig.oauthDiscovery.discoveryUrl' failed to satisfy constraint: Member must satisfy regular expression pattern: [^\s?#]+/\.well-known/(openid-configuration|oauth-authorization-server)(/[^\s?#]+)?"; actual HTTP 501 `NotImplemented`. Assertion: `expected ValidationException, got NotImplemented`.
7. DeleteMemory on a well-formed absent memoryId — expected HTTP 404 `ResourceNotFoundException` + "Resource not found during DeleteMemory: Failed to retrieve memory with ID: <memoryId> for account: <accountId>"; actual HTTP 501 `NotImplemented`. Assertion: `expected ResourceNotFoundException, got NotImplemented`.
8. PutResourcePolicy on arn:aws:bedrock-agentcore:<region>:<account>:gateway/<absent-id> — expected HTTP 404 `ResourceNotFoundException` + "ResourceArn provided in the request does not exist" (fixed text), and the same from GetResourcePolicy; actual HTTP 501 `NotImplemented`. Assertion: `expected ResourceNotFoundException, got NotImplemented`.
9. UpdateMemory omitting namespaceKeys entry "customattr" while a semanticMemoryStrategy namespaceTemplate "/custom/{customattr}/facts" still references it, on an ACTIVE memory — expected HTTP 400 `ValidationException` + original namespaceKeys unchanged; actual HTTP 501 `NotImplemented` (CreateMemory itself succeeded). Assertion: `expected ValidationException, got NotImplemented`.

Smithy grounding (services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json): CreateBrowser errors include ValidationException(400), networkConfiguration @required; CreateApiKeyCredentialProvider errors include DecryptionFailure(400); CreateConfigurationBundle clientToken @idempotencyToken; CreateDataset errors include ValidationException(400), schemaType/source @required; CreateOauth2CredentialProvider errors include ValidationException(400), credentialProviderVendor + oauth2ProviderConfigInput @required; DeleteMemory errors include ResourceNotFoundException(404); PutResourcePolicy errors include ResourceNotFoundException(404); UpdateMemory errors include ValidationException(400), namespaceKeys list max 5. In every case `NotImplemented`/501 is NOT a modeled error of the operation.

Handler files named by the report's stub diagnostics, all under services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/: create_browser.rs, create_api_key_credential_provider.rs, create_configuration_bundle.rs, create_dataset.rs, create_oauth2_credential_provider.rs, delete_memory.rs, put_resource_policy.rs, update_memory.rs.

## Steps to Reproduce
1. git clone the repo and boot the emulator (as run_behavioral_suite does).
2. Replay these nine checks against bedrock-agentcore-control:
   - bedrock-agentcore-control.browser-vpc-network-mode-requires-real-vpc-config
   - bedrock-agentcore-control.create-api-key-credential-provider-decryption-failure-on-bad-secret
   - bedrock-agentcore-control.create-configuration-bundle-parallel-create-idempotency
   - bedrock-agentcore-control.create-dataset-version-invalid-schema-rejected
   - bedrock-agentcore-control.create-oauth2-credential-provider-invalid-scope-rejected
   - bedrock-agentcore-control.create-oauth2-credential-provider-malformed-input-rejected
   - bedrock-agentcore-control.delete-memory-nonexistent-returns-resource-not-found
   - bedrock-agentcore-control.put-resource-policy-nonexistent-resource-not-found
   - bedrock-agentcore-control.update-memory-invalid-state-transition-rejected
3. Minimal manual reproduction of two representative causes:
   a. aws bedrock-agentcore-control create-browser --name Sim2realVpcNoCfg --network-configuration '{"networkMode":"VPC"}'  ->  emulator answers 501 NotImplemented; AWS answers 400 ValidationException and creates nothing.
   b. aws bedrock-agentcore-control delete-memory --memory-id Sim2realMemDel-0123456789  (an id matching ^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$ that was never created)  ->  emulator answers 501 NotImplemented; AWS answers 404 ResourceNotFoundException with "Resource not found during DeleteMemory: Failed to retrieve memory with ID: <memoryId> for account: <accountId>".
4. For each cause, assert the HTTP status, the error code and the message template (fixed text verbatim, variable parts resolved from the request and from emulator state), and assert the effect-absent property: the refused create/update/put left no resource and mutated no existing resource.

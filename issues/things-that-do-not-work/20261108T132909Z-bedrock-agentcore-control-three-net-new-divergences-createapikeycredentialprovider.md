Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.create-api-key-credential-provider-decryption-failure-on-bad-secret bedrock-agentcore-control.put-resource-policy-refused-for-unsupported-resource-types bedrock-agentcore-control.registry-creation-deterministically-refused-without-entitlement
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control.create-api-key-credential-provider-decryption-failure-on-bad-secret|_error_detail#13|AssertionError|raise AssertionError(f"the call was expected to be refused but it SUCCEEDED and returned {result!r}")
Issue-Key: bedrock-agentcore-control|PutResourcePolicy|ResourceNotFoundException
Issue-Key: bedrock-agentcore-control.registry-creation-deterministically-refused-without-entitlement|_refusal#13|AssertionError|raise AssertionError(
Issue-Commit: 77fc38481d0944218df9b5c2964437ecad54bbf8

# bedrock-agentcore-control: three net-new divergences — CreateApiKeyCredentialProvider accepts an undecryptable EXTERNAL secret, PutResourcePolicy resolves only runtime/gateway ARNs, CreateRegistry is not entitlement-gated

> _Note: this issue was identified against the code base at commit `77fc38481d0944218df9b5c2964437ecad54bbf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApiKeyCredentialProvider, PutResourcePolicy, CreateRegistry

## Description
Three independent root causes in the bedrock-agentcore-control emulator. Each must be closed before submitting.

=== Cause 1 of 3: CreateApiKeyCredentialProvider accepts an EXTERNAL secret whose KMS key cannot decrypt it, instead of refusing with DecryptionFailure ===
Failing check: bedrock-agentcore-control.create-api-key-credential-provider-decryption-failure-on-bad-secret

AWS: Smithy CreateApiKeyCredentialProvider (POST /identities/CreateApiKeyCredentialProvider, success 201) declares the error `DecryptionFailure` with httpError 400 among its modeled errors, alongside EncryptionFailure, ValidationException, ResourceNotFoundException and ConflictException. Its input members `apiKeySecretSource` (SecretSourceType enum) and `apiKeySecretConfig` (SecretReference: secretId + jsonKey) make the provider's key live in a CALLER-owned Secrets Manager secret when the source is EXTERNAL. The model fixes the error shape but not its message text, so for the message: recorded behavior only — when the referenced secret exists but its KMS key cannot be used to decrypt it (e.g. the CMK the secret is encrypted under is in a disabled state), the call is refused with HTTP 400, error code `DecryptionFailure`, message "Unable to decrypt the provided secret. Verify KMS key permissions." That whole sentence is FIXED text — it interpolates nothing from the request (neither the secret id, the key id nor the provider name), so it is the requirement verbatim. The refusal has no effect: no provider is recorded, and a following GetApiKeyCredentialProvider for that name is a 404 ResourceNotFoundException. The identical call succeeds (201) once the same key is usable again, so the refusal is a property of the key's state, not of EXTERNAL secrets in general.

Emulator: the request is ACCEPTED. The replay shows HTTP 201 with a full success body (apiKeySecretArn.secretArn pointing at the caller's secret, apiKeySecretSource "EXTERNAL", apiKeySecretJsonKey "apiKey", name and credentialProviderArn), even though the secret's CMK had just been disabled via kms:DisableKey. No DecryptionFailure is ever produced for this path.

Why: a client that creates a provider over a customer-managed secret is told the provider is usable when the service could not actually read the key. The failure is deferred to first use (or never surfaces), so error handling written against DecryptionFailure — re-grant the KMS key, re-enable it, then retry — is dead code against the emulator, and the "create succeeded" signal is wrong. Conversely a test suite that proves KMS permission handling cannot distinguish "the disabled key caused DecryptionFailure" from "the service does not check at all".

Fix: the EXTERNAL-secret resolution step of the CreateApiKeyCredentialProvider handler and the shared secret-resolution helper it calls. Today the helper only establishes that the referenced secret EXISTS (an absent reference maps to ResourceNotFoundException, any transport problem maps to InternalServerException — see the `ApiKeySecretProblem` classification), so a secret that exists but is undecryptable has no classification at all and falls through to success. The resolution needs a third outcome — "the secret is there but its key will not decrypt it" — mapped to the modeled DecryptionFailure with the fixed sentence above, taken before any provider record is inserted so the refusal leaves no state behind.

=== Cause 2 of 3: PutResourcePolicy resolves only runtime and gateway ARNs, so it 404s on resource types AWS accepts and mis-classifies the types AWS refuses ===
Failing check: bedrock-agentcore-control.put-resource-policy-refused-for-unsupported-resource-types

AWS: Smithy PutResourcePolicy (PUT /resourcepolicy/{resourceArn}, success 201) takes `resourceArn` (BedrockAgentcoreResourceArn, @httpLabel, @required, @length 20..1011) and `policy`, and declares AccessDeniedException (403), ValidationException (400) and ResourceNotFoundException (404). The operation's own documentation says the feature "is currently available only for AgentCore Runtime and Gateway", but recorded behavior is broader and is what a client sees: a Browser ARN and a CodeInterpreter ARN are ACCEPTED just like a Gateway ARN — 201, the response echoes the submitted document, and GetResourcePolicy on the same ARN returns it. The resource types genuinely out of scope are refused, and the class of refusal depends on the ARN: a WorkloadIdentity ARN and an ApiKeyCredentialProvider ARN are refused with `ValidationException`, while an ARN whose resource type is not a real AgentCore resource type at all (e.g. arn:aws:bedrock-agentcore:<region>:<account>:totally-bogus-type/<id>) is refused with `AccessDeniedException`. In every refused case no policy is stored, and policies already attached to other resources are untouched.

Emulator: PutResourcePolicy resolves the ARN through a target lookup that admits exactly two resource types — "runtime" and "gateway" (services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/state/resource_policies.rs, POLICY_TARGET_RUNTIME / POLICY_TARGET_GATEWAY, `resource_policy_target`) — and answers every other ARN with the modeled 404. Replaying the check, a policy put on a READY browser (created by CreateBrowser, read back READY by GetBrowser, policy put with the browser's own `browserArn`) is refused: HTTP 404, code ResourceNotFoundException, message "Failed to retrieve resource because it doesn't exist. Retry the request with a different resource identifier." The same single `None` outcome also means the out-of-scope types collapse into one answer: WorkloadIdentity and ApiKeyCredentialProvider ARNs get 404 ResourceNotFoundException where AWS gives 400 ValidationException, and a bogus-resource-type ARN gets 404 ResourceNotFoundException where AWS gives 403 AccessDeniedException. A CodeInterpreter ARN is affected identically to the browser.

Why: a client cannot attach a resource policy to a browser or a code interpreter at all — the call that AWS answers 201 is answered 404, so sharing an execution sandbox across accounts is impossible against the emulator and any provisioning flow that does it aborts. For the genuinely refused types the status and code are wrong too, so a client that branches on ValidationException (fix the request — this resource type takes no policy) versus AccessDeniedException (a permissions/identity problem) versus ResourceNotFoundException (create the resource, or retry — it may not have settled) takes the wrong branch and, in the not-found case, retries a call that can never succeed.

Fix: the ARN-resolution and scope-classification step shared by the resource-policy handlers — `resource_policy_target` in the service crate's resource-policy state module, and the error construction in the PutResourcePolicy handler (with GetResourcePolicy and DeleteResourcePolicy following it). Resolution must admit the browser and code-interpreter stores as policy-carrying targets, and must distinguish three outcomes rather than one `None`: a supported type that resolves (store the document, 201), a real AgentCore resource type that carries no policy plane (ValidationException 400), and an ARN naming no AgentCore resource type (AccessDeniedException 403). Note: the already-open issue 20261108T095631Z-bedrock-agentcore-control-putresourcepolicy-s-resourcenotfoundexception-reuses-the.md covers only the WORDING of the not-found message on a nonexistent-resource ARN; it does not cover which resource types resolve or which refusal class each out-of-scope type gets, so it does not cover this cause.

=== Cause 3 of 3: CreateRegistry succeeds where AWS refuses it with AccessDeniedException in an account without the AgentCore Registry entitlement ===
Failing check: bedrock-agentcore-control.registry-creation-deterministically-refused-without-entitlement

AWS: Smithy CreateRegistry (POST /registries, success 202) takes `name` (RegistryName, @required, @length 1..64, pattern ^[a-zA-Z0-9][a-zA-Z0-9_\-\.\/]*$) plus description/authorizer/clientToken/approvalConfiguration, and declares `AccessDeniedException` with httpError 403 among its errors. The model does not fix the message, so recorded behavior only: in an account that does not hold the AgentCore Registry entitlement, the call is refused SYNCHRONOUSLY on every attempt (deterministically — not a transient throttle, and well under a 30s budget) with code `AccessDeniedException` and the message template "User: <caller-arn> is not authorized to perform: bedrock-agentcore:CreateRegistry". The fixed text is "User: " / " is not authorized to perform: bedrock-agentcore:CreateRegistry"; the single variable part is the caller's principal ARN, taken from the signing identity (the same value sts:GetCallerIdentity reports as Arn) — never a literal from one recording. ListRegistries sits behind the same wall and, when refused, uses the same template with the action `bedrock-agentcore:ListRegistries`. The refusal is scoped to the Registry plane: with the SAME credentials, ListGateways succeeds and CreateApiKeyCredentialProvider + GetApiKeyCredentialProvider round-trip successfully, and the refused registry calls leave no registry listed.

Emulator: CreateRegistry is ACCEPTED. The check's first attempt returned success instead of any error ("CreateRegistry(<name>) attempt 1 unexpectedly SUCCEEDED"), so no AccessDeniedException is produced, the registry is minted, and the account-level entitlement gate does not exist on this path.

Why: an over-permissive write. A client (or an IaC/pre-flight check) that probes whether this account may use AgentCore Registry concludes it may, provisions registry-dependent resources, and then diverges from AWS, where every one of those calls is refused 403. Code that handles the AccessDeniedException entitlement wall — surfacing "request the AgentCore Registry entitlement" to an operator — is never exercised, and nothing distinguishes "Registry is unavailable in this account" from "Registry works".

Fix: the authorization/entitlement gate of the CreateRegistry handler (and the matching gate on the registry read path, ListRegistries, which AWS refuses with the same template) in the bedrock-agentcore-control service crate: the registry plane must refuse with the modeled AccessDeniedException built from the caller's principal ARN and the operation's `bedrock-agentcore:<OperationName>` action, synchronously and identically on every attempt, and must leave no registry recorded. The gate must be scoped to the registry operations only — gateway reads and credential-provider writes with the same credentials must keep succeeding.

Traces: bedrock-agentcore-control.create-api-key-credential-provider-decryption-failure-on-bad-secret, bedrock-agentcore-control.put-resource-policy-refused-for-unsupported-resource-types, bedrock-agentcore-control.registry-creation-deterministically-refused-without-entitlement.

## Evidence
Cause 1 — check bedrock-agentcore-control.create-api-key-credential-provider-decryption-failure-on-bad-secret (verdict: mismatch).
Assertion that failed (check_..._create_api_key_credential_provider_decryption_failure_on_bad_secret.py line 56 -> _error_detail line 103):
  AssertionError: the call was expected to be refused but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'apiKeySecretArn': {'secretArn': 'arn:aws:secretsmanager:<region>:<account>:secret:<generated>'}, 'apiKeySecretJsonKey': 'apiKey', 'apiKeySecretSource': 'EXTERNAL', 'name': '<generated>', 'credentialProviderArn': 'arn:aws:acps:<region>:<account>:token-vault/default/apikeycredentialprovider/<generated>'}
Expected (AWS): HTTP 400, code DecryptionFailure, message "Unable to decrypt the provided secret. Verify KMS key permissions." (entirely fixed text), and GetApiKeyCredentialProvider on that name then 404 ResourceNotFoundException with "ApiKeyCredentialProvider not found for <name>" (the name is the request's value).
Actual (emulator): HTTP 201 success. Region, account, generated names and secret/provider ARNs above are per-run values and are not part of the requirement.
Code path: src/ops/create_api_key_credential_provider.rs lines 277-302 resolve the EXTERNAL reference through `crate::state::external_api_key_secret_arn`, whose only failure classifications are `ApiKeySecretProblem::Absent` -> ResourceNotFoundException and `::Unavailable` -> InternalServerException (lines 354-382). There is no decryptability check, so a secret under a disabled key resolves and the record is inserted at lines 304-331.

Cause 2 — check bedrock-agentcore-control.put-resource-policy-refused-for-unsupported-resource-types (verdict: error).
Failure:
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the PutResourcePolicy operation: Failed to retrieve resource because it doesn't exist. Retry the request with a different resource identifier.
The call was a positive control: the check had already created a Browser via CreateBrowser (networkConfiguration.networkMode PUBLIC) and polled GetBrowser to status READY, then put a valid policy document on the browser's own `browserArn`. AWS answers 201 and echoes the document; the emulator answers 404.
Expected (AWS), per resource type, with ONE identical document reused so only the resource type differs:
  gateway ARN            -> 201, response `policy` equals the submitted document, GetResourcePolicy returns it
  browser ARN            -> 201, same
  code interpreter ARN   -> 201, same
  workloadIdentity ARN   -> ValidationException (400), nothing stored
  apikeycredentialprovider ARN -> ValidationException (400), nothing stored
  arn:aws:bedrock-agentcore:<region>:<account>:totally-bogus-type/<id> -> AccessDeniedException (403), nothing stored
Actual (emulator): every ARN other than a runtime or gateway ARN -> HTTP 404 ResourceNotFoundException. src/state/resource_policies.rs lines 61-74 (`resource_policy_target`) matches only POLICY_TARGET_RUNTIME ("runtime") and POLICY_TARGET_GATEWAY ("gateway") and returns None otherwise; src/ops/put_resource_policy.rs lines 163-169 maps that single None to ResourceNotFoundException. The message quoted above is separately filed (20261108T095631Z-...putresourcepolicy-s-resourcenotfoundexception-reuses-the.md); the status/code/resource-type scope is this cause.

Cause 3 — check bedrock-agentcore-control.registry-creation-deterministically-refused-without-entitlement (verdict: mismatch).
Assertion that failed (check_..._registry_creation_deterministically_refused_without_entitlement.py line 61 -> _refusal line 126):
  AssertionError: CreateRegistry(<generated-name>) attempt 1 unexpectedly SUCCEEDED; an account without the AgentCore Registry entitlement refuses it with AccessDeniedException
Expected (AWS): every one of 3 identical attempts refused synchronously (< 30s) with code AccessDeniedException and message "User: <caller-arn> is not authorized to perform: bedrock-agentcore:CreateRegistry" — fixed text apart from <caller-arn>, which is the signing principal's ARN as reported by sts:GetCallerIdentity; ListRegistries either succeeds without listing the name or is refused with the same template and action bedrock-agentcore:ListRegistries.
Actual (emulator): the first CreateRegistry returned success, so no code, status or message was produced at all.
Positive controls in the same check passed with the same credentials before the subject call: ListGateways(maxResults=1) returned `items`, and CreateApiKeyCredentialProvider + GetApiKeyCredentialProvider round-tripped with matching credentialProviderArn — so the divergence is the missing Registry-scoped entitlement gate, not a broken session. The generated registry/provider names are per-run values.

## Steps to Reproduce
Boot the emulator and replay the three named behavioral checks, or reproduce each by hand:

Cause 1 (CreateApiKeyCredentialProvider / DecryptionFailure):
1. kms:CreateKey, then secretsmanager:CreateSecret with KmsKeyId set to that key and SecretString {"apiKey": "..."}.
2. kms:DisableKey on that key.
3. Call bedrock-agentcore-control CreateApiKeyCredentialProvider with name=<new>, apiKeySecretSource="EXTERNAL", apiKeySecretConfig={secretId: <secret ARN>, jsonKey: "apiKey"}.
4. Observe HTTP 201 and a success body. AWS refuses: HTTP 400, code DecryptionFailure, message "Unable to decrypt the provided secret. Verify KMS key permissions."
5. Call GetApiKeyCredentialProvider(name=<new>): AWS returns 404 ResourceNotFoundException "ApiKeyCredentialProvider not found for <new>" because the refused create stored nothing; the emulator returns the provider it created.
6. kms:EnableKey and repeat step 3: AWS now answers 201 for the identical request.

Cause 2 (PutResourcePolicy resource-type scope):
1. Create a READY Gateway (CreateGateway + poll GetGateway to READY), a READY Browser (CreateBrowser networkMode PUBLIC + poll GetBrowser out of CREATING), and a READY CodeInterpreter (CreateCodeInterpreter networkMode PUBLIC + poll GetCodeInterpreter out of CREATING). Also CreateWorkloadIdentity and CreateApiKeyCredentialProvider.
2. Call PutResourcePolicy with resourceArn = the gateway's gatewayArn and any valid policy document: both AWS and the emulator answer 201.
3. Call PutResourcePolicy with the SAME document shape and resourceArn = the browser's browserArn: AWS answers 201 and GetResourcePolicy on that ARN returns the document. The emulator answers HTTP 404 ResourceNotFoundException. Repeat with the code interpreter's codeInterpreterArn for the same divergence.
4. Call PutResourcePolicy with resourceArn = the workloadIdentityArn, then with the credentialProviderArn: AWS refuses each with ValidationException (400); the emulator answers 404 ResourceNotFoundException.
5. Call PutResourcePolicy with resourceArn = arn:aws:bedrock-agentcore:<region>:<account>:totally-bogus-type/does-not-exist-xyz: AWS refuses with AccessDeniedException (403); the emulator answers 404 ResourceNotFoundException.
6. Re-read GetResourcePolicy on the gateway: the accepted policy must be unchanged.

Cause 3 (CreateRegistry entitlement gate):
1. With ordinary credentials, confirm the session is healthy: ListGateways(maxResults=1) returns `items`, and CreateApiKeyCredentialProvider + GetApiKeyCredentialProvider round-trip.
2. Read the caller ARN with sts:GetCallerIdentity.
3. Call CreateRegistry(name=<valid RegistryName>, description="...") three times with the same name. AWS refuses each one synchronously with HTTP 403, code AccessDeniedException, message "User: <caller ARN from step 2> is not authorized to perform: bedrock-agentcore:CreateRegistry". The emulator returns success on the first attempt.
4. Call ListRegistries: AWS either refuses with the same message template and action bedrock-agentcore:ListRegistries, or succeeds without the name present. The emulator lists the registry it created.
5. Re-read the step-1 credential provider: it must still be readable, proving the refusal is scoped to the Registry plane.

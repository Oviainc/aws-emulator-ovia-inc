Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.delete-guardrail-with-enforced-config-blocked bedrock.delete-guardrail-in-use-by-version-conflict bedrock.create-inference-profile-duplicate-name-succeeds bedrock.inference-profile-duplicate-name-rejected bedrock.prompt-router-unknown-model-rejected bedrock.guardrail-malformed-custom-regex-rejected bedrock.put-use-case-for-model-access-server-recorded bedrock.create-automated-reasoning-policy-duplicate-name-succeeds
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.delete-guardrail-with-enforced-config-blocked|_error#7|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: bedrock.delete-guardrail-in-use-by-version-conflict|_error#15|AssertionError|raise AssertionError("the call was ACCEPTED where a refusal was expected")
Issue-Key: bedrock.create-inference-profile-duplicate-name-succeeds|_error_code#5|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: bedrock.inference-profile-duplicate-name-rejected|_error#9|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: bedrock.prompt-router-unknown-model-rejected|_error#6|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: bedrock.guardrail-malformed-custom-regex-rejected|run#3|AssertionError|raise AssertionError(
Issue-Key: bedrock.put-use-case-for-model-access-server-recorded|run#9|AssertionError|raise AssertionError(
Issue-Key: bedrock|CreateAutomatedReasoningPolicy|ConflictException
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# bedrock control plane is over-permissive: enforced-guardrail deletion, unknown model sources, idempotency-token reuse, uncompilable guardrail regexes and the model-access form are all accepted, while a duplicate automated-reasoning-policy name is wrongly refused (7 causes)

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteGuardrail

## Description
Six bedrock control-plane operations accept requests AWS refuses, and a seventh refuses one AWS accepts. Each is an independent pre-condition the handler never evaluates (or invents); they are bundled as one service's over-permissiveness sweep.

=== Cause 1 of 7: DeleteGuardrail succeeds while the account-level enforced guardrail configuration still targets the guardrail ===
AWS: Smithy (com.amazonaws.bedrock) DeleteGuardrail = DELETE /guardrails/{guardrailIdentifier}, success 202, optional httpQuery guardrailVersion, errors ValidationException (httpError 400), ConflictException (400), ResourceInUseException (400), ResourceNotFoundException (404). Message: recorded behavior only — when the guardrail (or the exact version) is registered through PutEnforcedGuardrailConfiguration as the account's mandatory guardrail, the delete is refused 400 ValidationException with the wholly fixed text "The guardrail cannot be deleted because it is associated with an account-level or organization-level default guardrail configuration.", and the guardrail remains readable; removing the enforcement reference makes the identical delete succeed.
Emulator: HTTP 202 with an empty body in both shapes of the check — whole-guardrail delete by ARN while the enforced config points at a published version, and delete of the exact enforced version. Nothing is refused, so the account is left with an enforced configuration naming a guardrail that no longer exists.
Why: a caller can silently destroy the account's mandatory guardrail; every later inference that should be intervened on now has a dangling enforcement reference, and the guard against exactly this (AWS's ValidationException) never fires. Both checks' positive controls — delete succeeding once the reference is removed — are also never exercised.
Fix: DeleteGuardrail's handler must consult the account-enforced guardrail configuration state before deleting, for both the version-less and version-scoped forms.
Traces: bedrock.delete-guardrail-with-enforced-config-blocked, bedrock.delete-guardrail-in-use-by-version-conflict.

=== Cause 2 of 7: CreateInferenceProfile accepts a modelSource.copyFrom ARN that names no real model ===
AWS: Smithy CreateInferenceProfile = POST /inference-profiles, success 201, required members inferenceProfileName and modelSource, errors ValidationException (400), ConflictException (400), ResourceNotFoundException (404). Message: recorded behavior only — modelSource.copyFrom naming a foundation-model ARN that does not resolve is refused 400 ValidationException with the wholly fixed text "The provided ARN is invalid."
Emulator: HTTP 201 with inferenceProfileArn=<minted application-inference-profile ARN> and status ACTIVE — the bogus source is never resolved, so a profile is created that routes to nothing.
Why: an unroutable profile is created and reported ACTIVE; the failure surfaces much later at inference time instead of at create time, and the caller's error handling for a bad source ARN is never reached.
Fix: resolve modelSource.copyFrom against the model catalogue in CreateInferenceProfile's handler/validation and refuse with the modelled ValidationException.
Traces: bedrock.create-inference-profile-duplicate-name-succeeds.

=== Cause 3 of 7: CreateInferenceProfile ignores clientRequestToken, so token reuse with different parameters creates a second profile instead of conflicting ===
AWS: Smithy CreateInferenceProfile carries clientRequestToken with @idempotencyToken and the operation is marked idempotent; ConflictException is modelled (httpError 400). Recorded behavior: replaying the SAME clientRequestToken with DIFFERENT request parameters (a different inferenceProfileName) is refused with ConflictException and creates nothing, while replaying the same token with the SAME parameters returns the first profile's ARN.
Emulator: the mismatched replay is ACCEPTED — HTTP 201 with a new, distinct application-inference-profile ARN and status ACTIVE. The token is not stored or compared at all.
Why: idempotency is the contract clients rely on when retrying through timeouts; with the token ignored, a retried create silently duplicates the resource, and the conflict that tells a caller "you reused a token with different arguments" never happens.
Fix: CreateInferenceProfile's handler/state must record clientRequestToken with the request's parameter fingerprint and answer ConflictException on a mismatched replay (and the original result on an identical replay).
Traces: bedrock.inference-profile-duplicate-name-rejected.

=== Cause 4 of 7: CreatePromptRouter accepts a fallbackModel that is neither a real model nor one of the routed models ===
AWS: Smithy CreatePromptRouter = POST /prompt-routers, success 200, required members promptRouterName, models, routingCriteria, fallbackModel; errors ValidationException (400), ResourceNotFoundException (404), ConflictException (400). Message: recorded behavior only — a fallbackModel.modelArn that is not present in the submitted models list (here a nonexistent foundation-model ARN) is refused 400 ValidationException with the wholly fixed text "The fallback model is not present in the list of models for routing.", and no router is left behind.
Emulator: HTTP 200 with promptRouterArn=<minted default-prompt-router ARN> — the request is accepted and a router is created.
Why: a router is created whose fallback can never serve traffic, and the create-time guard AWS provides is absent; the check's effect-absent assertion (no router by that name) and its positive control (the same request with a valid fallback reaching status AVAILABLE) are not reached.
Fix: validate fallbackModel against the submitted models list and the model catalogue in CreatePromptRouter's handler/validation.
Traces: bedrock.prompt-router-unknown-model-rejected.

=== Cause 5 of 7: CreateGuardrail accepts an uncompilable custom regex ===
AWS: Smithy CreateGuardrail = POST /guardrails, success 202, errors ValidationException (400); sensitiveInformationPolicyConfig.regexesConfig entries carry a pattern string. Recorded behavior only: a pattern that cannot compile (here 'EMP-[0-9{6}', an unclosed character class) is refused at CreateGuardrail with ValidationException; no guardrail id is minted. The check also accepts the alternative that AWS validates asynchronously and the guardrail reaches GuardrailStatus FAILED — the emulator does neither.
Emulator: HTTP 202 with a guardrailId, and the guardrail is not reported FAILED — the regex is stored verbatim and never compiled.
Why: the guardrail is advertised as usable with a filter that can never match, so the sensitive-information policy the caller configured silently does not apply. A caller gets no signal at configuration time, which is the only time it can fix the pattern.
Fix: compile every regexesConfig pattern in CreateGuardrail's validation (and UpdateGuardrail's) and refuse with ValidationException, or drive the guardrail to FAILED as the alternative AWS behavior.
Traces: bedrock.guardrail-malformed-custom-regex-rejected.

=== Cause 6 of 7: PutUseCaseForModelAccess is accepted where AWS refuses every submission ===
AWS: Smithy PutUseCaseForModelAccess = POST /use-case-for-model-access, success 201, required member formData (blob, @length 10..16384), errors ValidationException (400), AccessDeniedException (403). Recorded behavior only: every submission is refused — ValidationException with the byte-identical, wholly fixed message "Invalid form data." independent of the payload (three structurally distinct payloads produced the identical text), or, on some account classes, an AccessDeniedException whose message names "Internal Account". Nothing is persisted: an independent GetUseCaseForModelAccess still reports no form.
Emulator: the call SUCCEEDS, so the write appears to persist and the refusal a caller must handle never occurs.
Why: a client that submits the model-access form and branches on the refusal (the only outcome AWS produces) takes a success path that does not exist in production, and anything gated on "has the form been accepted" diverges from the real account state.
Fix: PutUseCaseForModelAccess's handler must answer the modelled ValidationException "Invalid form data." and leave the account's use-case state untouched.
Traces: bedrock.put-use-case-for-model-access-server-recorded.

=== Cause 7 of 7 (opposite direction): CreateAutomatedReasoningPolicy invents a name-uniqueness conflict AWS does not enforce ===
AWS: Smithy CreateAutomatedReasoningPolicy = POST /automated-reasoning-policies, success 200, required member name (@pattern ^[0-9a-zA-Z-_ ]+$, @length 1..256), output policyArn/version/name/createdAt/updatedAt; ConflictException is modelled (httpError 400). Recorded behavior only: creating a second policy with the SAME name succeeds and returns a distinct policyArn — the name is not a unique identifier for this resource, so ConflictException is not the answer to a duplicate name.
Emulator: the second create is REFUSED with 400 ConflictException "an automated reasoning policy named <name> already exists in this account in us-west-2" — a uniqueness rule the service does not have, in the emulator's own wording.
Why: callers that legitimately create same-named policies (per-environment copies, re-runs of a template) are blocked by an invented conflict, and the check cannot proceed to the duplicate-name behavior it asserts.
Fix: remove the name-uniqueness pre-condition from CreateAutomatedReasoningPolicy's handler/state; reserve ConflictException for the conditions the service actually conflicts on (e.g. idempotency-token reuse with different parameters).
Traces: bedrock.create-automated-reasoning-policy-duplicate-name-succeeds.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service bedrock 91/199 passed.

[mismatch] bedrock.delete-guardrail-with-enforced-config-blocked
  expected: refusal (400 ValidationException "The guardrail cannot be deleted because it is associated with an account-level or organization-level default guardrail configuration.")
  actual:   "expected this call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 202, ...}}"
[mismatch] bedrock.delete-guardrail-in-use-by-version-conflict
  got = _error(lambda: bedrock.delete_guardrail(guardrailIdentifier=guardrail_arn)) -> AssertionError: "the call was ACCEPTED where a refusal was expected"
  expected tuple in the check body: ('ValidationException', 'The guardrail cannot be deleted because it is associated with an account-level or organization-level default guardrail configuration.')

[mismatch] bedrock.create-inference-profile-duplicate-name-succeeds
  request: CreateInferenceProfile(inferenceProfileName=<unique>, modelSource={'copyFrom': 'arn:aws:bedrock:<region>::foundation-model/sim2real-does-not-exist-v1'})
  expected: ValidationException 'The provided ARN is invalid.'
  actual:   HTTP 201 {'inferenceProfileArn': 'arn:aws:bedrock:us-west-2:<account>:application-inference-profile/<id>', 'status': 'ACTIVE'}

[mismatch] bedrock.inference-profile-duplicate-name-rejected
  request: CreateInferenceProfile replaying the SAME clientRequestToken with a different inferenceProfileName
  expected: ConflictException, no second profile
  actual:   HTTP 201 {'inferenceProfileArn': '...application-inference-profile/<new id>', 'status': 'ACTIVE'}

[mismatch] bedrock.prompt-router-unknown-model-rejected
  request: CreatePromptRouter(models=[nova-lite, nova-pro], fallbackModel={'modelArn': 'arn:aws:bedrock:<region>::foundation-model/does-not-exist.no-such-model-v9:0'}, routingCriteria={'responseQualityDifference': 10.0})
  expected: ValidationException 'The fallback model is not present in the list of models for routing.'
  actual:   HTTP 200 {'promptRouterArn': 'arn:aws:bedrock:us-west-2:<account>:default-prompt-router/<id>'}

[mismatch] bedrock.guardrail-malformed-custom-regex-rejected
  "CreateGuardrail accepted the uncompilable pattern 'EMP-[0-9{6}' and returned guardrailId=<id>. Either the pattern is validated asynchronously (check for GuardrailStatus.FAILED and retarget this assertion) or config is not validated at all."

[mismatch] bedrock.put-use-case-for-model-access-server-recorded
  "PutUseCaseForModelAccess unexpectedly SUCCEEDED on this account"; the check accepts only ValidationException with the exact text 'Invalid form data.' or an AccessDeniedException naming "Internal Account".

[error] bedrock.create-automated-reasoning-policy-duplicate-name-succeeds
  botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the CreateAutomatedReasoningPolicy operation: an automated reasoning policy named <name> already exists in this account in us-west-2
  expected: the second create SUCCEEDS with a distinct policyArn.

Model facts used (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json): DeleteGuardrail DELETE /guardrails/{guardrailIdentifier} code 202, errors ValidationException 400 / ConflictException 400 / ResourceInUseException 400 / ResourceNotFoundException 404; CreateInferenceProfile POST /inference-profiles code 201, idempotent, clientRequestToken @idempotencyToken, errors ConflictException 400 / ValidationException 400 / ResourceNotFoundException 404; CreatePromptRouter POST /prompt-routers code 200, required fallbackModel, errors ValidationException 400 / ResourceNotFoundException 404 / ConflictException 400; CreateGuardrail POST /guardrails code 202, errors ValidationException 400; PutUseCaseForModelAccess POST /use-case-for-model-access code 201, formData required blob @length 10..16384, errors ValidationException 400 / AccessDeniedException 403; CreateAutomatedReasoningPolicy POST code 200, name required, ConflictException 400 modelled.

## Steps to Reproduce
Boot the emulator from this checkout and replay the eight checks in trace_ids (suite simparity-behavioral-aws-v1).
1. CreateGuardrail -> wait READY -> CreateGuardrailVersion -> PutEnforcedGuardrailConfiguration(guardrailInferenceConfig={guardrailIdentifier, guardrailVersion}) -> DeleteGuardrail(guardrailIdentifier=<arn>). Expect 400 ValidationException with the enforced-configuration message and the guardrail still gettable; observe 202. Repeat deleting the exact enforced version.
2. CreateInferenceProfile(modelSource={'copyFrom': 'arn:aws:bedrock:<region>::foundation-model/sim2real-does-not-exist-v1'}). Expect ValidationException 'The provided ARN is invalid.'; observe 201 ACTIVE.
3. CreateInferenceProfile(clientRequestToken=T, inferenceProfileName=A, modelSource=<system profile>), then again with clientRequestToken=T and inferenceProfileName=B. Expect ConflictException and no second profile; observe 201 with a new ARN.
4. CreatePromptRouter with a fallbackModel ARN absent from models. Expect ValidationException 'The fallback model is not present in the list of models for routing.' and no router; observe 200 with a router ARN.
5. CreateGuardrail with sensitiveInformationPolicyConfig.regexesConfig=[{pattern: 'EMP-[0-9{6}'}]. Expect ValidationException (or the guardrail reaching FAILED); observe 202 READY.
6. PutUseCaseForModelAccess(formData=b'{"companyName": "<marker>"}'). Expect ValidationException 'Invalid form data.' and GetUseCaseForModelAccess still reporting no form; observe success.
7. CreateAutomatedReasoningPolicy(name=N) twice. Expect both to succeed with distinct policyArns; observe the second refused ConflictException.

Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.automated-reasoning-test-case-confidence-threshold-rejected bedrock.put-resource-policy-malformed-policy-document-rejected
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.automated-reasoning-test-case-confidence-threshold-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: bedrock.put-resource-policy-malformed-policy-document-rejected|run#17|AssertionError|assert message == _NONCONFORMING_MESSAGE, f"unexpected rejection message: {message!r}"
Issue-Commit: 93db7882a7dd57b95771a5327951c720d44e45a1

# bedrock control plane: range-violation ValidationException message interpolates the rejected value, and PutResourcePolicy refuses a malformed policy document with emulator-invented text (2 causes)

> _Note: this issue was identified against the code base at commit `93db7882a7dd57b95771a5327951c720d44e45a1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutomatedReasoningPolicyTestCase, PutResourcePolicy

## Description
Two distinct root causes found on the Bedrock control plane by the behavioral suite. Both are in the same service (bedrock) and are bundled here; a fixer must close BOTH before submitting. In both cases the HTTP status (400) and the error code (ValidationException) are correct — only the rendered `message` diverges, and the message is part of the contract.

=== Cause 1 of 2: the range-violation validation message interpolates the rejected value into the "Value at '<member>'" template ===
AWS: Smithy (com.amazonaws.bedrock) CreateAutomatedReasoningPolicyTestCase input member `confidenceThreshold` targets `AutomatedReasoningCheckTranslationConfidence`, a double with @range min 0 / max 1; the operation models ValidationException (httpError 400, member `message`). POST /automated-reasoning-policies/{policyArn}/test-cases, success 200. For a value outside that range AWS refuses with HTTP 400 / ValidationException and renders its standard constraint template with the offending VALUE OMITTED: "<N> validation error detected: Value at '<member path>' failed to satisfy constraint: Member must have value less than or equal to <max>". Fixed text of that message: "validation error detected: Value at '", "' failed to satisfy constraint: Member must have value less than or equal to " and the leading error count. Variable parts, all taken from the request and the model: N (the number of violations), the member path (here `confidenceThreshold`) from the request member that failed, and the limit (here 1) from the member's @range max. The rejected value itself is NOT a part of this template.
Emulator: returns the right status and the right code but renders the other template, splicing the submitted value in after "Value": recorded "1 validation error detected: Value '1.5' at 'confidenceThreshold' failed to satisfy constraint: Member must have value less than or equal to 1" where AWS emits "1 validation error detected: Value at 'confidenceThreshold' failed to satisfy constraint: Member must have value less than or equal to 1".
Why: a client or test suite that matches Bedrock's validation text (and any log scraper built on it) does not match, so a rejection that is correct on the wire reads as a different error. It also reflects submitted request content back into an error string that AWS deliberately keeps value-free for this API, putting caller data into logs AWS keeps clean.
Fix: the shared constraint-validation message renderer used by the bedrock control-plane handlers (the component that formats @range / @pattern / @length violations for this crate), so a range violation emits "Value at '<path>'" and never interpolates the value. NOTE: open issue 20261024T025841Z-bedrock-control-plane-no-iam-authorization-gate-guardrail.md, Cause 4 of 5, describes the same renderer defect observed on CreateModelInvocationJob's s3Uri/timeoutDurationInHours; this check was NOT marked as owned by it, so it is filed here — if that renderer is the single shared one, the two may close together, but this check must be replayed to confirm it.
Traces: bedrock.automated-reasoning-test-case-confidence-threshold-rejected; validation for services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/create_automated_reasoning_policy_test_case.rs.

=== Cause 2 of 2: PutResourcePolicy refuses a syntactically invalid policy document with an emulator-authored message instead of Bedrock's fixed refusal text ===
AWS: Smithy PutResourcePolicy input members `resourceArn` (ResourcePolicyResourceArn, length 1..255, required) and `resourcePolicy` (ResourcePolicyDocument, length 1..20480, pattern ^[\t\n\r -\u00ff]+$, required); errors AccessDeniedException (403), ConflictException (400), InternalServerException (500), ThrottlingException (429), ValidationException (400); HTTP POST /resource-policy, success 201. A `resourcePolicy` that satisfies the member's length/pattern but is not a parseable IAM policy document is refused with HTTP 400 / ValidationException whose message is entirely FIXED text carrying no value from the request: "The provided policy document does not conform to the supported policy formats. For more information about valid policy document formats, check out the Amazon Bedrock Guardrails documentation". There are no variable parts in this message — it names neither the member, the document, nor the parse defect.
Emulator: refuses with the correct status and code but its own sentence, naming the member and an internal parse diagnosis: "resourcePolicy is not a resource-based policy document: the document is not JSON". The handler builds it at services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/put_resource_policy.rs lines 198-204, formatting `resource_policy_defect`'s diagnostic into the modelled ValidationException.
Why: a client cannot recognise Bedrock's documented non-conforming-policy refusal, so the standard handling for it (surface the policy-format guidance, fall back to a different policy shape) never fires; the emulator instead leaks an internal parser verdict ("the document is not JSON") that AWS never emits and that invites callers to branch on a string AWS will never send. The same handler's not-found path ("no Bedrock resource <arn> exists in this account") is authored the same way and should be reviewed against AWS's text while this is fixed.
Fix: PutResourcePolicy's validation/refusal path in its handler (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/put_resource_policy.rs) — keep the 400 ValidationException but emit Bedrock's fixed non-conforming-policy-document message rather than a formatted internal defect string.
Traces: bedrock.put-resource-policy-malformed-policy-document-rejected.

## Evidence
Behavioral suite (simparity-behavioral-ga-prod-v1), bedrock 32/84 passed. Both checks are [mismatch]: the emulator answered, with the correct HTTP status and error code, and only the message text diverged.

Cause 1 — bedrock.automated-reasoning-test-case-confidence-threshold-rejected [mismatch]:
  call:     CreateAutomatedReasoningPolicyTestCase(policyArn=<a policy created in this account>, guardContent=..., expectedAggregatedFindingsResult='INVALID', confidenceThreshold=1.5)
  agreed:   HTTP 400, error code ValidationException
  expected: message "1 validation error detected: Value at 'confidenceThreshold' failed to satisfy constraint: Member must have value less than or equal to 1"
  actual:   message "1 validation error detected: Value '1.5' at 'confidenceThreshold' failed to satisfy constraint: Member must have value less than or equal to 1"
  (replay: AssertionError: expected message "1 validation error detected: Value at 'confidenceThreshold' failed to satisfy constraint: Member must have value less than or equal to 1", got "1 validation error detected: Value '1.5' at 'confidenceThreshold' failed to satisfy constraint: Member must have value less than or equal to 1")
  The only difference is the interpolated "'1.5' " — the member path and the limit are rendered correctly. '1.5' is the request's value, not part of the template.

Cause 2 — bedrock.put-resource-policy-malformed-policy-document-rejected [mismatch]:
  call:     PutResourcePolicy(resourceArn=<READY guardrail's ARN>, resourcePolicy='{this is not valid json at all!!')
  agreed:   HTTP 400, error code ValidationException
  expected: message "The provided policy document does not conform to the supported policy formats. For more information about valid policy document formats, check out the Amazon Bedrock Guardrails documentation" (fixed text, no request values)
  actual:   message 'resourcePolicy is not a resource-based policy document: the document is not JSON'
  (replay: AssertionError: unexpected rejection message: 'resourcePolicy is not a resource-based policy document: the document is not JSON')
  Source of the diverging text: services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/put_resource_policy.rs:198-204.

Related open issues (neither is marked as owning these checks): 20261024T025841Z-bedrock-control-plane-no-iam-authorization-gate-guardrail.md (its Cause 4 is the same value-interpolating renderer seen on CreateModelInvocationJob, but it does not name confidenceThreshold or CreateAutomatedReasoningPolicyTestCase, and names no PutResourcePolicy message) and 20261025T040517Z-bedrock-deleteresourcepolicy-stub.md (the DeleteResourcePolicy scaffold stub, a different operation).

## Steps to Reproduce
Boot the emulator from this checkout and replay the named checks (suite simparity-behavioral-ga-prod-v1):

Cause 1: call CreateAutomatedReasoningPolicy to get a policyArn, then (with client-side parameter validation disabled so the request reaches the server) call CreateAutomatedReasoningPolicyTestCase against it with confidenceThreshold=1.5, guardContent set and expectedAggregatedFindingsResult='INVALID'. Read the ValidationException message. Expect "1 validation error detected: Value at 'confidenceThreshold' failed to satisfy constraint: Member must have value less than or equal to 1"; observe the same string with "'1.5' " spliced in after "Value". Replay bedrock.automated-reasoning-test-case-confidence-threshold-rejected.

Cause 2: CreateGuardrail (blockedInputMessaging, blockedOutputsMessaging, a wordPolicyConfig entry), poll GetGuardrail until status READY, CreateGuardrailVersion and poll that version to READY, then call PutResourcePolicy(resourceArn=<guardrail ARN>, resourcePolicy='{this is not valid json at all!!'). Expect HTTP 400 ValidationException with the fixed message "The provided policy document does not conform to the supported policy formats. For more information about valid policy document formats, check out the Amazon Bedrock Guardrails documentation"; observe 'resourcePolicy is not a resource-based policy document: the document is not JSON'. Replay bedrock.put-resource-policy-malformed-policy-document-rejected.

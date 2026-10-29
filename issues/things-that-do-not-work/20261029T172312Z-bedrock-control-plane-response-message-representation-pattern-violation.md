Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.register-marketplace-endpoint-invalid-arn-rejected bedrock.register-marketplace-endpoint-malformed-arn-rejected bedrock.custom-models-list bedrock.put-resource-policy-malformed-document-rejected bedrock.delete-resource-policy-missing-returns-notfound
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.register-marketplace-endpoint-invalid-arn-rejected|run#7|AssertionError|assert message == "1 validation error detected: Value at 'modelSourceIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: .
Issue-Key: bedrock.register-marketplace-endpoint-malformed-arn-rejected|run#8|AssertionError|assert message_malformed == "1 validation error detected: Value at 'modelSourceIdentifier' failed to satisfy constraint: Member must satisfy regular expression
Issue-Key: bedrock.custom-models-list|run#12|AssertionError|assert got == expected, (
Issue-Key: bedrock.put-resource-policy-malformed-document-rejected|run#8|KeyError|baseline_policy_text = bedrock.get_resource_policy(resourceArn=guardrail_arn)["resourcePolicy"]
Issue-Key: bedrock.delete-resource-policy-missing-returns-notfound|run#7|KeyError|before_policy = bedrock.get_resource_policy(resourceArn=guardrail_arn)["resourcePolicy"]
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# bedrock control plane response/message representation: pattern-violation messages echo the rejected value and drop the modelled '.*', the enum-violation message drops the value AWS does echo, and GetResourcePolicy omits the "{}" no-policy sentinel (3 causes)

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterMarketplaceModelEndpoint

## Description
Three representation defects in the bedrock control plane: two in how constraint-violation ValidationException messages are built, one in a 200 response body. The open issue 20261024T025841Z-bedrock-control-plane-no-iam-authorization-gate-guardrail.md names "validation messages echo the rejected value" as one of its causes but does not carry any of the replay keys below, and causes 2 and 3 are the opposite/unrelated defects, so none of this is covered there.

=== Cause 1 of 3: the pattern-constraint message echoes the rejected value and renders the pattern without the leading '.*' ===
AWS: Smithy (com.amazonaws.bedrock) RegisterMarketplaceModelEndpoint = POST /marketplace-model/endpoints/{endpointIdentifier}/registration, success 200, required members endpointIdentifier (httpLabel) and modelSourceIdentifier with @pattern arn:aws:sagemaker:.*:hub-content/SageMakerPublicHub/Model/.* ; errors ValidationException (httpError 400) and ResourceNotFoundException (404). Message text: recorded behavior only — a modelSourceIdentifier that is not a hub-content ARN at all is refused 400 ValidationException with exactly "1 validation error detected: Value at 'modelSourceIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: .*arn:aws:sagemaker:.*:hub-content/SageMakerPublicHub/Model/.*". Fixed text: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: ". Variable parts: the member path from the request ('modelSourceIdentifier') and the pattern, which AWS reports wrapped in '.*' on both sides. NO value token appears for a pattern violation on this member.
Emulator: ValidationException with "1 validation error detected: Value 'not-an-arn-at-all' at 'modelSourceIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws:sagemaker:.*:hub-content/SageMakerPublicHub/Model/.*" — two deltas from one message builder: (a) it interpolates the rejected value where AWS emits none, (b) it renders the pattern without the leading '.*' AWS reports.
Why: the rejected value is caller data; echoing it into an error reflects potentially sensitive input back and breaks every exact-message assertion. The pattern text is what a caller is told to satisfy, so reporting a different regex than AWS does misdirects the fix. Both checks also never reach their positive control, which pins the DIFFERENT refusal a well-formed-but-nonexistent hub-content ARN must get: ResourceNotFoundException "Hub content with name <hub content name from the ARN> does not exist." — so format-vs-existence distinguishability for this operation is unverified.
Fix: the pattern-constraint arm of the shared validation-message builder as used by RegisterMarketplaceModelEndpoint's validation (and the handler's existence check for the positive-control refusal).
Traces: bedrock.register-marketplace-endpoint-invalid-arn-rejected, bedrock.register-marketplace-endpoint-malformed-arn-rejected.

=== Cause 2 of 3: the enum-constraint message omits the rejected value that AWS DOES echo ===
AWS: Smithy ListCustomModels = GET /custom-models, success 200, httpQuery member modelStatus targeting the ModelStatus enum; errors ValidationException (400). Message: recorded behavior only — an unmodelled modelStatus is refused with exactly "1 validation error detected: Value 'not-a-real-status' at 'modelStatus' failed to satisfy constraint: Member must satisfy enum value set: [Active, Creating, Failed]". Fixed text as shown; variable parts are the submitted value, the member path, and the admitted value set. For an ENUM violation the value IS part of the message (unlike the pattern violation in cause 1).
Emulator: "1 validation error detected: Value at 'modelStatus' failed to satisfy constraint: Member must satisfy enum value set: [Active, Creating, Failed]" — the value token is missing.
Why: together with cause 1 this shows the value token is being decided globally instead of per constraint kind: it is emitted where AWS omits it and omitted where AWS emits it. A caller or log reader cannot see WHICH value was rejected on a list call that may carry several filters.
Fix: the enum-constraint arm of the same validation-message builder; the value token must be driven by constraint kind, not by one global policy.
Traces: bedrock.custom-models-list.

=== Cause 3 of 3: GetResourcePolicy on a resource with no policy returns 200 with no resourcePolicy member instead of the "{}" sentinel ===
AWS: Smithy GetResourcePolicy = GET /resource-policy/{resourceArn}, success 200, output member resourcePolicy (ResourcePolicyDocument, @length 1..20480), errors ResourceNotFoundException (404), ValidationException (400), AccessDeniedException (403). Body: recorded behavior only — a guardrail that never had a resource policy attached answers GetResourcePolicy SUCCESSFULLY (not 404) with resourcePolicy present and set to the canonical no-policy sentinel "{}"; that is the baseline callers compare against to prove a rejected PutResourcePolicy changed nothing.
Emulator: HTTP 200 whose body contains no resourcePolicy member at all, so the SDK read raises KeyError: 'resourcePolicy'.
Why: the member is the only thing GetResourcePolicy returns; omitting it makes the operation unusable for its sole purpose — a caller cannot read, diff, back up or restore a resource policy, and cannot distinguish "no policy" from "the service forgot to answer". Both grounded checks abort on this read BEFORE their actual subject, so PutResourcePolicy's malformed-document refusal (expected 400 ValidationException "The provided policy document does not conform to the supported policy formats. For more information about valid policy document formats, check out the Amazon Bedrock Guardrails documentation") and DeleteResourcePolicy's missing-policy behavior are both unverified behind it.
Fix: GetResourcePolicy's handler/serializer — always emit resourcePolicy on the 200 path, using "{}" when the resource has no stored policy.
Traces: bedrock.put-resource-policy-malformed-document-rejected, bedrock.delete-resource-policy-missing-returns-notfound.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service bedrock 91/199 passed.

[mismatch] bedrock.register-marketplace-endpoint-invalid-arn-rejected and [mismatch] bedrock.register-marketplace-endpoint-malformed-arn-rejected
  request: RegisterMarketplaceModelEndpoint(endpointIdentifier=<a sagemaker endpoint ARN>, modelSourceIdentifier='not-an-arn-at-all' / 'this-is-not-an-arn')
  expected: 400 ValidationException "1 validation error detected: Value at 'modelSourceIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: .*arn:aws:sagemaker:.*:hub-content/SageMakerPublicHub/Model/.*"
  actual:   400 ValidationException "1 validation error detected: Value 'not-an-arn-at-all' at 'modelSourceIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws:sagemaker:.*:hub-content/SageMakerPublicHub/Model/.*"
  (the quoted value is this run's input; the requirement is that NO value token appears and the pattern is reported with the leading '.*')
  unreached in both checks: the positive control asserting ResourceNotFoundException "Hub content with name <name> does not exist." for a well-formed but nonexistent hub-content ARN, and the ListMarketplaceModelEndpoints effect-absent read.

[mismatch] bedrock.custom-models-list
  request: ListCustomModels(modelStatus='not-a-real-status')
  expected: ('ValidationException', "1 validation error detected: Value 'not-a-real-status' at 'modelStatus' failed to satisfy constraint: Member must satisfy enum value set: [Active, Creating, Failed]")
  actual:   ('ValidationException', "1 validation error detected: Value at 'modelStatus' failed to satisfy constraint: Member must satisfy enum value set: [Active, Creating, Failed]")

[error] bedrock.put-resource-policy-malformed-document-rejected
  CreateGuardrail succeeds, then: baseline_policy_text = bedrock.get_resource_policy(resourceArn=guardrail_arn)["resourcePolicy"]
  -> KeyError: 'resourcePolicy'  (HTTP 200 with the member absent)
  check comment states the recorded AWS behavior verbatim: "A fresh guardrail with no policy ever attached still answers GetResourcePolicy successfully with the canonical 'no policy' sentinel resourcePolicy=\"{}\", rather than raising ResourceNotFoundException"
  unreached: both PutResourcePolicy malformed-document refusals (ValidationException with the non-conforming-document message), the two effect-absent comparisons, and the valid-document positive control.

[error] bedrock.delete-resource-policy-missing-returns-notfound
  before_policy = bedrock.get_resource_policy(resourceArn=guardrail_arn)["resourcePolicy"] -> KeyError: 'resourcePolicy'

Model facts used (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json): RegisterMarketplaceModelEndpoint POST code 200, modelSourceIdentifier required @pattern arn:aws:sagemaker:.*:hub-content/SageMakerPublicHub/Model/.* , errors ValidationException 400 / ResourceNotFoundException 404; ListCustomModels GET /custom-models code 200, modelStatus httpQuery -> ModelStatus enum, errors ValidationException 400; GetResourcePolicy GET /resource-policy/{resourceArn} code 200, output resourcePolicy -> ResourcePolicyDocument @length 1..20480 (optional in the model), errors ResourceNotFoundException 404 / ValidationException 400 / AccessDeniedException 403.

## Steps to Reproduce
Boot the emulator from this checkout and replay the five checks in trace_ids (suite simparity-behavioral-aws-v1).
1. RegisterMarketplaceModelEndpoint(endpointIdentifier=arn:aws:sagemaker:<region>:<account>:endpoint/<name>, modelSourceIdentifier='not-an-arn-at-all'). Expect the pattern message with NO value token and the pattern printed as .*arn:aws:sagemaker:.*:hub-content/SageMakerPublicHub/Model/.* ; observe the value echoed and the leading .* missing. Then repeat with modelSourceIdentifier=arn:aws:sagemaker:<region>:aws:hub-content/SageMakerPublicHub/Model/<missing-name>/1.0.0 and expect ResourceNotFoundException "Hub content with name <missing-name> does not exist."
2. ListCustomModels(modelStatus='not-a-real-status'). Expect the enum message WITH the value token; observe it missing.
3. CreateGuardrail, then GetResourcePolicy(resourceArn=<guardrailArn>). Expect 200 with resourcePolicy == "{}"; observe 200 with no resourcePolicy member. Then PutResourcePolicy with '{this is not json at all::::' and with '{"hello": "world"}' — both expect 400 ValidationException "The provided policy document does not conform to the supported policy formats. For more information about valid policy document formats, check out the Amazon Bedrock Guardrails documentation" and must leave the Get result equal to the baseline.

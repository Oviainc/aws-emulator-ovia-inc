Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.parameter-allowed-values-reject cloudformation.parameter-type-mismatch-rejected cloudformation.invalid-template-error cloudformation.validate-template-invalid
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.parameter-allowed-values-reject|_error#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded with {result!r}")
Issue-Key: cloudformation.parameter-type-mismatch-rejected|_error#5|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: cloudformation.invalid-template-error|_client_error#8|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: cloudformation.validate-template-invalid|run#5|AssertionError|assert error == ("ValidationError", "Template format error: unsupported structure."), (
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# cloudformation: template and parameter validation is not enforced — AllowedValues, Number typing and a structurally invalid template are all accepted, and ValidateTemplate's refusal wording diverges (4 causes)

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Four net-new behavioral root causes of service `cloudformation`, grounded in four failing behavioral checks. Each cause is independent and must be closed separately.

=== Cause 1 of 4: a Parameters value outside the parameter's AllowedValues is accepted ===
AWS: Smithy models CreateStack.Parameters (list of Parameter) but not the template's own parameter rules, so this is recorded behavior only: CloudFormation evaluates each template Parameter's AllowedValues against the supplied ParameterValue during the synchronous CreateStack validation and refuses with HTTP 400, Error.Code `ValidationError` before any StackId is minted.
Emulator: the request is accepted with HTTP 200 and a StackId/OperationId; AllowedValues is never evaluated.
Why: a client relying on AWS's behavior uses AllowedValues as the contract for what a stack may be parameterised with. Against the emulator a bad value silently flows into the resources, so a deployment pipeline that depends on the refusal to catch operator error deploys a stack AWS would never have created.
Fix: template-parameter constraint evaluation (AllowedValues) in the cloudformation service's template/parameter validator, applied by CreateStack, UpdateStack and CreateChangeSet before any state write.
Traces: cloudformation.parameter-allowed-values-reject.

=== Cause 2 of 4: a ParameterValue that does not match the parameter's declared Type is accepted ===
AWS: recorded behavior only — a template Parameter declared `Type: Number` whose supplied ParameterValue is not numeric is refused synchronously with HTTP 400 `ValidationError`.
Emulator: accepted with HTTP 200 and a StackId; the declared Type is not checked.
Why: the parameter Type is the only type system a template has. A non-numeric value reaching a resource property that AWS would have rejected produces divergent downstream behaviour (or an unrelated provider-level failure much later) instead of a clear up-front refusal.
Fix: the same template/parameter validator as Cause 1 — parameter Type coercion/validation, independent of the AllowedValues rule.
Traces: cloudformation.parameter-type-mismatch-rejected.

=== Cause 3 of 4: a structurally invalid template is accepted by CreateStack ===
AWS: recorded behavior only — a template whose structure is invalid is refused by CreateStack with HTTP 400 `ValidationError` ('Template format error: …'), synchronously, before a stack exists.
Emulator: HTTP 200 with a StackId/OperationId; the stack is created from a document AWS refuses.
Why: the caller now owns a stack id for a template that cannot be deployed on AWS, and the structural fault is reported — if at all — only as a provisioning failure later. Template-format refusals are the first gate every CI pipeline relies on.
Fix: structural template validation in the cloudformation service's template parser/validator, invoked by the CreateStack handler before state is written.
Traces: cloudformation.invalid-template-error.

=== Cause 4 of 4: ValidateTemplate's 'Template format error' message names the wrong fault ===
AWS: Smithy models ValidateTemplate with NO error shapes, so refusals arrive as the service-common `ValidationError` (HTTP 400). Message is recorded behavior only: for the template this check sends, AWS answers `Template format error: unsupported structure.` — entirely fixed text with no interpolation.
Emulator: answers ValidationError with `Template format error: At least one Resources member must be defined.` — also fixed text, but the message of a different template-format fault.
Why: ValidateTemplate exists to tell an author WHAT is wrong. The emulator sends the caller to add a Resources member when the real fault is the document's structure, so linting/CI output is actively misleading, and taxonomy tests keyed on AWS's wording fail.
Fix: the ValidateTemplate handler's template-format diagnosis in the cloudformation service — classify the structural fault before falling back to the missing-Resources message.
Traces: cloudformation.validate-template-invalid.

## Evidence
Cause 1 — cloudformation.parameter-allowed-values-reject [mismatch]:
  AssertionError: expected the call to be rejected, but it succeeded with {'StackId': 'arn:aws:cloudformation:us-west-2:000000000000:stack/sim2real-behav-stk-bad-35d2091525e5/0827aeb7-...', 'OperationId': '531f623b-...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Cause 2 — cloudformation.parameter-type-mismatch-rejected [mismatch]:
  AssertionError: expected this call to be rejected, but it SUCCEEDED and returned {'StackId': 'arn:aws:cloudformation:us-west-2:000000000000:stack/sim2real-behav-badnum-eff31e27dbc4/aa0fc53a-...', 'OperationId': 'a9c41ba7-...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Cause 3 — cloudformation.invalid-template-error [mismatch]:
  AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'StackId': 'arn:aws:cloudformation:us-west-2:000000000000:stack/sim2real-behav-stk-bad-2b94d69966b7/3d854934-...', 'OperationId': '448c84d4-...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Cause 4 — cloudformation.validate-template-invalid [mismatch]:
  AssertionError: Expected ValidationError 'Template format error: unsupported structure.', got ('ValidationError', 'Template format error: At least one Resources member must be defined.')
  Smithy: ValidateTemplate input {TemplateBody @length min 1, TemplateURL @length 1..5120}, output {Parameters, Description, Capabilities, CapabilitiesReason, DeclaredTransforms}, errors = [] — refusals are the service-common ValidationError; both messages are entirely fixed text, so the divergence is which fault is reported.

In all four cases the stack ids, operation ids and request ids shown are per-run values and are not part of the requirement; the requirement is the 400 ValidationError (causes 1-3) and the exact message (cause 4).

## Steps to Reproduce
Cause 1:
1. CreateStack with a template declaring Parameters: {Env: {Type: String, AllowedValues: [dev, prod]}} and pass Parameters=[{ParameterKey: 'Env', ParameterValue: 'staging'}].
2. Observe HTTP 200 + StackId. AWS answers 400 ValidationError and creates nothing.

Cause 2:
1. CreateStack with Parameters: {Size: {Type: Number}} and pass ParameterValue='not-a-number'.
2. Observe HTTP 200 + StackId. AWS answers 400 ValidationError.

Cause 3:
1. CreateStack with the structurally invalid template body the check uses (a document AWS refuses with a Template format error).
2. Observe HTTP 200 + StackId instead of 400 ValidationError.

Cause 4:
1. ValidateTemplate with that same template body.
2. Observe 400 ValidationError 'Template format error: At least one Resources member must be defined.'. AWS answers 'Template format error: unsupported structure.'
3. Re-replay: cloudformation.parameter-allowed-values-reject, cloudformation.parameter-type-mismatch-rejected, cloudformation.invalid-template-error, cloudformation.validate-template-invalid.

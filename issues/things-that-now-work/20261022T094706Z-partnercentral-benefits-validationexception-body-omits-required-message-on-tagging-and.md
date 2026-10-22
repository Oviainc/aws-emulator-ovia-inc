Issue-Source: trace-sweep-divergence
Issue-Trace: partnercentral-benefits-application-lifecycle-validation-errors
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: partnercentral-benefits-application-lifecycle-validation-errors partnercentral-benefits-list-and-tag-validation-error-paths
Issue-Shape-Id: com.amazonaws.partnercentralbenefits#PartnerCentralBenefitsService
Issue-Key: partnercentral-benefits|UntagResource|4xx->4xx|$.Message
Issue-Key: partnercentral-benefits|ListBenefitApplications|4xx->4xx|$.Message
Issue-Key: partnercentral-benefits|GetBenefit|4xx->4xx|$.Message
Issue-Key: partnercentral-benefits|GetBenefitAllocation|4xx->4xx|$.Message
Issue-Key: partnercentral-benefits|TagResource|4xx->4xx|$.Message
Issue-Commit: 9cc0add35ee0fa09b41f1226e545932e7bbe5c43

# ValidationException body omits required Message on tagging and read operations

> _Note: this issue was identified against the code base at commit `9cc0add35ee0fa09b41f1226e545932e7bbe5c43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UntagResource

## Description
This issue bundles 5 causes, the same fault as the lifecycle-operation bundle but on the tagging and single-resource read operations: a constraint violation is answered with the correct HTTP 400 ValidationException, yet the error body carries no `Message` member. These are filed separately only because one issue bundles at most ten causes; a fix in the shared validation-error path should close both.

AWS (common to all five): Smithy `com.amazonaws.partnercentralbenefits#ValidationException` has `Message` @required and `Reason` @required with httpError 400, so `Message` is never absent. Its text is generated from the request and the model's constraints in the standard form `<N> validation error(s) detected: Value at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>` (or `Member must have length less than or equal to <max>`), so the rule — not the recorded string — is the requirement. Each pattern/bound quoted below is the one the on-disk model declares for that member.

Emulator (common to all five): HTTP 400 is returned, but the replay reports `$.Message: missing (got None)`.

Why: clients and SDK error deserializers lose the only machine-readable statement of which input member was rejected and why; a required modelled member is absent, so strict deserializers can fail outright and operators debugging a 400 get an empty error.

Fix: the shared ValidationException construction/serialization path behind these operations' input validation — the constraint check fires correctly and sets the status, but the diagnostic is dropped before the awsJson1_0 error serializer writes the body.

Cause 1 of 5: UntagResource, member `resourceArn`. AWS: Smithy UntagResourceRequest.resourceArn targets TaggableResourceArn, @required, @length 1..1000, @pattern `^arn:[\w+=/,.@-]+:partnercentral:[\w+=/,.@-]*:[0-9]{12}:catalog/([a-zA-Z]+)/[\w+=,.@-]+(/[\w+=,.@-]+)*$`; a non-matching ARN gives 400 with Message naming 'resourceArn' and that pattern. Emulator: 400, `$.Message` absent. Traces: partnercentral-benefits-application-lifecycle-validation-errors#task#11.

Cause 2 of 5: ListBenefitApplications, member `Stages`. AWS: Smithy ListBenefitApplicationsInput.Stages is a list with @length min 0 max 1, so an over-long list gives 400 with Message "1 validation error detected: Value at 'stages' failed to satisfy constraint: Member must have length less than or equal to 1". Emulator: 400, `$.Message` absent — the collection bound is unreported. Traces: partnercentral-benefits-list-and-tag-validation-error-paths#task#2.

Cause 3 of 5: GetBenefit, member `Identifier`. AWS: Smithy GetBenefitInput.Identifier @required, @pattern `^(arn:.+|ben-[0-9a-z]{14})$`; 400 with Message naming 'identifier' and that pattern. Emulator: 400, `$.Message` absent. Traces: partnercentral-benefits-list-and-tag-validation-error-paths#task#3.

Cause 4 of 5: GetBenefitAllocation, member `Identifier`. AWS: allocation identifiers are @pattern `^(arn:.+|benalloc-[0-9a-z]{14})$`; 400 with Message naming 'identifier' and that pattern. Emulator: 400, `$.Message` absent. Traces: partnercentral-benefits-list-and-tag-validation-error-paths#task#5.

Cause 5 of 5: TagResource, member `resourceArn`. AWS: Smithy TagResourceRequest.resourceArn targets TaggableResourceArn, @required, @length 1..1000, same partnercentral catalog-ARN @pattern as UntagResource; 400 with Message naming 'resourceArn' and that pattern. Emulator: 400, `$.Message` absent. Traces: partnercentral-benefits-list-and-tag-validation-error-paths#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (5 of 17 corpus divergences, all verdict mismatch, divergence key `status=400->400|keys=$.Message`).

Fixture partnercentral-benefits-application-lifecycle-validation-errors, task#11 (UntagResource) — expected `$.Message` = "1 validation error detected: Value at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[\w+=/,.@-]+:partnercentral:[\w+=/,.@-]*:[0-9]{12}:catalog/([a-zA-Z]+)/[\w+=,.@-]+(/[\w+=,.@-]+)*"; actual: Message missing.

Fixture partnercentral-benefits-list-and-tag-validation-error-paths:
- task#2 (ListBenefitApplications) — expected "1 validation error detected: Value at 'stages' failed to satisfy constraint: Member must have length less than or equal to 1"; actual: Message missing.
- task#3 (GetBenefit) — expected "1 validation error detected: Value at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:.+|ben-[0-9a-z]{14})"; actual: Message missing.
- task#5 (GetBenefitAllocation) — expected "1 validation error detected: Value at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:.+|benalloc-[0-9a-z]{14})"; actual: Message missing.
- task#6 (TagResource) — expected "1 validation error detected: Value at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[\w+=/,.@-]+:partnercentral:[\w+=/,.@-]*:[0-9]{12}:catalog/([a-zA-Z]+)/[\w+=,.@-]+(/[\w+=,.@-]+)*"; actual: Message missing.

In all five the HTTP status is 400 in both expected and actual — only the required `Message` member differs. Model evidence: on-disk Smithy for PartnerCentralBenefitsService (awsJson1_0) declares ValidationException.Message @required / httpError 400, and declares each quoted @pattern and @length on the named input member.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture `partnercentral-benefits-application-lifecycle-validation-errors` step task#11 (UntagResource with a resourceArn that does not match the partnercentral catalog-ARN pattern).
3. Replay fixture `partnercentral-benefits-list-and-tag-validation-error-paths` steps task#2 (ListBenefitApplications with a Stages list longer than 1), task#3 (GetBenefit with a malformed Identifier), task#5 (GetBenefitAllocation with a malformed Identifier) and task#6 (TagResource with a malformed resourceArn).
4. Each returns HTTP 400 with an awsJson1_0 body lacking the `Message` key; AWS returns the `<N> validation error detected: Value at '<member>' failed to satisfy constraint: ...` message built from the violated member and its modelled constraint.
5. Re-replay those five steps after the fix; only the Message assertion distinguishes fixed from unfixed.

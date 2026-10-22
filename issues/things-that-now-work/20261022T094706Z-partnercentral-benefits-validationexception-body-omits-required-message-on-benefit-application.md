Issue-Source: trace-sweep-divergence
Issue-Trace: partnercentral-benefits-application-lifecycle-validation-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: partnercentral-benefits-application-lifecycle-validation-errors
Issue-Shape-Id: com.amazonaws.partnercentralbenefits#PartnerCentralBenefitsService
Issue-Key: partnercentral-benefits|CreateBenefitApplication|4xx->4xx|$.Message
Issue-Key: partnercentral-benefits|GetBenefitApplication|4xx->4xx|$.Message
Issue-Key: partnercentral-benefits|UpdateBenefitApplication|4xx->4xx|$.Message
Issue-Key: partnercentral-benefits|CancelBenefitApplication|4xx->4xx|$.Message
Issue-Key: partnercentral-benefits|RecallBenefitApplication|4xx->4xx|$.Message
Issue-Key: partnercentral-benefits|SubmitBenefitApplication|4xx->4xx|$.Message
Issue-Key: partnercentral-benefits|AmendBenefitApplication|4xx->4xx|$.Message
Issue-Key: partnercentral-benefits|AssociateBenefitApplicationResource|4xx->4xx|$.Message
Issue-Key: partnercentral-benefits|DisassociateBenefitApplicationResource|4xx->4xx|$.Message
Issue-Key: partnercentral-benefits|ListBenefitAllocations|4xx->4xx|$.Message
Issue-Commit: 9cc0add35ee0fa09b41f1226e545932e7bbe5c43

# ValidationException body omits required Message on benefit application lifecycle operations

> _Note: this issue was identified against the code base at commit `9cc0add35ee0fa09b41f1226e545932e7bbe5c43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBenefitApplication

## Description
This issue bundles 10 causes, all one fault: when a request to a benefit-application lifecycle operation violates an input constraint, the emulator rejects it with the correct HTTP 400 ValidationException but serializes the error body WITHOUT the `Message` member. `com.amazonaws.partnercentralbenefits#ValidationException` models `Message` as @required (Smithy: ValidationException, errorKind=client, httpError=400, members Message @required, Reason @required, FieldList), so the member must always be present. Its content is not free text either: AWS's constraint-violation message is BUILT FROM THE REQUEST and the model's own constraints, in the standard framework form `<N> validation error(s) detected: Value at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>` (or `Member must have length less than or equal to <max>` / `Member must satisfy enum value set: [...]`), with the clauses joined by `; ` and the count matching the number of violations. Every pattern and bound in the recorded messages below is exactly the one the on-disk model declares for that member, so the generating rule is known and is the requirement; the recorded strings are examples of applying it.

Fix: the shared ValidationException construction/serialization path used by these handlers' input validation — the constraint checks already fire (status 400 is right), but the resulting error value is emitted with no Message, so the validation diagnostic is dropped between the check and the awsJson1_0 error serializer. Repair it once in that shared path and all ten causes close together.

Cause 1 of 10: CreateBenefitApplication, member `BenefitIdentifier`. AWS: Smithy CreateBenefitApplicationInput.BenefitIdentifier @required, @pattern `^(arn:.+|ben-[0-9a-z]{14})$`; a non-matching value yields 400 ValidationException whose Message names that member and that pattern, e.g. "1 validation error detected: Value at 'benefitIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:.+|ben-[0-9a-z]{14})". Emulator: 400 with `$.Message` absent (got None). Why: a client that reads the message to learn WHICH member it got wrong, or an SDK that deserializes a required member, sees nothing. Fix: as above. Traces: partnercentral-benefits-application-lifecycle-validation-errors#task#1.

Cause 2 of 10: GetBenefitApplication, member `Identifier`. AWS: @pattern `^(arn:.+|benappl-[0-9a-z]{14})$`; 400 ValidationException with Message naming 'identifier' and that pattern. Emulator: 400, `$.Message` absent. Why/Fix: as above. Traces: partnercentral-benefits-application-lifecycle-validation-errors#task#2.

Cause 3 of 10: UpdateBenefitApplication, member `Identifier`, @pattern `^(arn:.+|benappl-[0-9a-z]{14})$`. Emulator: 400, `$.Message` absent. Traces: partnercentral-benefits-application-lifecycle-validation-errors#task#3.

Cause 4 of 10: CancelBenefitApplication, member `Identifier`, @pattern `^(arn:.+|benappl-[0-9a-z]{14})$`. Emulator: 400, `$.Message` absent. Traces: partnercentral-benefits-application-lifecycle-validation-errors#task#4.

Cause 5 of 10: RecallBenefitApplication, member `Identifier`, @pattern `^(arn:.+|benappl-[0-9a-z]{14})$`. Emulator: 400, `$.Message` absent. Traces: partnercentral-benefits-application-lifecycle-validation-errors#task#5.

Cause 6 of 10: SubmitBenefitApplication, member `Identifier`, @pattern `^(arn:.+|benappl-[0-9a-z]{14})$`. Emulator: 400, `$.Message` absent. Traces: partnercentral-benefits-application-lifecycle-validation-errors#task#6.

Cause 7 of 10: AmendBenefitApplication, member `Identifier`, @pattern `^(arn:.+|benappl-[0-9a-z]{14})$`. Emulator: 400, `$.Message` absent. Traces: partnercentral-benefits-application-lifecycle-validation-errors#task#7.

Cause 8 of 10: AssociateBenefitApplicationResource — TWO violations in one request (`BenefitApplicationIdentifier` @pattern `^(arn:.+|benappl-[0-9a-z]{14})$` and `ResourceArn`, an Arn @pattern `arn:aws:([a-zA-Z0-9\-])+:([a-z]{2}(-gov)?-[a-z]+-\d{1})?:(\d{12})?:(.+)`). AWS: one 400 ValidationException whose Message is pluralised and carries both clauses joined by `; `, prefixed "2 validation errors detected: ...". Emulator: 400, `$.Message` absent — so neither the multi-violation aggregation nor the count prefix is observable. Traces: partnercentral-benefits-application-lifecycle-validation-errors#task#8.

Cause 9 of 10: DisassociateBenefitApplicationResource — same two members, same two-violation aggregated Message. Emulator: 400, `$.Message` absent. Traces: partnercentral-benefits-application-lifecycle-validation-errors#task#9.

Cause 10 of 10: ListBenefitAllocations, member `FulfillmentTypes`. AWS: Smithy ListBenefitAllocationsInput.FulfillmentTypes is a list with @length min 0 max 2 over the FulfillmentType enum, so an over-long list of bad members produces a 400 whose Message carries both a length clause ("Member must have length less than or equal to 2") and a member-level clause ("Member must satisfy constraint: [Member must satisfy enum value set: [ACCESS, CASH, CREDITS], Member must not be null]"), prefixed "2 validation errors detected:". Emulator: 400, `$.Message` absent, so neither the collection-level bound nor the per-element enum check is reported. Traces: partnercentral-benefits-application-lifecycle-validation-errors#task#10.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture `partnercentral-benefits-application-lifecycle-validation-errors`, steps task#1..task#10 (verdict mismatch, 10 of 17 corpus divergences).

Expected (recorded AWS): HTTP 400, awsJson1_0 ValidationException body with `Message` present, e.g. task#1 `$.Message` = "1 validation error detected: Value at 'benefitIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:.+|ben-[0-9a-z]{14})"; task#8/#9 = "2 validation errors detected: Value at 'benefitApplicationIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:.+|benappl-[0-9a-z]{14}); Value at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws:([a-zA-Z0-9\-])+:([a-z]{2}(-gov)?-[a-z]+-\d{1})?:(\d{12})?:(.+)"; task#10 = "2 validation errors detected: Value at 'fulfillmentTypes' failed to satisfy constraint: Member must have length less than or equal to 2; Value at 'fulfillmentTypes' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [ACCESS, CASH, CREDITS], Member must not be null]".

Actual (emulator): HTTP 400 on every step — the status matches — but the replay reports `$.Message: missing (... got None)` for all ten steps. The required member is simply not serialized.

Model evidence (on-disk Smithy, PartnerCentralBenefitsService, awsJson1_0): ValidationException { Message @required, Reason @required, FieldList } with httpError 400; the cited @pattern/@length/enum constraints are the ones declared on each input member, which is why the recorded message text is reproducible rather than environment-specific.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture `partnercentral-benefits-application-lifecycle-validation-errors`, steps task#1 through task#10 (one per operation: CreateBenefitApplication, GetBenefitApplication, UpdateBenefitApplication, CancelBenefitApplication, RecallBenefitApplication, SubmitBenefitApplication, AmendBenefitApplication, AssociateBenefitApplicationResource, DisassociateBenefitApplicationResource, ListBenefitAllocations).
3. Each step POSTs an awsJson1_0 request (X-Amz-Target PartnerCentralBenefitsService.<Operation>) whose identifier/ARN/list member violates the member's modelled constraint.
4. Observe the response is HTTP 400 but the JSON body has no `Message` key; AWS returns the `<N> validation error(s) detected: ...` message built from the violated member names and their constraints.
5. After fixing, re-replay all ten steps listed above — the status already passes, so only the Message assertion distinguishes fixed from unfixed.

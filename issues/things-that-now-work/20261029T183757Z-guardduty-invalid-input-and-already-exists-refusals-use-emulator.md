Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: guardduty.filter-rank-out-of-range-rejected guardduty.create-filter-invalid-criteria-field-rejected guardduty.findings-feedback-invalid-value-rejected guardduty.create-members-malformed-account-id-rejected guardduty.disassociate-members-not-a-member-rejected guardduty.create-duplicate-custom-detection-rule-association-conflict guardduty.create-custom-detection-rule-association-duplicate-name-conflict guardduty.create-duplicate-malware-protection-plan-conflict guardduty.create-malware-protection-plan-duplicate-conflict guardduty.create-custom-detection-rule-association-without-permission-denied
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty.filter-rank-out-of-range-rejected|run#7|AssertionError|assert err["Message"] == (
Issue-Key: guardduty.create-filter-invalid-criteria-field-rejected|run#7|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: guardduty.findings-feedback-invalid-value-rejected|run#9|AssertionError|assert err["Message"] == expected, f"expected message {expected!r}, got {err['Message']!r}"
Issue-Key: guardduty.create-members-malformed-account-id-rejected|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: guardduty.disassociate-members-not-a-member-rejected|run#19|AssertionError|assert err["Message"] == expected, f"expected message {expected!r}, got {err['Message']!r}"
Issue-Key: guardduty.create-duplicate-custom-detection-rule-association-conflict|run#15|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: guardduty.create-custom-detection-rule-association-duplicate-name-conflict|run#11|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: guardduty.create-duplicate-malware-protection-plan-conflict|run#11|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: guardduty.create-malware-protection-plan-duplicate-conflict|run#23|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: guardduty.create-custom-detection-rule-association-without-permission-denied|run#9|AssertionError|assert admin_message == expected_admin_message, (
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# Invalid-input and already-exists refusals use emulator-authored messages instead of GuardDuty's fixed texts

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFilter

## Description
This issue bundles 7 distinct root causes, all in the guardduty crate: the emulator refuses these calls with the right error code and status but writes its own explanatory prose (often naming the member and restating the constraint) where real GuardDuty emits a fixed, documented sentence. Message text is part of the contract, so each is a divergence. Fix all 7 before submitting.

Cause 1 of 7: CreateFilter Rank out of range.
AWS: Smithy CreateFilter.Rank @range min 1 max 100; errors BadRequestException 400, InternalServerErrorException 500. Recorded behavior: Rank=150 is refused HTTP 400 BadRequestException with the fixed text 'The request is rejected because the one or more input parameters have invalid values.' — GuardDuty's generic constraint-violation sentence, with no member name and no limit interpolated.
Emulator: 'The request is rejected because the parameter rank has an invalid value: it must be less than or equal to 100.'
Why: a client that keys on GuardDuty's generic constraint message (or snapshots it) sees an unknown sentence; the emulator also leaks a constraint-specific tail AWS never sends.
Fix: the Rank range check in the CreateFilter handler/validation in the guardduty crate (src/ops/create_filter.rs).
Traces: guardduty.filter-rank-out-of-range-rejected.

Cause 2 of 7: CreateFilter with a criterion key that is not a finding field.
AWS: Smithy CreateFilter.FindingCriteria (required, jsonName findingCriteria); BadRequestException 400. Recorded behavior: a criterion key such as 'this.field.does.not.exist' is refused with the fixed text 'The request was rejected because the parameter findingCriteria has an invalid value.' (fixed text only; the offending key is NOT quoted).
Emulator: "findingCriteria.criterion: 'this.field.does.not.exist' is not a field of a GuardDuty finding, so no finding can be selected or ordered by it." — a bespoke sentence with the request value interpolated.
Why: the message is not AWS's, and it echoes request input AWS does not echo, so parity harnesses and log scrapers both diverge.
Fix: the findingCriteria field-name validation shared by CreateFilter/UpdateFilter in the guardduty crate (src/ops/create_filter.rs and the criteria validator it calls).
Traces: guardduty.create-filter-invalid-criteria-field-rejected.

Cause 3 of 7: UpdateFindingsFeedback with a non-modeled Feedback value.
AWS: Smithy UpdateFindingsFeedback.Feedback targets the Feedback enum (required); errors BadRequestException 400, InternalServerErrorException 500. Recorded behavior: an off-enum feedback value is refused HTTP 400 BadRequestException with the fixed text 'The request is rejected because the JSON could not be processed.' — AWS rejects it as an unparseable payload, it does not enumerate the allowed values.
Emulator: 'The request is rejected because the parameter feedback has an invalid value: it must be one of USEFUL, NOT_USEFUL.'
Why: the emulator reclassifies a payload/deserialization rejection as a semantic parameter rejection and discloses the enum; clients branching on the real text treat the call as retryable-after-reserialize and never see that signal.
Fix: the enum deserialization path for UpdateFindingsFeedback in the guardduty crate (src/ops/update_findings_feedback.rs / the request deserializer's enum handling).
Traces: guardduty.findings-feedback-invalid-value-rejected.

Cause 4 of 7: CreateMembers with a malformed account id.
AWS: Smithy CreateMembers.AccountDetails (list 1..50, required); BadRequestException 400. Recorded behavior: a malformed AccountId is refused with the fixed text "The request failed because the 'accountId' was formatted incorrectly." (single-quoted member name included).
Emulator: 'The request is rejected because the parameter accountId has an invalid value.'
Why: the format-rejection class is indistinguishable from the emulator's generic invalid-parameter text, so clients cannot tell a syntactically bad account id from a semantically rejected one.
Fix: the AccountId format validation in the CreateMembers handler in the guardduty crate (src/ops/create_members.rs).
Traces: guardduty.create-members-malformed-account-id-rejected.

Cause 5 of 7: DisassociateMembers with malformed account ids.
AWS: Smithy DisassociateMembers.AccountIds (list 1..50, required); BadRequestException 400. Recorded behavior: refused with the fixed text 'The request failed because parameter accountIds has accountId values that are formatted incorrectly.' — note AWS names the LIST member (accountIds) for the batch operation, a different sentence from the single-account one in cause 4.
Emulator: 'The request is rejected because the parameter accountId has an invalid value.' — the same generic text it uses everywhere, naming the wrong member.
Why: the batch and single-account rejection classes are conflated and neither matches AWS, so a caller cannot tell which element of the batch AWS objected to.
Fix: the AccountIds validation in the DisassociateMembers handler in the guardduty crate (src/ops/disassociate_members.rs). Note: open issue 20261029T165950Z-guardduty-disassociatemembers-wrong-behavior.md concerns only the monotonic clock and does not cover this message.
Traces: guardduty.disassociate-members-not-a-member-rejected.

Cause 6 of 7: duplicate CreateCustomDetectionRuleAssociation conflict, and the not-found message for an unknown RuleId.
AWS: Smithy CreateCustomDetectionRuleAssociation (RuleId pattern ^[a-z0-9]+(-[a-z0-9]+)*$, required; errors include ConflictException 409 and ResourceNotFoundException 404). Recorded behavior: a second association for the same rule is refused with the message 'Association already exists for rule <RuleId>' — fixed prefix 'Association already exists for rule ' plus the RuleId taken from the request; and an unknown RuleId is refused ResourceNotFoundException with 'Rule not found: <RuleId>' — fixed prefix 'Rule not found: ' plus the request's RuleId.
Emulator: 'The request is rejected because the current account already has an association for the given ruleId and mode.' for the conflict, and 'The request is rejected because the input ruleId does not refer to a custom detection rule published by GuardDuty.' for the unknown rule — neither interpolates the RuleId AWS echoes.
Why: both texts are invented, and because they drop the RuleId a caller handling a batch of associations cannot tell which rule conflicted or was missing.
Fix: the conflict and rule-lookup paths of the CreateCustomDetectionRuleAssociation handler in the guardduty crate (src/ops/create_custom_detection_rule_association.rs).
Traces: guardduty.create-duplicate-custom-detection-rule-association-conflict, guardduty.create-custom-detection-rule-association-duplicate-name-conflict, guardduty.create-custom-detection-rule-association-without-permission-denied.

Cause 7 of 7: duplicate CreateMalwareProtectionPlan conflict.
AWS: Smithy CreateMalwareProtectionPlan (ProtectedResource required; modeled ConflictException 409). Recorded behavior: creating a second plan for an already-protected S3 bucket is refused with the fixed text 'The request was rejected because the provided S3 bucket is already protected.' (fixed text; the bucket name is not interpolated).
Emulator: 'The request is rejected because a Malware Protection plan already exists for the specified protected resource.'
Why: the resource-already-protected refusal AWS words in terms of the S3 bucket is replaced by generic prose, so clients keyed to it mis-handle the idempotent-create path.
Fix: the duplicate-protected-resource check in the CreateMalwareProtectionPlan handler in the guardduty crate (src/ops/create_malware_protection_plan.rs).
Traces: guardduty.create-duplicate-malware-protection-plan-conflict, guardduty.create-malware-protection-plan-duplicate-conflict.

## Evidence
Replayed check assertions (expected = real AWS, got = emulator); error codes matched in every case:
- filter-rank-out-of-range-rejected: got {'Code': 'BadRequestException', 'Message': 'The request is rejected because the parameter rank has an invalid value: it must be less than or equal to 100.'}, expected Message 'The request is rejected because the one or more input parameters have invalid values.'
- create-filter-invalid-criteria-field-rejected: expected 'The request was rejected because the parameter findingCriteria has an invalid value.', got "findingCriteria.criterion: 'this.field.does.not.exist' is not a field of a GuardDuty finding, so no finding can be selected or ordered by it."
- findings-feedback-invalid-value-rejected: expected 'The request is rejected because the JSON could not be processed.', got 'The request is rejected because the parameter feedback has an invalid value: it must be one of USEFUL, NOT_USEFUL.'
- create-members-malformed-account-id-rejected: expected "The request failed because the 'accountId' was formatted incorrectly.", got 'The request is rejected because the parameter accountId has an invalid value.'
- disassociate-members-not-a-member-rejected: expected 'The request failed because parameter accountIds has accountId values that are formatted incorrectly.', got 'The request is rejected because the parameter accountId has an invalid value.'
- create-duplicate-custom-detection-rule-association-conflict / create-custom-detection-rule-association-duplicate-name-conflict: expected 'Association already exists for rule cloudtrail-logging-disabled', got 'The request is rejected because the current account already has an association for the given ruleId and mode.'
- create-custom-detection-rule-association-without-permission-denied: code ResourceNotFoundException matched; expected 'Rule not found: 1d6468139ca547399612b508fff653cd', got 'The request is rejected because the input ruleId does not refer to a custom detection rule published by GuardDuty.'
- create-duplicate-malware-protection-plan-conflict / create-malware-protection-plan-duplicate-conflict: expected 'The request was rejected because the provided S3 bucket is already protected.', got 'The request is rejected because a Malware Protection plan already exists for the specified protected resource.'

## Steps to Reproduce
1. Run the guardduty behavioral checks named in trace_ids against the emulator.
2. Manual repro: CreateDetector, then (a) CreateFilter with Rank=150; (b) CreateFilter with FindingCriteria={'Criterion': {'this.field.does.not.exist': {'Eq': ['x']}}}; (c) UpdateFindingsFeedback with Feedback='NOT_A_VALUE'; (d) CreateMembers with AccountDetails=[{'AccountId':'12345','Email':'a@example.com'}]; (e) DisassociateMembers with AccountIds=['12345']; (f) CreateCustomDetectionRuleAssociation twice for a rule from ListCustomDetectionRules, and once with a random 32-hex RuleId; (g) CreateMalwareProtectionPlan twice for the same S3 bucket.
3. Compare each Error.Message with the AWS text quoted above.

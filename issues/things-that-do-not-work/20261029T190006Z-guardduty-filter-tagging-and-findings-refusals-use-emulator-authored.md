Issue-Source: trace-sweep-divergence
Issue-Trace: guardduty-filter-and-detector-error-paths
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: guardduty-filter-and-detector-error-paths guardduty-filter-error-paths guardduty-filter-lifecycle guardduty-filter-lifecycle-with-criteria guardduty-multiple-filters-isolation guardduty-error-handling-invalid-inputs guardduty-tag-nonexistent-detector-fails guardduty-findings-archive-unarchive-workflow guardduty-findings-error-paths guardduty-list-malware-protection-plans
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|GetFilter|4xx->4xx|$.message
Issue-Key: guardduty|UpdateFilter|4xx->4xx|$.message
Issue-Key: guardduty|DeleteFilter|4xx->4xx|$.message
Issue-Key: guardduty|CreateFilter|4xx->4xx|$.message
Issue-Key: guardduty|ListTagsForResource|4xx->4xx|$.message
Issue-Key: guardduty|TagResource|4xx->4xx|$.message
Issue-Key: guardduty|ArchiveFindings|4xx->4xx|$.message
Issue-Key: guardduty|UnarchiveFindings|4xx->4xx|$.message
Issue-Key: guardduty|ListMalwareProtectionPlans|4xx->4xx|$.message
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# Filter, tagging and findings refusals use emulator-authored messages

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFilter

## Description
On the filter, tagging, findings-metadata and malware-plan-listing surfaces the emulator returns the right status (400) and the right error class but replaces GuardDuty's fixed refusal sentences with its own prose, usually one that asserts an account-ownership or per-parameter fault the request did not have. 9 causes.

Cause 1 of 9: GetFilter on a filter name that does not exist.
AWS: HTTP 400, BadRequestException with $.__type/$.type 'InvalidInputException', message 'The request is rejected since no such resource found.' — entirely fixed text, naming no member and no value. Smithy: GetFilter models BadRequestException(400) and InternalServerErrorException(500) only.
Emulator: 'The request is rejected because the input filterName is not owned by the current account.'
Why: the emulator asserts a cross-account ownership problem for a filter nobody owns; a caller debugging a 400 is pointed at IAM/account configuration instead of a missing filter.
Fix: the GetFilter handler's not-found refusal message.
Traces: guardduty-filter-and-detector-error-paths#task#4.

Cause 2 of 9: UpdateFilter on a filter name that does not exist.
AWS: HTTP 400, InvalidInputException, fixed message 'The request is rejected because the given filter name is invalid.'
Emulator: 'The request is rejected because the input filterName is not owned by the current account.'
Why: as cause 1; note AWS uses a different fixed sentence for the mutating filter paths than for GetFilter, and the emulator collapses both into one.
Fix: the UpdateFilter handler's refusal message.
Traces: guardduty-filter-and-detector-error-paths#task#5.

Cause 3 of 9: DeleteFilter on a filter name that does not exist.
AWS: HTTP 400, InvalidInputException, fixed message 'The request is rejected because the given filter name is invalid.'
Emulator: 'The request is rejected because the input filterName is not owned by the current account.'
Why: as cause 2.
Fix: the DeleteFilter handler's refusal message.
Traces: guardduty-filter-error-paths#task#6.

Cause 4 of 9: CreateFilter with a findingCriteria criterion key that is not a finding field.
AWS: HTTP 400, InvalidInputException, fixed message 'The request was rejected because the parameter findingCriteria has an invalid value.' The message names the input member (findingCriteria) and nothing else — it does not quote the offending key.
Emulator: emits its own diagnostic, "findingCriteria.criterion: '<key>' is not a field of a GuardDuty finding, so no finding can be selected or ordered by it."
Why: a client asserting on AWS's documented validation sentence does not match; worse, the emulator's text leaks an internal explanation that AWS never emits, so tests written against the emulator break on real AWS.
Fix: CreateFilter input validation — report the modeled findingCriteria refusal text.
Traces: guardduty-filter-error-paths#task#2.

Cause 5 of 9: ListTagsForResource with a resourceArn that is not a GuardDuty resource.
AWS: HTTP 400, InvalidInputException, fixed message 'Invalid input resource arn'.
Emulator: 'The request is rejected because the input resourceArn is not owned by the current account.'
Why: a malformed/foreign ARN is reported as an ownership failure, so a caller cannot tell a typo in the ARN from a cross-account permission problem.
Fix: the ListTagsForResource handler's ARN validation and its refusal message.
Traces: guardduty-error-handling-invalid-inputs#task#4.

Cause 6 of 9: TagResource with a resourceArn naming a detector that does not exist.
AWS: HTTP 400, InvalidInputException, fixed message 'Invalid input resource arn'.
Emulator: 'The request is rejected because the input resourceArn is not owned by the current account.'
Why: as cause 5, on the write side of the tag surface.
Fix: the TagResource handler's ARN validation and its refusal message.
Traces: guardduty-tag-nonexistent-detector-fails#task#1.

Cause 7 of 9: ArchiveFindings with finding ids that are not valid.
AWS: HTTP 400, InvalidInputException, fixed message 'The request is rejected because an invalid or out-of-range value is specified as an input parameter.' — the generic input-parameter sentence, naming no member.
Emulator: 'The request is rejected because the parameter findingIds has an invalid value.'
Why: the emulator names a member AWS deliberately does not, so a client matching the documented sentence misses the error class entirely.
Fix: the ArchiveFindings input validation refusal message.
Traces: guardduty-findings-archive-unarchive-workflow#task#2.

Cause 8 of 9: UnarchiveFindings with finding ids that are not valid.
AWS: HTTP 400, InvalidInputException, fixed message 'The request is rejected because an invalid or out-of-range value is specified as an input parameter.'
Emulator: 'The request is rejected because the parameter findingIds has an invalid value.'
Why: as cause 7.
Fix: the UnarchiveFindings input validation refusal message.
Traces: guardduty-findings-archive-unarchive-workflow#task#3.

Cause 9 of 9: ListMalwareProtectionPlans with an unusable nextToken.
AWS: HTTP 400, InvalidInputException, fixed message 'The request is rejected because an invalid or out-of-range value is specified as an input parameter.'
Emulator: 'The request is rejected because the parameter nextToken has an invalid value.'
Why: as cause 7, on the pagination surface.
Fix: the ListMalwareProtectionPlans page-token validation refusal message.
Traces: guardduty-list-malware-protection-plans#task#2.

Related open issues (neither covers these; neither carries a replay key for them): 20261029T183757Z-guardduty-invalid-input-and-already-exists-refusals-use-emulator.md and 20261029T040138Z-guardduty-getusagestatistics-pagination.md.

## Evidence
Cause 1: $.message expected 'The request is rejected since no such resource found.', got 'The request is rejected because the input filterName is not owned by the current account.' (status 400 both sides)
Causes 2-3: expected 'The request is rejected because the given filter name is invalid.', got 'The request is rejected because the input filterName is not owned by the current account.'
Cause 4: expected 'The request was rejected because the parameter findingCriteria has an invalid value.', got "findingCriteria.criterion: 'not_a_real_field' is not a field of a GuardDuty finding, so no finding can be selected or ordered by it."
Causes 5-6: expected 'Invalid input resource arn', got 'The request is rejected because the input resourceArn is not owned by the current account.'
Causes 7-9: expected 'The request is rejected because an invalid or out-of-range value is specified as an input parameter.', got 'The request is rejected because the parameter <findingIds|nextToken> has an invalid value.'

## Steps to Reproduce
1. Replay guardduty-filter-and-detector-error-paths steps task#4 (GetFilter) and task#5 (UpdateFilter), and guardduty-filter-error-paths steps task#2 (CreateFilter bad criterion key) and task#6 (DeleteFilter).
2. Replay guardduty-error-handling-invalid-inputs step task#4 (ListTagsForResource) and guardduty-tag-nonexistent-detector-fails step task#1 (TagResource).
3. Replay guardduty-findings-archive-unarchive-workflow steps task#2 and task#3, and guardduty-list-malware-protection-plans step task#2.
4. In each case the status is 400 as expected but $.message is the emulator's own sentence.

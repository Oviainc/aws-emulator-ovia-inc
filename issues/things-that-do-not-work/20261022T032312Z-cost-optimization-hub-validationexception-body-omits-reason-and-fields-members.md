Issue-Source: trace-sweep-divergence
Issue-Trace: cost-optimization-hub-enrollment-status-validation-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cost-optimization-hub-enrollment-status-validation-errors cost-optimization-hub-list-recommendations-filter-errors
Issue-Shape-Id: com.amazonaws.costoptimizationhub#CostOptimizationHubService
Issue-Key: cost-optimization-hub|ListEnrollmentStatuses|4xx->4xx|$.fields
Issue-Key: cost-optimization-hub|ListEnrollmentStatuses|4xx->4xx|$.message
Issue-Key: cost-optimization-hub|ListEnrollmentStatuses|4xx->4xx|$.reason
Issue-Key: cost-optimization-hub|UpdateEnrollmentStatus|4xx->4xx|$.fields
Issue-Key: cost-optimization-hub|UpdateEnrollmentStatus|4xx->4xx|$.message
Issue-Key: cost-optimization-hub|UpdateEnrollmentStatus|4xx->4xx|$.reason
Issue-Key: cost-optimization-hub|ListRecommendations|4xx->4xx|$.fields
Issue-Key: cost-optimization-hub|ListRecommendations|4xx->4xx|$.message
Issue-Key: cost-optimization-hub|ListRecommendations|4xx->4xx|$.reason
Issue-Commit: f81229b87ab2500fc3bfee437baf8654766d98fe

# ValidationException body omits reason and fields members

> _Note: this issue was identified against the code base at commit `f81229b87ab2500fc3bfee437baf8654766d98fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListEnrollmentStatuses

## Description
Cost Optimization Hub's ValidationException is a structured error: besides the required `message` it carries `reason` (ValidationExceptionReason enum) and `fields` (list of ValidationExceptionDetail, each with required `fieldName` and `message`). The emulator rejects bad input with the right HTTP status and error shape name, but serializes ONLY a generic `message` and drops `reason` and `fields` entirely, on every operation that validates input. Three recorded causes, one root cause in the shared validation/error-serialization path.

Cause 1 of 3: ListEnrollmentStatuses with accountId='12345'.
AWS: Smithy: ListEnrollmentStatuses.accountId targets AccountId @pattern ^[0-9]{12}$; modeled error ValidationException (client, httpError 400) with members message (required), reason -> ValidationExceptionReason, fields -> list<ValidationExceptionDetail{fieldName required, message required}>. The recording shows HTTP 400 with reason='FieldValidationFailed' and fields=[{fieldName:'accountId', message:'Member must satisfy regular expression pattern: [0-9]{12}'}]; the top-level message is the fixed summary 'Input validation failed. Please check the field list for more details.' (example text), with the per-member detail carried in fields.
Emulator: HTTP 400, but body has no `reason` and no `fields`; the whole diagnosis is crammed into message as "1 validation error detected: Value '12345' at 'accountId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]{12}$".
Why: an SDK client reading ValidationException.reason or iterating ValidationException.fields to map errors back to form members gets None/empty and cannot tell which member failed except by scraping prose; code that switches on reason never matches FieldValidationFailed.
Fix: the service's ValidationException construction/serializer plus the per-operation input validation that raises it, so a constraint failure records fieldName + constraint message into `fields` and sets `reason`.
Traces: cost-optimization-hub-enrollment-status-validation-errors#task#1 (also task#2 in the same cluster).

Cause 2 of 3: UpdateEnrollmentStatus with status='Enabled'.
AWS: Smithy: UpdateEnrollmentStatus.status is required, targets enum EnrollmentStatus; modeled error ValidationException (400). Recording: HTTP 400 with reason='FieldValidationFailed' and fields=[{fieldName:'status', message:'Member must satisfy enum value set: ...'}]. Requirement is the presence of reason and of a fields entry naming the member and its enum-set constraint; the exact value list in that text is an example only (the on-disk model's EnrollmentStatus enum is [Active, Inactive], while the recording's message text lists a third value), so the message difference alone is not the defect.
Emulator: HTTP 400 with message only; `reason` and `fields` absent.
Why: same as cause 1 — the member that failed is not machine-readable.
Fix: same validation/error-serialization path, for the enum-constraint case.
Traces: cost-optimization-hub-enrollment-status-validation-errors#task#3.

Cause 3 of 3: ListRecommendations with filter.accountIds containing '12345'.
AWS: Smithy: ListRecommendations.filter -> Filter.accountIds -> AccountIdList (@length 1..100) of AccountId @pattern ^[0-9]{12}$; modeled error ValidationException (400). Recording: HTTP 400 with reason='FieldValidationFailed' and fields=[{fieldName:'accountIds', message:'Member must satisfy constraint: [Member must satisfy regular expression pattern: [0-9]{12}]'}] — i.e. the nested list member violation is reported against the containing member name.
Emulator: HTTP 400 with message only ("...Value '12345' at 'filter.accountIds.1.member' failed to satisfy constraint..."); `reason` and `fields` absent.
Why: same as above, and additionally a client cannot attribute the failure to the accountIds filter member.
Fix: same path; nested list/structure constraint failures must still emit a `fields` entry for the named member.
Traces: cost-optimization-hub-list-recommendations-filter-errors#task#3 (also task#4 in the same cluster).

## Evidence
Cause 1 — fixture cost-optimization-hub-enrollment-status-validation-errors step task#1, ListEnrollmentStatuses. Expected 400 body: reason='FieldValidationFailed', fields=[{fieldName:'accountId', message:'Member must satisfy regular expression pattern: [0-9]{12}'}], message='Input validation failed. Please check the field list for more details.'. Actual 400 body: $.reason missing, $.fields missing, message="1 validation error detected: Value '12345' at 'accountId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]{12}$". Cluster also covers task#2.

Cause 2 — same fixture, step task#3, UpdateEnrollmentStatus status='Enabled'. Expected 400 body: reason='FieldValidationFailed', fields=[{fieldName:'status', message:'Member must satisfy enum value set: [...]'}]. Actual 400 body: $.reason missing, $.fields missing, message="1 validation error detected: Value 'Enabled' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [Active, Inactive]".

Cause 3 — fixture cost-optimization-hub-list-recommendations-filter-errors step task#3, ListRecommendations. Expected 400 body: reason='FieldValidationFailed', fields=[{fieldName:'accountIds', message:'Member must satisfy constraint: [Member must satisfy regular expression pattern: [0-9]{12}]'}]. Actual 400 body: $.reason missing, $.fields missing, message names 'filter.accountIds.1.member'. Cluster also covers task#4.

In all three the HTTP status (400) and the error shape match; only the modeled members reason/fields are absent. Model consulted: services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/smithy-model/com-amazonaws-costoptimizationhub-costoptimizationhubservice.json (ValidationException, ValidationExceptionDetail, EnrollmentStatus, Filter).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture cost-optimization-hub-enrollment-status-validation-errors, step task#1: ListEnrollmentStatuses with accountId='12345'. Observe HTTP 400 whose JSON body has no `reason` and no `fields`.
3. Same fixture, step task#3: UpdateEnrollmentStatus with status='Enabled'. Observe HTTP 400 with no `reason`/`fields`.
4. Replay fixture cost-optimization-hub-list-recommendations-filter-errors, step task#3: ListRecommendations with filter.accountIds=[<valid 12-digit id>, '12345']. Observe HTTP 400 with no `reason`/`fields`.
5. Expected in every case: reason='FieldValidationFailed' and a fields[] entry naming the offending member and its constraint.

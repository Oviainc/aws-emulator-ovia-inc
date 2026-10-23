Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: guardduty.filter-rank-out-of-range-rejected guardduty.findings-feedback-invalid-value-rejected
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty.filter-rank-out-of-range-rejected|run#6|AssertionError|assert err["Code"] == "BadRequestException", err
Issue-Key: guardduty.findings-feedback-invalid-value-rejected|run#7|AssertionError|assert err["Code"] == "BadRequestException", (
Issue-Commit: ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c

# GuardDuty constraint violations are refused as ValidationException, a shape GuardDuty does not model, instead of BadRequestException (2 causes)

> _Note: this issue was identified against the code base at commit `ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFilter, UpdateFindingsFeedback

## Description
The emulator does enforce modeled input constraints for GuardDuty, but refuses them through the framework's generic pre-handler constraint channel, which emits error code `ValidationException` with a Smithy-style "N validation error(s) detected: ..." message. `ValidationException` is not a shape the GuardDuty model declares for any operation; GuardDuty's only client-error shapes are BadRequestException (400), AccessDeniedException (403), ResourceNotFoundException (404) and ConflictException (409). So every constraint refusal in this service is delivered under a code no GuardDuty client can catch. Two causes below; they share a mechanism but are two distinct operation/constraint contracts, and both must be closed.

Cause 1 of 2 — CreateFilter Rank outside 1-100.
AWS: Smithy: CreateFilter.Rank -> FilterRank, @range min 1 max 100; CreateFilter declares errors BadRequestException (httpError 400) and InternalServerErrorException (500) and nothing else. Recorded behavior: CreateFilter with Rank=150 is refused with error code BadRequestException and no filter is left behind under that name, and an immediately following CreateFilter with the same Name and a valid Rank succeeds.
Emulator: error code `ValidationException`, message "1 validation error detected: Value '150' at 'rank' failed to satisfy constraint: Member must have value less than or equal to 100".
Why: botocore builds the exception class from the error code, so `client.exceptions.BadRequestException` never matches and a caller's GuardDuty-specific except block falls through as an unmodeled error; code that branches on the GuardDuty error set treats a plain out-of-range Rank as an unknown failure rather than a fixable input error.
Fix: the guardduty service's constraint-violation -> error mapping (the crate's `scaffolded`/validation error channel as wired for this service, plus the CreateFilter handler's own range check) must render constraint refusals as the modeled BadRequestException with httpError 400, not the generic ValidationException. The message rule is the service's own ("The request was rejected because the parameter ... has an invalid value."-style text built from the offending member); the recorded message is an example, the code and status are the requirement.
Traces: guardduty.filter-rank-out-of-range-rejected.

Cause 2 of 2 — UpdateFindingsFeedback Feedback outside the USEFUL/NOT_USEFUL enum.
AWS: Smithy: UpdateFindingsFeedback.Feedback -> Feedback, required enum (USEFUL, NOT_USEFUL); UpdateFindingsFeedback declares errors BadRequestException (400) and InternalServerErrorException (500) only. Recorded behavior: Feedback='MAYBE_USEFUL' is refused with error code BadRequestException, the finding's Service.UserFeedback is left unset by the rejected call, and a following valid feedback call on the same finding succeeds.
Emulator: error code `ValidationException`, message "1 validation error detected: Value 'MAYBE_USEFUL' at 'feedback' failed to satisfy constraint: Member must satisfy enum value set: [USEFUL, NOT_USEFUL]".
Why: same client consequence as cause 1 — the refusal arrives under a code absent from GuardDuty's model, so SDK-generated error types and error-code switches do not match it; an out-of-enum feedback value is misreported as an unknown/service-side failure.
Fix: same component as cause 1 for the enum-constraint path, reached via the UpdateFindingsFeedback handler's validation; emit BadRequestException/400.
Traces: guardduty.findings-feedback-invalid-value-rejected.

## Evidence
guardduty.filter-rank-out-of-range-rejected [mismatch]:
  assert err["Code"] == "BadRequestException"
  AssertionError: {'Message': "1 validation error detected: Value '150' at 'rank' failed to satisfy constraint: Member must have value less than or equal to 100", 'Code': 'ValidationException'}

guardduty.findings-feedback-invalid-value-rejected [mismatch]:
  AssertionError: expected BadRequestException for an out-of-enum Feedback value, got {'Message': "1 validation error detected: Value 'MAYBE_USEFUL' at 'feedback' failed to satisfy constraint: Member must satisfy enum value set: [USEFUL, NOT_USEFUL]", 'Code': 'ValidationException'}

Model evidence (services/service-com-amazonaws-guardduty-guarddutyapiservice/smithy-model/com-amazonaws-guardduty-guarddutyapiservice.json):
  CreateFilter.errors = [BadRequestException(400), InternalServerErrorException(500)]; Rank @range {min:1,max:100}
  UpdateFindingsFeedback.errors = [BadRequestException(400), InternalServerErrorException(500)]; Feedback required enum
  No operation in the GuardDuty model declares a ValidationException shape.

The generic channel is the documented scaffold behavior: op-stub files state "To refuse a bad request this operation models no error for, return Err(crate::scaffolded::Scaffolded::validation(\"..\")) -- a 400 ValidationException" (see e.g. services/service-com-amazonaws-s3-amazons3/src/ops/put_public_access_block.rs lines 194-198). For GuardDuty that channel is the wrong one, because GuardDuty does model an error for these refusals.

## Steps to Reproduce
1. Boot the emulator and point a botocore guardduty client at it.
2. CreateDetector(Enable=true).
3. Cause 1: CreateFilter(DetectorId=..., Name=<name>, Rank=150, FindingCriteria={'Criterion': {'severity': {'GreaterThanOrEqual': 1}}}) -> observe Error.Code == 'ValidationException'; AWS answers 400 BadRequestException.
4. Cause 2: CreateSampleFindings and poll ListFindings/GetFindings for a finding id, then UpdateFindingsFeedback(DetectorId=..., FindingIds=[<id>], Feedback='MAYBE_USEFUL') -> observe Error.Code == 'ValidationException'; AWS answers 400 BadRequestException.
Or replay guardduty.filter-rank-out-of-range-rejected and guardduty.findings-feedback-invalid-value-rejected.

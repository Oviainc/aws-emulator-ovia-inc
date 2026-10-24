Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0

# GetLatestAssessmentId answers 404 ResourceNotFoundException, an error the operation does not model, when the account has no assessment

_Filed: 2026-10-24T16:17:52Z_
_Operation: GetLatestAssessmentId | Class: error-response_

> _Note: this issue was identified against the code base at commit `ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: GET /get-latest-assessment-id in an account that has never started an assessment is refused with one of the errors the operation actually declares. The only client-side refusal com.amazonaws.migrationhubstrategy#GetLatestAssessmentId models is ValidationException (httpError 400); the operation declares AccessDeniedException (403), DependencyException (500), InternalServerException (500) and ValidationException (400) and nothing else. That ValidationException is declared on an operation whose input shape (GetLatestAssessmentIdRequest) has ZERO members is the telling detail: there is no request body a caller could get wrong, so the 400 can only be the service's refusal for the state condition "this account has no assessment to report" — which is exactly the condition the handler is answering.

Actual: the handler widens ResourceNotFoundException into the error channel with `extra_errors = ["ResourceNotFoundException"]` and answers HTTP 404 with `__type` ResourceNotFoundException (`crate::state::no_latest_assessment()`). The status line and the error code on the wire both differ from the modeled surface: a client that branches on the operation's declared failure modes (retry on DependencyException, surface ValidationException to the user) sees an error code the operation never declares, and anything that keys on the HTTP status sees 404 where the model admits only 403/400/500.

The in-code justification for the choice is explicit — "never a validation refusal, which an input-less operation cannot earn" (src/ops/get_latest_assessment_id.rs:79-83) — but it is contradicted by the model itself, which puts ValidationException on precisely that input-less operation. Note the sibling widenings in this crate cite recorded wire behaviour (GetAssessment widens ValidationException "the recorded GET /get-assessment/{id} refusal ... is HTTP 400 ValidationException"); this one cites none, and it widens in the opposite direction.

## Evidence

Smithy model (smithy_query query=operation operation=GetLatestAssessmentId): input_shape GetLatestAssessmentIdRequest has no members; http GET /get-latest-assessment-id -> 200; errors = AccessDeniedException(403), DependencyException(500), InternalServerException(500), ValidationException(400). ResourceNotFoundException is NOT in that list (contrast GetPortfolioPreferences, GetImportFileTask, GetServerDetails, which all do declare it — so the model is discriminating here, not merely incomplete).

Handler: services/service-com-amazonaws-migrationhubstrategy-awsmigrationhubstrategyrecommendation/src/ops/get_latest_assessment_id.rs:84-87 (`extra_errors = ["ResourceNotFoundException"]`) and :163-173, where `latest_assessment_id == None` returns `GetLatestAssessmentIdErrorModeled::ResourceNotFoundException(crate::state::no_latest_assessment())`. `crate::state::no_latest_assessment()` builds the 404 shape; `state.latest_assessment_id` is only ever set by StartAssessment (src/ops/start_assessment.rs:261), so on a freshly booted emulator EVERY call to this operation takes the 404 path — it is the common case, not an edge case.

Test coverage: tests/all/ has no assertion pinning this status/code for the empty-account case (assessment_error_paths.rs covers GetAssessment/StopAssessment id refusals; import_and_assess.rs exercises GetLatestAssessmentId only after a StartAssessment has recorded an id), so nothing in the suite asserts the 404 as intended behaviour.

Backlog: no open or fixed issue mentions GetLatestAssessmentId (the nearest, 20261024T153403Z-migrationhubstrategy-stopassessment-validation.md, is the same class of divergence on a different operation — 404 ResourceNotFoundException where the modeled refusal is 400 ValidationException — which confirms this class is in scope and is not already filed for this operation).

## Steps to Reproduce

1. Start the emulator with a fresh state store for migrationhub-strategy (no StartAssessment has been called, so state.latest_assessment_id is None).
2. GET /get-latest-assessment-id (aws migrationhubstrategy get-latest-assessment-id).
3. Observe the response: HTTP 404 with body {"message": ...} and x-amzn-errortype/__type ResourceNotFoundException.
4. Compare against the operation's modeled error set — AccessDeniedException(403), DependencyException(500), InternalServerException(500), ValidationException(400) — which does not contain ResourceNotFoundException; the account-has-no-assessment refusal should be reported through a modeled failure (the 400 ValidationException the model declares on this input-less operation) rather than an undeclared 404.
5. For contrast, call POST /start-assessment first and then GET /get-latest-assessment-id: it answers 200 with the id, confirming the 404 path is driven purely by the empty-account state condition.

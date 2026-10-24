Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 43c3b52fd22125f2604949f65e29a8c5612a98db

# PutPortfolioPreferences never answers ConflictException (409) while an assessment or import task is in progress

_Filed: 2026-10-24T07:04:50Z_
_Operation: PutPortfolioPreferences | Class: error-response_

> _Note: this issue was identified against the code base at commit `43c3b52fd22125f2604949f65e29a8c5612a98db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

PutPortfolioPreferences models ConflictException (HTTP 409), documented "Exception to indicate that there is an ongoing task when a new task is created. Return when once the existing tasks are complete." Portfolio preferences are the inputs the service's recommendations are computed from, so AWS refuses a save while a task that consumes them is still running and the caller has to wait for the ongoing task to finish.

Expected: POST /put-portfolio-preferences issued while the account has an assessment in IN_PROGRESS (or an import file task in ImportInProgress / a recommendation report generation task in IN_PROGRESS) is refused with HTTP 409 ConflictException, and the stored preferences are left exactly as they were.

Actual: the handler checks enum membership on applicationMode/applicationPreferences/databasePreferences and then unconditionally overwrites `store.portfolio_preferences`, answering HTTP 200 regardless of what tasks the store holds. ConflictException is a constructible variant of PutPortfolioPreferencesErrorModeled that is never built anywhere in the crate, so the 409 branch of this operation is unreachable — a caller can silently change the preferences underneath a running assessment and can never observe the conflict AWS reports.

## Evidence

Smithy model: PutPortfolioPreferences errors = AccessDeniedException(403), ConflictException(409), InternalServerException(500), ThrottlingException(429), ValidationException(400); com.amazonaws.migrationhubstrategy#ConflictException doc = "Exception to indicate that there is an ongoing task when a new task is created. Return when once the existing tasks are complete." (httpError 409, required `message`).
Handler: src/ops/put_portfolio_preferences.rs:248-290 — validation of enum values, then `state.lock().portfolio_preferences = Some(PortfolioPreferences { ... })` with no inspection of store.assessments / store.import_tasks / store.recommendation_reports and no in-progress gate. Grep of the crate finds no construction of ConflictException for this operation.
State model: in-progress is already a first-class notion in src/state.rs (Assessment::is_terminal / completion_due, ImportFileTask::is_complete, RecommendationReport::is_in_progress, in_progress_report_id), so the predicate the gate needs already exists and is used by StartRecommendationReportGeneration.
Tests: tests/all/portfolio_preferences.rs covers save/read round-trip, whole-record replacement and enum refusals (a_save_replaces_the_whole_record, a_value_the_model_does_not_declare_is_refused_and_saves_nothing) on a fresh emulator with no task in flight; no test asserts a 409 for a save during an ongoing task.

## Steps to Reproduce

1. POST /start-assessment {"s3bucketForAnalysisData":"migrationhub-strategy-bkt"} -> 200 with an assessmentId; GET /get-assessment/{id} shows dataCollectionDetails.status == "IN_PROGRESS".
2. Immediately POST /put-portfolio-preferences {"applicationMode":"ALL"} -> expected HTTP 409 with x-amzn-errortype ConflictException and the previously stored preferences untouched; emulator answers HTTP 200 and replaces the whole preferences record.
3. GET /get-portfolio-preferences -> reports the newly saved preferences even though the assessment that consumes them is still running.
4. Repeat with an import file task still in ImportInProgress (POST /start-import-file-task then immediately POST /put-portfolio-preferences) -> same divergence.

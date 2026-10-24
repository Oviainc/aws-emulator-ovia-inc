Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 43c3b52fd22125f2604949f65e29a8c5612a98db

# StartAssessment accepts assessmentDataSourceType and assessmentTargets[].condition values the enumerations do not declare

_Filed: 2026-10-24T07:04:50Z_
_Operation: StartAssessment | Class: validation_

> _Note: this issue was identified against the code base at commit `43c3b52fd22125f2604949f65e29a8c5612a98db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartAssessment's assessmentDataSourceType targets the AssessmentDataSourceType enumeration (StrategyRecommendationsApplicationDataCollector | ManualImport | ApplicationDiscoveryService) and each assessmentTargets entry's required `condition` targets the Condition enumeration (EQUALS | NOT_EQUALS | CONTAINS | NOT_CONTAINS). AWS refuses a value the enumeration does not declare with a 400 ValidationException and mints no assessment.

Expected: POST /start-assessment carrying assessmentDataSourceType="Bogus", or assessmentTargets=[{"condition":"LIKE","name":"OS_NAME","values":["Linux"]}], is refused with HTTP 400 (ValidationException, this service's "Invalid request body" sentence) and leaves no assessment behind.

Actual: the handler validates ONLY the two S3 bucket members and then persists the request verbatim — `assessment_data_source_type: input.assessment_data_source_type` and `assessment_targets: input.assessment_targets.unwrap_or_default()` — so the call answers HTTP 200 with an assessmentId and GetAssessment echoes the undeclared values back in assessmentTargets. This crate's own handlers document that the boundary validator does NOT enforce enum membership and check it themselves (ListServers::filter_of/descending/check_group_filter, PutPortfolioPreferences::check_enum, UpdateServerConfig::check_enum, StartRecommendationReportGeneration's OUTPUT_FORMAT_VALUES/GROUP_NAME_VALUES), so StartAssessment is inconsistent with the rest of the service as well as with AWS.

Note this is a different failure mode from the already-open issue 20261023T004429Z (StartAssessment accepting a start in an account with no registered collector): that one is a missing account-state precondition, this one is unchecked enum membership on two request members.

## Evidence

Smithy model: StartAssessmentRequest$assessmentDataSourceType -> com.amazonaws.migrationhubstrategy#AssessmentDataSourceType (values StrategyRecommendationsApplicationDataCollector, ManualImport, ApplicationDiscoveryService); StartAssessmentRequest$assessmentTargets -> list<AssessmentTarget> (@length 0..10) whose `condition` member is @required and targets Condition (EQUALS, NOT_EQUALS, CONTAINS, NOT_CONTAINS). The service models ValidationException (httpError 400, "The request body isn't valid."), and this operation's handler already widens it in for its bucket checks, so the error channel exists.
Handler: src/ops/start_assessment.rs:174-245 — only require_bucket/check_bucket_usable run; no enum check before `store.assessments.insert(...)`.
Contrast: src/ops/start_recommendation_report_generation.rs:183-214 refuses an undeclared OutputFormat/GroupName with exactly this ValidationException, with the comment "the boundary validator does not enforce enum membership".
Tests: tests/all/import_and_assess.rs only ever sends declared values (CONDITION_VALUES / ASSESSMENT_DATA_SOURCE_TYPES, lines 26-46, 448-500); tests/all/assessment_error_paths.rs covers unknown ids and the bucket prefix rule only. No test asserts a refusal for an undeclared assessmentDataSourceType or condition.

## Steps to Reproduce

1. POST /start-assessment {"s3bucketForAnalysisData":"migrationhub-strategy-bkt","assessmentDataSourceType":"Bogus"} -> expected HTTP 400 x-amzn-errortype ValidationException and no assessment created; emulator answers HTTP 200 with {"assessmentId":"assessment-..."}.
2. GET /get-latest-assessment-id -> the bogus-typed assessment is now the account's latest assessment.
3. POST /start-assessment {"s3bucketForAnalysisData":"migrationhub-strategy-bkt","assessmentTargets":[{"condition":"LIKE","name":"OS_NAME","values":["Linux"]}]} -> expected HTTP 400 ValidationException; emulator answers HTTP 200.
4. GET /get-assessment/{id} -> the undeclared condition "LIKE" is echoed back inside assessmentTargets.
5. Compare with POST /start-recommendation-report-generation {"outputFormat":"Bogus"}, which correctly answers HTTP 400.

Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 43c3b52fd22125f2604949f65e29a8c5612a98db

# assessmentTargets is stored and echoed but never scopes the assessment the service runs

_Filed: 2026-10-24T07:04:50Z_
_Operation: StartAssessment | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `43c3b52fd22125f2604949f65e29a8c5612a98db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartAssessmentRequest$assessmentTargets is "Defines the criteria of assessment" — a list of (condition, name, values) criteria that select WHICH of the account's servers the assessment collects data for. GetAssessment's dataCollectionDetails then reports the counters for that scoped set (servers / inProgress / success / failed).

Expected: an assessment started with assessmentTargets=[{"condition":"EQUALS","name":"OS_NAME","values":["Linux"]}] covers only the portfolio servers matching that criterion, so dataCollectionDetails.servers (and inProgress, and later success) counts those servers and not the whole portfolio; a target matching nothing yields a zero-server assessment.

Actual: the handler copies the targets onto the Assessment record purely so GetAssessment can echo them, and computes the scope with `store.imported_server_count()` — the sum of every completed import task's numberOfRecordsSuccess, which has no relation to the targets (and no relation to the server inventory either: it counts imported RECORDS of any resource type, not matching servers). Two assessments started with different, even mutually exclusive, targets therefore report identical counters. The targets are an input the service accepts, stores and returns, but they change no observable outcome.

## Evidence

Smithy model: StartAssessmentRequest$assessmentTargets -> AssessmentTargets (list<AssessmentTarget>, @length 0..10), documented "List of criteria for assessment"; AssessmentTarget doc "Defines the criteria of assessment" with required condition (Condition: EQUALS/NOT_EQUALS/CONTAINS/NOT_CONTAINS), name and values; DataCollectionDetails$servers doc "The total number of servers in the assessment" and $inProgress "The number of servers with the assessment status IN_PROGESS" — i.e. the counters describe the servers IN the assessment, which is what the criteria select.
Handler: src/ops/start_assessment.rs:212-238 — `store.settle_all(now); let servers = store.imported_server_count();` then `assessment_targets: input.assessment_targets.unwrap_or_default()` and `in_progress: servers`. The targets are never consulted. src/state.rs:1006-1013 imported_server_count() sums import tasks' number_of_records_success; src/state.rs:298-299 documents assessment_targets as "reported back verbatim". The store has the machinery to scope a selection (ServerRecord::matches_criteria, in_any_group, servers_in_groups) but StartAssessment uses none of it.
Tests: tests/all/import_and_assess.rs::an_assessments_server_count_comes_from_the_imports_in_state asserts the counter follows the IMPORT record counts, and an_assessment_is_reported_from_what_was_stored asserts the targets are echoed — no test asserts that different targets produce different scopes, so nothing pins the current behavior as intended.

## Steps to Reproduce

1. Populate the portfolio with servers of differing attributes (e.g. one Linux, one Windows).
2. POST /start-assessment {"s3bucketForAnalysisData":"migrationhub-strategy-bkt","assessmentTargets":[{"condition":"EQUALS","name":"OS_NAME","values":["Linux"]}]} -> note assessmentId A.
3. POST /start-assessment {"s3bucketForAnalysisData":"migrationhub-strategy-bkt","assessmentTargets":[{"condition":"EQUALS","name":"OS_NAME","values":["NoSuchOs"]}]} -> note assessmentId B.
4. GET /get-assessment/A and GET /get-assessment/B -> expected dataCollectionDetails.servers/inProgress to differ (B covering no server); emulator reports the identical counters for both, derived from the import record totals, while echoing each request's targets back unchanged.

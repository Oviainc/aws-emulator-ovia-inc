Issue-Severity: HIGH
Issue-Gap-Class: error-response
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# Return ConflictException (409) for a start issued while a report generation task is in progress instead of coalescing onto it with HTTP 200

_Filed: 2026-10-24T00:23:28Z_
_Operation: StartRecommendationReportGeneration | Class: error-response_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The real service allows only ONE recommendation report generation task at a time: a StartRecommendationReportGeneration issued while a previous generation is still running is REFUSED with the modeled ConflictException (HTTP 409, 'Exception to indicate that there is an ongoing task when a new task is created. Return when once the existing tasks are complete.'), so of N concurrent starts exactly one succeeds and the others fail with 409 carrying a distinguishable error code.

The emulator instead COALESCES: when a stored task is still within its completion span the handler returns HTTP 200 with the id of the already-running task (src/ops/start_recommendation_report_generation.rs, `if let Some(running) = store.in_progress_report_id(now) { return Ok(...id: Some(running)) }`). ConflictException is constructible in the operation's error channel (it is a variant of StartRecommendationReportGenerationErrorModeled) but is never produced anywhere in the crate.

Observable difference: a client that starts a second generation gets 200 + an id (and cannot tell it is the OLD task's id) where AWS answers 409 ConflictException; any workflow asserting 'the second concurrent start is rejected' passes on the emulator and fails on AWS. Note also that coalescing means two starts with DIFFERENT parameters (e.g. different outputFormat or groupIdFilter) silently produce one task generated with the first call's parameters.

## Evidence

Smithy model: com.amazonaws.migrationhubstrategy#StartRecommendationReportGeneration declares ConflictException (httpError 409) whose documentation is exactly this condition ('there is an ongoing task when a new task is created. Return when once the existing tasks are complete'); the only other operation carrying it is PutPortfolioPreferences. Handler: services/service-com-amazonaws-migrationhubstrategy-awsmigrationhubstrategyrecommendation/src/ops/start_recommendation_report_generation.rs lines ~219-227 return Ok with the running task's id; src/state.rs::in_progress_report_id documents the coalescing design ('a start call made while one is running coalescing onto it'). Tests: tests/all/recommendation_report.rs::a_start_while_one_is_in_progress_answers_the_running_tasks_id ASSERTS the coalescing (200 + same id), i.e. the current wrong behavior is locked in by a test and no test asserts a 409. Behavioral corpus has a check named migrationhubstrategy.concurrent-recommendation-report-generation-exactly-one-succeeds (listed in the open import-loop issue only as an IAM-prerequisite timeout; its conflict assertion was never reached and no issue covers this behavior).

## Steps to Reproduce

1. POST /start-recommendation-report-generation with {"outputFormat": "Json"} -> 200 with id R1.
2. Immediately (inside the task's completion span) POST /start-recommendation-report-generation again, e.g. with {"outputFormat": "Excel"}.
3. Expected: HTTP 409 with x-amzn-errortype ConflictException and no second task created. Actual: HTTP 200 with {"id": R1} — the first task's id, generated with the first call's outputFormat.
4. GET /get-recommendation-report-details/R1 -> only one task exists and it reports the first call's format, with no way for the second caller to learn its request was never acted on.

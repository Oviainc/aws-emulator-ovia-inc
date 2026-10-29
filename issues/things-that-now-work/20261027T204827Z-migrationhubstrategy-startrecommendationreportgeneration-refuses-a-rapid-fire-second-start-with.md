Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: migrationhubstrategy.recommendation-report-generation-while-in-progress-coalesces-same-id
Issue-Shape-Id: com.amazonaws.migrationhubstrategy#AWSMigrationHubStrategyRecommendation
Issue-Key: migrationhubstrategy|StartRecommendationReportGeneration|ConflictException
Issue-Commit: 338b94c8d9746e1b0f8f90bad2ee731e2046fe55

# StartRecommendationReportGeneration refuses a rapid-fire second start with ConflictException instead of coalescing onto the in-flight report id

> _Note: this issue was identified against the code base at commit `338b94c8d9746e1b0f8f90bad2ee731e2046fe55`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartRecommendationReportGeneration

## Description
Cause 1 of 1: a second StartRecommendationReportGeneration issued while a first report generation task is still IN_PROGRESS is refused with 409 ConflictException rather than returning the id of the task already in flight.

AWS: recorded behavior only for the coalescing rule — the Smithy model (com.amazonaws.migrationhubstrategy#StartRecommendationReportGeneration, POST /start-recommendation-report-generation, 200, output member `id` -> RecommendationTaskId @length 0..52 @pattern [0-9a-z-:]+) declares ConflictException (HTTP 409, documented as "Exception to indicate that there is an ongoing task when a new task is created. Return when once the existing tasks are complete.") but says nothing about what happens on a rapid-fire repeat. The recorded behavior this check replays shows the account-level report generation task being treated as a singleton that start calls COALESCE onto: a start issued immediately after a first one, before that first task has reached a terminal status, succeeds with HTTP 200 and returns the SAME RecommendationTaskId as the first call, and GetRecommendationReportDetails on that id reports a single task (IN_PROGRESS, settling to SUCCESS). The modelled ConflictException belongs to the cases the recording does NOT cover (e.g. a start conflicting with a different ongoing task class), not to a repeat of the same report generation request.

Emulator: the second start is refused with HTTP 409 ConflictException and message of the form "The recommendation report generation task <taskId> is IN_PROGRESS. Retry once the ongoing task is complete." — fixed text "The recommendation report generation task ", " is IN_PROGRESS. Retry once the ongoing task is complete.", with <taskId> taken from the emulator's state (in the replay it was the minted id of the first task). Nothing is minted for the second call. This is deliberate in the handler: services/service-com-amazonaws-migrationhubstrategy-awsmigrationhubstrategyrecommendation/src/ops/start_recommendation_report_generation.rs reads `portfolio.ongoing_report_generation(now)` after `settle_all(now)` and, when any task is still owed its terminal status, returns `crate::state::ongoing_task_conflict(&ongoing)` unconditionally — and the module's own header documents the rule as "a start call made while one is still in progress is refused with the modelled ConflictException ... and mints nothing".

Why: the normal client pattern — kick off report generation, and retry/re-issue the start when a previous response was lost, a retry fired, or two workers ask at once — breaks against the emulator. On AWS every one of those calls gets back the same in-flight report id and the caller polls it to SUCCESS; against the emulator every call after the first raises ConflictException, so an SDK caller with default retry semantics surfaces a hard error for a request AWS satisfies, and callers that treat the start as idempotent never obtain an id to poll. The failure is timing-dependent on AWS (any repeat inside the completion span) but deterministic here, so it turns a benign duplicate into an outage.

Fix: the operation's handler/state transition for StartRecommendationReportGeneration — the ongoing-task branch must, for a repeat of the same report generation request, return the existing in-flight task's id with HTTP 200 instead of constructing ConflictException, keeping the single-task invariant (no second record minted) while making the call idempotent over the in-flight task's span. Retain ConflictException only for cases the recording actually refuses.

Traces: check migrationhubstrategy.recommendation-report-generation-while-in-progress-coalesces-same-id. The check seeds a completed file import (service-linked role + S3 seed + StartImportFileTask polled to ImportSuccess), calls start twice back to back, and asserts second_id == first_id; it fails on the second call with botocore.errorfactory.ConflictException. Not covered by the open issues for this service (GetRecommendationReportDetails s3Bucket/s3Keys/statusMessage, StartImportFileTask validation, GetAssessment import settling), which concern other operations and other fields.

## Evidence
Replay output for migrationhubstrategy.recommendation-report-generation-while-in-progress-coalesces-same-id:

  botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the StartRecommendationReportGeneration operation: The recommendation report generation task recommendation-report-000000000003 is IN_PROGRESS. Retry once the ongoing task is complete.

Expected (recorded AWS): both calls return HTTP 200; second response's `id` equals the first response's `id`; GetRecommendationReportDetails(id=first_id) then reports status IN_PROGRESS settling to SUCCESS. The check's assertion is `second_id == first_id`.

Actual (emulator): first call 200 with an id; second call HTTP 409 ConflictException. Message breakdown — fixed text: "The recommendation report generation task " + " is IN_PROGRESS. Retry once the ongoing task is complete."; variable part: the task id, read from emulator state (recommendation-report-000000000003 in this run — a per-run generated id, not part of the requirement).

Smithy: StartRecommendationReportGeneration input {outputFormat: OutputFormat(Excel|Json), groupIdFilter: GroupIds}, output {id: RecommendationTaskId @length 0..52 @pattern [0-9a-z-:]+}, errors AccessDeniedException(403), ConflictException(409), InternalServerException(500), ThrottlingException(429), ValidationException(400).

Source: services/service-com-amazonaws-migrationhubstrategy-awsmigrationhubstrategyrecommendation/src/ops/start_recommendation_report_generation.rs lines ~235-246 (the `ongoing_report_generation` -> `ongoing_task_conflict` refusal) and the module header lines ~76-79 stating the single-start refusal as intended behavior.

## Steps to Reproduce
1. Seed the account so report generation is permitted: create the AWSServiceRoleForMigrationHubStrategy service-linked role, create a migrationhub-strategy-* S3 bucket, put a valid import JSON payload, call StartImportFileTask against it and poll GetImportFileTask until status == ImportSuccess.
2. POST /start-recommendation-report-generation with {"outputFormat": "Json"} -> 200, note `id` as first_id.
3. Immediately (within the task's completion span, while GetRecommendationReportDetails(first_id) still reports IN_PROGRESS) POST /start-recommendation-report-generation again with {"outputFormat": "Json"}.
4. Expected: 200 with `id` == first_id, and still exactly one report generation task in the account. Actual: HTTP 409 ConflictException "The recommendation report generation task <taskId> is IN_PROGRESS. Retry once the ongoing task is complete."

Issue-Source: trace-sweep-divergence
Issue-Trace: migrationhubstrategy-import-file-task-workflow
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: migrationhubstrategy-import-file-task-workflow migrationhubstrategy-portfolio-preferences-validation-error
Issue-Shape-Id: com.amazonaws.migrationhubstrategy#AWSMigrationHubStrategyRecommendation
Issue-Key: migrationhub-strategy|StartImportFileTask|4xx->4xx|$.message
Issue-Key: migrationhub-strategy|GetImportFileTask|4xx->4xx|$.message
Issue-Key: migrationhub-strategy|PutPortfolioPreferences|4xx->4xx|$.message
Issue-Commit: adba6aa61f0ba2f78a6b2d7c8f4b487f05884eaf

# Validation and not-found responses carry emulator-authored message text

> _Note: this issue was identified against the code base at commit `adba6aa61f0ba2f78a6b2d7c8f4b487f05884eaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartImportFileTask

## Description
On these three paths the emulator already gets the HTTP status and the x-amzn-errortype right; only the $.message body field diverges. MigrationHubStrategy answers with a fixed, service-wide canned string, while the emulator substitutes text it composes itself - either the generic AWS-SDK constraint report ('N validation error detected: Value ... failed to satisfy constraint ...') or a prose sentence naming the resource. The canned strings are observed as identical constants across independent fixtures and operations in this corpus, so the rule producing them is the service's, not an environment artifact: constraint failures in the request body yield 'Invalid request body', and the parameter/id-not-found family yields 'Looks like you provided invalid or missing parameters.'. Three causes, all in this service.

Cause 1 of 3 - StartImportFileTask with an over-long S3Bucket.
AWS: recorded behavior only - HTTP 400 with x-amzn-errortype ValidationException and message 'Invalid request body'. Smithy: StartImportFileTaskRequest.S3Bucket targets importS3Bucket @length(0,63) @pattern [0-9a-z]+[0-9a-z\.\-]*[0-9a-z]+; ValidationException is httpError 400, doc 'The request body isn't valid.' - the model states the constraint but prescribes no per-member message text, and the recording shows AWS does not emit any.
Emulator: same 400/ValidationException, but message '1 validation error detected: Value '<bucket>' at 's3Bucket' failed to satisfy constraint: Member must have length less than or equal to 63'.
Why: the emulator detects the right violation and reports it in a dialect this service never speaks, and the text it emits also uses the member spelling 's3Bucket' rather than the modeled 'S3Bucket'. A client or test that asserts on the documented message, or that parses it to recover which member failed, behaves differently against the emulator than against AWS.
Fix: the StartImportFileTask validation path's error construction - raise ValidationException with the service's canned 'Invalid request body' instead of rendering the generic Smithy constraint report.
Traces: migrationhubstrategy-import-file-task-workflow#task#1.

Cause 2 of 3 - GetImportFileTask for an import task id that was never created.
AWS: recorded behavior only - HTTP 404 with ResourceNotFoundException and message 'Looks like you provided invalid or missing parameters.' (the same constant the GetAssessment not-found path records in fixture migrationhubstrategy-assessment-error-paths). Smithy: GET /get-import-file-task/{id}, id @length(0,1024) @pattern .*\S.*; ResourceNotFoundException httpError 404.
Emulator: same 404/ResourceNotFoundException, but message 'Import file task <id> not found'.
Why: the emulator echoes the caller-supplied id into the message where AWS returns a fixed string, so message-level assertions diverge and any consumer treating the message as stable sees per-request text.
Fix: the GetImportFileTask handler's not-found error construction.
Traces: migrationhubstrategy-import-file-task-workflow#task#3.

Cause 3 of 3 - PutPortfolioPreferences with a business-goal rank out of range.
AWS: recorded behavior only - HTTP 400 with ValidationException and message 'Invalid request body'. Smithy: BusinessGoals.speedOfMigration targets BusinessGoalsInteger @range(min 1, max 5); ValidationException httpError 400.
Emulator: same 400/ValidationException, but message '1 validation error detected: Value '99' at 'prioritizeBusinessGoals.businessGoals.speedOfMigration' failed to satisfy constraint: Member must have value less than or equal to 5'.
Why: same dialect mismatch as cause 1, on the range constraint instead of the length one - confirming it is one shared error-rendering path rather than a per-operation slip.
Fix: the shared request-body validation error renderer for this service (and PutPortfolioPreferences' use of it) - return the canned 'Invalid request body' message.
Traces: migrationhubstrategy-portfolio-preferences-validation-error#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1; for all three clusters the status and x-amzn-errortype matched and only $.message diverged:

1) StartImportFileTask - fixture migrationhubstrategy-import-file-task-workflow step task#1, 400 -> 400: $.message expected 'Invalid request body', got "1 validation error detected: Value 'migrationhub-strategy-sim2real-probe-migrationhubstrategy-import-26cd426f' at 's3Bucket' failed to satisfy constraint: Member must have length less than or equal to 63" (the probed bucket is 70 chars; modeled max is 63).

2) GetImportFileTask - fixture migrationhubstrategy-import-file-task-workflow step task#3, 404 -> 404: $.message expected 'Looks like you provided invalid or missing parameters.', got 'Import file task never-created-import-task-id not found'.

3) PutPortfolioPreferences - fixture migrationhubstrategy-portfolio-preferences-validation-error step task#1, 400 -> 400: $.message expected 'Invalid request body', got "1 validation error detected: Value '99' at 'prioritizeBusinessGoals.businessGoals.speedOfMigration' failed to satisfy constraint: Member must have value less than or equal to 5".

Cross-fixture corroboration that these are service constants and not recording noise: 'Invalid request body' is the recorded message for two different operations and two different constraint kinds (length, range), and 'Looks like you provided invalid or missing parameters.' is the recorded message for both GetImportFileTask (404) and GetAssessment (400, fixture migrationhubstrategy-assessment-error-paths step task#1). Smithy evidence: importS3Bucket @length(0,63); BusinessGoalsInteger @range(1,5); ValidationException httpError 400 doc 'The request body isn't valid.'; ResourceNotFoundException httpError 404.

state_reset_failures = 0, bootstrap_signal = false. Severity is LOW because status and error code are already correct on all three paths - only the message body differs. Related, but a different fault: the status/error-code swaps on the assessment paths are filed separately as 'Assessment error paths return wrong status and error code' for this same service.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. POST /start-import-file-task with an S3Bucket longer than 63 characters (fixture migrationhubstrategy-import-file-task-workflow, step task#1) - 400/ValidationException is correct, but compare $.message against 'Invalid request body'.
4. GET /get-import-file-task/never-created-import-task-id (same fixture, step task#3) - 404/ResourceNotFoundException is correct, but compare $.message against 'Looks like you provided invalid or missing parameters.'.
5. POST /put-portfolio-preferences with prioritizeBusinessGoals.businessGoals.speedOfMigration = 99 (fixture migrationhubstrategy-portfolio-preferences-validation-error, step task#1) - 400/ValidationException is correct, but compare $.message against 'Invalid request body'.

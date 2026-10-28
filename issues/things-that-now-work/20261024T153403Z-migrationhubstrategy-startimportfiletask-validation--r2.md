Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# StartImportFileTask accepts an import name/key AWS rejects and a bucket that does not exist, minting a task either way

_Filed: 2026-10-24T15:34:03Z_
_Operation: StartImportFileTask | Class: validation_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartImportFileTask mints an import file task for requests the real service refuses with 400 ValidationException, so a caller gets an id (and a task that later reports ImportSuccess) for input AWS would never have accepted. Two symptoms of the same class:

(1) The request's member-level constraints are never enforced. Per the Smithy model, StartImportFileTaskRequest$name targets String with @length(1,50) AND @pattern [a-zA-Z0-9_-]+ (so no spaces, no punctuation other than '-'/'_', non-empty, at most 50 characters); s3key carries @pattern .*\S.* ; S3Bucket and s3bucketForReportData carry @pattern [0-9a-z]+[0-9a-z\.\-]*[0-9a-z]+ (lowercase only). The handler validates only the documented 'migrationhub-strategy-' bucket PREFIX, the dataSourceType enum and the groupId entries' GroupName, then stores input.name/input.s3key verbatim. Expected: POST /start-import-file-task with name="my import 1" (or "", "   ", a 60-character name, name="bad!name"), or s3key="   ", or S3Bucket="migrationhub-strategy-UPPER" -> HTTP 400 ValidationException and NO task created. Actual: HTTP 200 with {"id": "import-file-task-..."}, and GetImportFileTask/ListImportFileTask then report that importName/inputS3Key back.

(2) The S3 bucket is never checked for existence or reachability. state.rs::check_bucket_usable implements the prefix rule as its only test, yet words its failure '... does not exist or is not accessible [S3BucketNotFound]' — a reason the real service produces from an actual reachability check on the bucket it must read the import file from. Expected: StartImportFileTask(S3Bucket="migrationhub-strategy-does-not-exist", s3key=...) -> HTTP 400 ValidationException naming the not-found/not-accessible condition, no task created. Actual: HTTP 200, a task is minted and reaches ImportSuccess against a bucket nothing ever created. The same unchecked member exists on StartAssessment (s3bucketForAnalysisData / s3bucketForReportData), which shares check_bucket_usable.

## Evidence

Spec: smithy_query(query="operation", operation="StartImportFileTask") gives input members name {target String, length 1..50, pattern [a-zA-Z0-9_-]+, required}, s3key {pattern .*\S.*, required}, S3Bucket/s3bucketForReportData {pattern [0-9a-z]+[0-9a-z\.\-]*[0-9a-z]+}; the operation models ValidationException (httpError 400, 'The request body isn't valid.').
Handler: src/ops/start_import_file_task.rs:169-237 — only check_bucket_usable (prefix), check_enum(dataSourceType) and check_group_names run; `import_name: input.name` and `input_s3_key: input.s3key` are stored unexamined. src/state.rs check_bucket_usable tests only the 'migrationhub-strategy-' prefix while reporting '[S3BucketNotFound]'.
Test coverage: NONE pins these. tests/all/refusal_messages.rs:103-118 states outright that only @length/@range/@required on a member's TARGET shape are enforced and that "`@pattern` is not enforced at all; both are gaps in the generated schema", and deliberately includes no pattern row; every test name used in the suite is already pattern-legal ('import-1', 'IMPORT-4_mixed-case_and-underscores'), and no test asserts an illegal name/key/bucket is accepted.
Well-established service behavior: the published behavioral corpus carries a dedicated AWS check for this (migrationhubstrategy.import-file-task-invalid-name-pattern-rejected), and the open issue 20261023T004429Z documents [S3BucketNotFound] as the reason AWS returns for a bucket its reachability check cannot see — which presupposes the check the emulator never performs. That issue covers the WORDING of a prefix violation on an existing bucket; it does not cover a non-existent bucket being accepted, nor the name/s3key constraints.

## Steps to Reproduce

1. POST /start-import-file-task with body {"name": "my import 1", "S3Bucket": "migrationhub-strategy-import", "s3key": "servers.csv"} — the name contains spaces, which StartImportFileTaskRequest$name's @pattern [a-zA-Z0-9_-]+ forbids. Expected HTTP 400 ValidationException with no task created; emulator answers HTTP 200 with an id.
2. Repeat with "name": "" (empty, below @length min 1), "name": "   ", and a 60-character name (past @length max 50) — each is accepted with HTTP 200.
3. Repeat with "s3key": "   " (violates @pattern .*\S.*) and "S3Bucket": "migrationhub-strategy-UPPER" (uppercase violates importS3Bucket's @pattern) — both accepted with HTTP 200.
4. GET /list-import-file-task — the tasks minted above are listed, reporting importName/inputS3Key values AWS would have refused.
5. POST /start-import-file-task with {"name": "import-1", "S3Bucket": "migrationhub-strategy-does-not-exist", "s3key": "servers.csv"} without ever creating that bucket in S3. Expected HTTP 400 ValidationException (bucket not found / not accessible) and no task; emulator answers HTTP 200 and the task settles to ImportSuccess.

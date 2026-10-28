Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.batch-create-partition-arity-mismatch-partial glue.concurrent-identity-center-config-modification-rejected glue.get-dashboard-url-returns-valid-resource-url glue.job-bookmark-absent-until-run
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.batch-create-partition-arity-mismatch-partial|run#10|AssertionError|assert message == expected_message, (
Issue-Key: glue.concurrent-identity-center-config-modification-rejected|run#16|AssertionError|assert f["message"] == expected[f["error"]], (
Issue-Key: glue.get-dashboard-url-returns-valid-resource-url|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: glue.job-bookmark-absent-until-run|run#13|AssertionError|assert reset_message == expected_message, (
Issue-Commit: 84725303c518fb402f10cf931cddd35e44c00d9a

# Glue: 4 net-new root causes — four refusal messages are emulator-authored instead of AWS's (BatchCreatePartition arity, CreateGlueIdentityCenterConfiguration duplicate, GetDashboardUrl unknown session, Get/ResetJobBookmark absent bookmark)

> _Note: this issue was identified against the code base at commit `84725303c518fb402f10cf931cddd35e44c00d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchCreatePartition

## Description
Four independent Glue operations return the correct HTTP status and the correct error CODE but an error MESSAGE the emulator authored itself, instead of the message real Glue returns for that error class. An error message is part of the contract: clients, SDK retry/branching helpers and test suites match on it. None of the open Glue issues cover these four operations (the closest, 20261025T031335Z-glue-job-runs-never-execute-and-publish-no.md, covers two job-RUN refusal messages, not these); they are net-new.

Cause 1 of 4: BatchCreatePartition arity refusal message is rewritten with request-derived text.
AWS: Smithy: BatchCreatePartition errors include InvalidInputException (client, HTTP 400); input PartitionInputList is required (@length 0..100). When one PartitionInput.Values has a different number of values than the table's PartitionKeys, real Glue refuses the WHOLE request with InvalidInputException and the message is the fixed, invariant text `The number of partition keys do not match the number of partition values` — it contains NO variable parts: no table name, no counts, no values.
Emulator: same code (InvalidInputException) but the message is `Partition values must name one value per partition key of the table; the table has 2 partition keys and the input carried 1 values` — emulator-authored prose that interpolates the table's partition-key count and the offending entry's value count.
Why: a client that branches on, logs or asserts the documented arity message never matches, so an arity mismatch is indistinguishable from any other InvalidInputException; the interpolated counts also make the message vary per request where AWS's does not.
Fix: the BatchCreatePartition handler's partition-arity validation (services/service-com-amazonaws-glue-awsglue/src/ops/batch_create_partition.rs) must emit the fixed AWS text verbatim for both directions of the mismatch (too few and too many values).
Traces: glue.batch-create-partition-arity-mismatch-partial (assertion at check line 100).

Cause 2 of 4: CreateGlueIdentityCenterConfiguration's duplicate rejection message omits the ApplicationArn.
AWS: Smithy: CreateGlueIdentityCenterConfiguration errors include AlreadyExistsException and ConcurrentModificationException (both client, HTTP 400); its output carries ApplicationArn. Recorded behavior: when the account singleton already exists, the loser of a race is refused with AlreadyExistsException whose message is `Glue Idc Application already exists. ApplicationArn: <applicationArn>` — fixed text `Glue Idc Application already exists. ApplicationArn: ` plus one variable part, the ApplicationArn of the configuration that already exists (taken from emulator state, i.e. the winning create's ApplicationArn). A racer that overlaps the creation lock instead gets ConcurrentModificationException with the fixed text `Concurrent modification when creating glue Idc application`.
Emulator: serialization itself is right (exactly one racer of six won), but the losers' AlreadyExistsException message is `Glue Identity Center configuration already exists.` — different fixed text and no ApplicationArn at all.
Why: a caller cannot learn WHICH application already owns the account singleton from the refusal, and message-matching clients do not recognize the duplicate case.
Fix: the CreateGlueIdentityCenterConfiguration handler's duplicate/concurrency rejection path (services/service-com-amazonaws-glue-awsglue/src/ops/create_glue_identity_center_configuration.rs) must render AWS's text and interpolate the stored ApplicationArn.
Traces: glue.concurrent-identity-center-config-modification-rejected (assertion at check line 64).

Cause 3 of 4: GetDashboardUrl's unknown-session refusal message leaks a resource-specific sentence.
AWS: Smithy: GetDashboardUrl errors include EntityNotFoundException and OperationNotSupportedException (client, HTTP 400); input ResourceId + ResourceType (GlueResourceType enum). For a ResourceType=SESSION whose ResourceId names no session, real Glue raises EntityNotFoundException with the fixed, invariant message `Entity not found` — no id, no resource type, no trailing period. (The ResourceType=JOB refusal, OperationNotSupportedException `GetDashboardUrl operation is not supported for JOB`, is already correct in the emulator — that assertion passed.)
Emulator: code EntityNotFoundException is right, message is `Session <session-id> not found.` where <session-id> is echoed from the request.
Why: the caller sees a message shape AWS never emits for this operation, and anything keyed on the exact `Entity not found` text (the only thing distinguishing this refusal) fails.
Fix: the GetDashboardUrl handler's session-lookup miss (services/service-com-amazonaws-glue-awsglue/src/ops/get_dashboard_url.rs) must raise EntityNotFoundException with the bare fixed text rather than formatting the requested id.
Traces: glue.get-dashboard-url-returns-valid-resource-url (assertion at check line 42).

Cause 4 of 4: GetJobBookmark/ResetJobBookmark on a job with no bookmark use a 'Job bookmark' message instead of AWS's 'Continuation' message.
AWS: Smithy: both GetJobBookmark and ResetJobBookmark are modeled with EntityNotFoundException (client, HTTP 400) and take a required JobName. Recorded behavior: for a job that has never run — and equally for a JobName that does not exist at all — real Glue raises EntityNotFoundException with `Continuation for job <job-name> not found`: fixed text `Continuation for job ` + the JobName taken verbatim from the request + ` not found`, with NO trailing period.
Emulator: code EntityNotFoundException is right, message is `Job bookmark for job <job-name> not found.` — different fixed wording and an extra trailing period.
Why: the same wrong message is produced by two operations and by three distinct inputs (never-run job, sibling never-run job, nonexistent job), so no client can recognize the documented 'continuation not found' condition.
Fix: the shared job-bookmark lookup used by the GetJobBookmark and ResetJobBookmark handlers (services/service-com-amazonaws-glue-awsglue/src/ops/get_job_bookmark.rs and .../reset_job_bookmark.rs) must render AWS's continuation text with the request's JobName.
Traces: glue.job-bookmark-absent-until-run (assertion at check line 214; the same check asserts the identical message for GetJobBookmark on a never-run job, on an unknown job name and on a sibling job).

## Evidence
Cause 1 — glue.batch-create-partition-arity-mismatch-partial:
  expected: InvalidInputException / 'The number of partition keys do not match the number of partition values'
  actual:   InvalidInputException / 'Partition values must name one value per partition key of the table; the table has 2 partition keys and the input carried 1 values'
  (AssertionError: too few values: expected the arity refusal message ... got ...)

Cause 2 — glue.concurrent-identity-center-config-modification-rejected:
  expected: AlreadyExistsException / 'Glue Idc Application already exists. ApplicationArn: <ApplicationArn of the existing configuration>'
  actual:   AlreadyExistsException / 'Glue Identity Center configuration already exists.'
  (AssertionError: unexpected CreateGlueIdentityCenterConfiguration AlreadyExistsException message: 'Glue Identity Center configuration already exists.')

Cause 3 — glue.get-dashboard-url-returns-valid-resource-url:
  expected: EntityNotFoundException / 'Entity not found'
  actual:   EntityNotFoundException / 'Session <requested session id> not found.'
  (AssertionError: expected message 'Entity not found', got 'Session sim2real-glue-nonexistent-session-616cbd830482 not found.')

Cause 4 — glue.job-bookmark-absent-until-run:
  expected: EntityNotFoundException / 'Continuation for job <JobName> not found'
  actual:   EntityNotFoundException / 'Job bookmark for job <JobName> not found.'
  (AssertionError: ResetJobBookmark on a never-run job: expected 'Continuation for job sim2real-behav-job-... not found', got 'Job bookmark for job sim2real-behav-job-... not found.')

In every one of the four the HTTP status and error code already match AWS; only the message diverges, and in causes 1 and 3 the emulator additionally interpolates request/state values into a message AWS keeps invariant.

## Steps to Reproduce
Cause 1: CreateDatabase, then CreateTable with PartitionKeys [{yr,string},{mo,string}]; call BatchCreatePartition with PartitionInputList = [Values ['2024','01'] (valid), Values ['2024'] (too few)] and again with Values ['2024','01','extra'] (too many). Read Error.Code and Error.Message: code is InvalidInputException, message must be exactly 'The number of partition keys do not match the number of partition values'.

Cause 2: ensure the account has no Glue Identity Center configuration (GetGlueIdentityCenterConfiguration must raise EntityNotFoundException 'Idc application not found. Need to create glue application first.'), then fire 6 concurrent CreateGlueIdentityCenterConfiguration(InstanceArn=<sso-admin instance arn>) calls from a thread barrier. One succeeds; inspect each loser's Error.Message — it must be 'Glue Idc Application already exists. ApplicationArn: ' + the winner's ApplicationArn (or ConcurrentModificationException 'Concurrent modification when creating glue Idc application').

Cause 3: call GetDashboardUrl(ResourceId=<an id that names no session>, ResourceType='SESSION'). Code must be EntityNotFoundException and Message exactly 'Entity not found'.

Cause 4: CreateJob (glueetl) and, without ever starting a run, call ResetJobBookmark(JobName=<job>) and GetJobBookmark(JobName=<job>), plus GetJobBookmark(JobName='does-not-exist-job'). Each must be EntityNotFoundException with Message 'Continuation for job <that JobName> not found' (no trailing period).

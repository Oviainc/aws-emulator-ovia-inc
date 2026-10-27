Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.job-update-and-batch-get
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.job-update-and-batch-get|run#21|AssertionError|assert message == expected_message, f"expected GetJob message {expected_message!r}, got {message!r}"
Issue-Commit: 9ce68f8efb447c5711e42ec7e2cb128c2507b7ba

# Glue GetJob/UpdateJob EntityNotFoundException message is request-derived ("Job <name> not found") instead of AWS's constant "Job not found."

> _Note: this issue was identified against the code base at commit `9ce68f8efb447c5711e42ec7e2cb128c2507b7ba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJob

## Description
This issue carries 1 net-new root cause.

=== Cause 1 of 1: the job not-found refusal message is rebuilt from the request instead of AWS's fixed, request-independent text ===

AWS: Smithy com.amazonaws.glue#GetJob takes a single required JobName (NameString, @length 1..255, pattern ^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\t]*$) and models EntityNotFoundException as a client error with httpError 400 (documentation: "A specified entity does not exist"); com.amazonaws.glue#UpdateJob (required JobName + JobUpdate) models the same EntityNotFoundException/400. The model documents the exception's Message only as free prose, so the exact wording is recorded behavior only. Recorded behavior: GetJob on a job name that no job in the account/Region holds (here, a name whose job was just deleted via DeleteJob) is refused with HTTP 400, Error.Code "EntityNotFoundException" and the CONSTANT message

    Job not found.

That whole string is fixed text, trailing period included; it has NO variable parts, and AWS deliberately does not echo the requested JobName back. UpdateJob against the same deleted name is refused the same way: EntityNotFoundException with the identical constant message.

Emulator: the status and the code are right (HTTP 400 / EntityNotFoundException), but the message is synthesised from the request by the shared helper crate::state::messages::job_not_found(&input.job_name) as the template "Job <JobName> not found" — different fixed text ("Job " ... " not found", no trailing period) with the request's JobName interpolated in the middle. In the failing replay the emulator answered 'Job sim2real-behav-glue-job-0af56235612c not found' where AWS answers the constant above; that job name is a per-run value from the request and is not part of any requirement. The check stops on the GetJob assertion, so the UpdateJob leg was never reached in this replay — but it is the same defect by construction: update_job.rs builds its EntityNotFoundException from the same helper, so a fix must cover both call sites.

Why: for this error class the message is a fixed constant and is what clients match on — SDK wrappers, error-classification and retry layers, CFN/Terraform-style providers, log scrapers and conformance suites compare Error.Message against the documented constant. Against the emulator such a matcher never recognises the refusal, so a genuinely absent (or just-deleted) job definition is surfaced as an unknown failure rather than a not-found, and because the code and status are correct the divergence stays silent until something inspects the prose. The request-derived form also echoes an identifier back to the caller that AWS withholds.

Fix: the not-found refusal path of the Glue GetJob handler and the shared message helper it calls, plus the identical call site in UpdateJob — raise EntityNotFoundException with AWS's constant message and no interpolation of JobName. Any other operation reusing the same job-lookup helper must emit the same constant.

Related open issue: 20261025T213800Z-glue-getjobrun-s-entitynotfoundexception-message-echoes-the-request.md records the SAME class of defect for a DIFFERENT operation (GetJobRun, whose AWS constant is "Job run not found for the given job name and runId.") and does not carry this check's replay key; 20261025T031335Z-glue-job-runs-never-execute-and-publish-no.md likewise covers job-RUN refusal messages only. Neither covers the job-DEFINITION lookup in GetJob/UpdateJob.

Traces: glue.job-update-and-batch-get. The check's earlier legs all passed (CreateJob, UpdateJob persisting MaxRetries=2/Timeout=45, BatchGetJobs partitioning Jobs vs JobsNotFound, ListJobs, DeleteJob); it stops on the GetJob-after-delete message assertion, so the trailing UpdateJob-after-delete message assertion was reached by neither this run nor this issue's evidence.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against this checkout: 56/223 checks passed. This failure is net-new (not marked ALREADY FILED and not marked OWNED in the report).

glue.job-update-and-batch-get [mismatch]:
  File "check_glue_job_update_and_batch_get.py", line 61, in run
  assert message == expected_message, f"expected GetJob message {expected_message!r}, got {message!r}"
  AssertionError: expected GetJob message 'Job not found.', got 'Job sim2real-behav-glue-job-0af56235612c not found'

  Expected (AWS): GetJob(JobName=<a name no job holds>) -> HTTP 400, Error.Code 'EntityNotFoundException', Error.Message exactly 'Job not found.' — entirely fixed text, no request-derived parts.
  Observed (emulator): HTTP 400 and Error.Code 'EntityNotFoundException' (both correct), Error.Message built from the template 'Job <JobName> not found' — fixed parts 'Job ' and ' not found', with the request's JobName interpolated and no trailing period. The concrete name in the trace above is a per-run value generated by the check.

  Source of the divergence in this checkout: services/service-com-amazonaws-glue-awsglue/src/ops/get_job.rs:157-165 returns EntityNotFoundException { message: Some(crate::state::messages::job_not_found(&input.job_name)) }; services/service-com-amazonaws-glue-awsglue/src/ops/update_job.rs:227-235 returns the same helper's output for its own not-found path.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-behavioral-ga-prod-v1, re-running glue.job-update-and-batch-get.
2. Create an IAM role trusting glue.amazonaws.com with arn:aws:iam::aws:policy/service-role/AWSGlueServiceRole attached.
3. glue.create_job(Name='<job>', Role='<that role arn>', Command={'Name':'glueetl','ScriptLocation':'s3://example-bucket/scripts/job.py'}, MaxRetries=0, Timeout=20).
4. glue.delete_job(JobName='<job>').
5. glue.get_job(JobName='<job>').
6. Expect: ClientError, HTTP 400, Error.Code 'EntityNotFoundException', Error.Message exactly 'Job not found.'.
7. Observed: HTTP 400 and Error.Code 'EntityNotFoundException' are correct, but Error.Message is 'Job <job> not found' with the requested job name interpolated.
8. Same for the deleted name through the update path: glue.update_job(JobName='<job>', JobUpdate={'Role':'<that role arn>','Command':{'Name':'glueetl','ScriptLocation':'s3://example-bucket/scripts/job.py'}}) must also answer EntityNotFoundException with the constant 'Job not found.'.

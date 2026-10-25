Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.job-batch-stop-run
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetJobRun|EntityNotFoundException
Issue-Commit: 1bfa76d6b42491e64360a5046ec0d56674da7849

# Glue GetJobRun's EntityNotFoundException message echoes the request instead of AWS's constant "Job run not found for the given job name and runId."

> _Note: this issue was identified against the code base at commit `1bfa76d6b42491e64360a5046ec0d56674da7849`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJobRun

## Description
This issue carries 1 net-new root cause.

=== Cause 1 of 1: GetJobRun's not-found refusal message is rebuilt from the request instead of AWS's fixed, request-independent text ===

AWS: Smithy com.amazonaws.glue#GetJobRun takes a required JobName (NameString, @length 1..255) and a required RunId (IdString, @length 1..255) and models EntityNotFoundException as a client error with httpError 400 ("A specified entity does not exist"); its Message member is documented only as "A message describing the problem", so the exact text is recorded behavior only. Recorded behavior: a GetJobRun naming an existing job but a RunId that was never created is refused with HTTP 400, Error.Code "EntityNotFoundException" and the CONSTANT message

    Job run not found for the given job name and runId.

That whole string is fixed text: it has NO variable parts. AWS deliberately echoes back neither the JobName nor the RunId from the request.

Emulator: the status and the code are right (EntityNotFoundException / 400), but the message is synthesised from the request as "Job run <RunId> not found for job <JobName>" — a different template that interpolates the request's RunId and JobName. In the failing replay the emulator answered 'Job run nonexistent-run-id not found for job sim2real-behav-job-7c20fc2cf81d' where AWS answers the constant above; 'nonexistent-run-id' and the job name there are per-run values from the request, not part of any requirement.

Why: the message for this error class is a fixed constant and is the thing clients match on — SDK wrappers, retry/classification layers, CFN/Terraform-style providers, log scrapers and conformance suites compare Error.Message to the documented constant. Against the emulator such a matcher never recognises the refusal, so a genuinely absent job run is surfaced as an unknown failure rather than a not-found; and because the code and status are correct the divergence is silent until something inspects the prose. The request-derived form additionally leaks identifiers back to the caller that AWS withholds.

Fix: the not-found refusal path of the Glue GetJobRun handler (services/service-com-amazonaws-glue-awsglue/src/ops/get_job_run.rs) — raise EntityNotFoundException with AWS's constant message, with no interpolation of JobName/RunId. Any shared job-run lookup helper that other operations reuse should raise the same constant.

Related open issue: 20261025T031335Z-glue-job-runs-never-execute-and-publish-no.md records this same GetJobRun message defect as its Cause 4, but it carries only the replay key of glue.reject-get-nonexistent-job-run; it does NOT carry the replay key of the check grounding this issue (glue.job-batch-stop-run), which is why the suite reports that check as net-new. If that issue's Cause 4 is fixed, this issue is closed by re-replaying the check id below — the fix must make the BatchStopJobRun check's independent GetJobRun read-back see the constant message too.

Traces: glue.job-batch-stop-run (the check exercises BatchStopJobRun with one real run id plus a bogus one, then corroborates the bogus id through an independent GetJobRun read; it stops on this message assertion, so the BatchStopJobRun SuccessfulSubmissions/Errors split and the stopped-run state corroboration it exists to grade were reached but the run-state corroboration after the message check was never evaluated and is NOT claimed by this issue).

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against this checkout: 31/230 checks passed. The failure below is net-new (not marked ALREADY FILED and not marked OWNED in the report).

glue.job-batch-stop-run [mismatch]:
  File "check_glue_job_batch_stop_run.py", line 140, in run
  assert message == expected_message, (
  AssertionError: expected message 'Job run not found for the given job name and runId.' reading back the nonexistent run id, got 'Job run nonexistent-run-id not found for job sim2real-behav-job-7c20fc2cf81d'

  Expected (AWS): GetJobRun(JobName=<an existing job>, RunId=<never created>) -> HTTP 400, Error.Code 'EntityNotFoundException', Error.Message exactly 'Job run not found for the given job name and runId.' (entirely fixed text; no request-derived parts).
  Observed (emulator): HTTP 400, Error.Code 'EntityNotFoundException' (both correct), Error.Message 'Job run <RunId> not found for job <JobName>' — the request's RunId and JobName interpolated into emulator-authored prose. The concrete id and job name in the trace above are per-run values.
  The check's earlier assertions passed: BatchStopJobRun did report 'nonexistent-run-id' in Errors and the real run id in one of the two buckets, so this is purely the GetJobRun refusal message.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-behavioral-ga-prod-v1, re-running glue.job-batch-stop-run.
2. Create an IAM role trusting glue.amazonaws.com with AWSGlueServiceRole attached.
3. glue.create_job(Name='<job>', Role='<that role arn>', Command={'Name':'pythonshell','PythonVersion':'3.9','ScriptLocation':'s3://<bucket>/scripts/etl.py'}, MaxCapacity=0.0625, Timeout=10).
4. glue.start_job_run(JobName='<job>') to obtain a real JobRunId.
5. glue.batch_stop_job_run(JobName='<job>', JobRunIds=[<realRunId>, 'nonexistent-run-id']) — 'nonexistent-run-id' is reported under Errors (correct today).
6. glue.get_job_run(JobName='<job>', RunId='nonexistent-run-id').
7. Expect: ClientError, HTTP 400, Error.Code 'EntityNotFoundException', Error.Message exactly 'Job run not found for the given job name and runId.'.
8. Observed: Error.Code is correct but Error.Message is 'Job run nonexistent-run-id not found for job <job>'.

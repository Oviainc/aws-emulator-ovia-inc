Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.gjwr-batch-get-jobs-reports-the-other-regions-job-as-not-found glue__iam__s3.gjwr-one-role-backs-jobs-in-both-regions
Issue-Key: glue.gjwr-batch-get-jobs-reports-the-other-regions-job-as-not-found|run#6|AssertionError|assert found == [want], f"{region}: BatchGetJobs found {found!r}, expected [{want!r}]"
Issue-Key: glue__iam__s3.gjwr-one-role-backs-jobs-in-both-regions|run#8|AssertionError|assert mine == [job_name], f"{region}: BatchGetJobs over both names found {mine!r}, expected [{job_name!r}]"
Issue-Commit: 2b5b554775f720de5c9510c8dde5c1ff761ca735

# Glue BatchGetJobs is not region-scoped: a job created through another region's endpoint is reported as found

> _Note: this issue was identified against the code base at commit `2b5b554775f720de5c9510c8dde5c1ff761ca735`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetJobs

## Description
This issue carries 1 net-new root cause. It must be closed before the issue is done.

Cause 1 of 1: Glue job state is shared across regions, so BatchGetJobs answers for jobs that belong to another region instead of partitioning the requested names into Jobs / JobsNotFound.

AWS: Smithy BatchGetJobs takes a required JobNames (JobNameList) and returns Jobs (JobList) plus JobsNotFound (JobNameList); it models only InvalidInputException / OperationTimeoutException / InternalServiceException, i.e. a name that does not exist is NOT an error, it is returned in JobsNotFound. Glue jobs are a regional resource: a job created through region A's endpoint does not exist in region B. Recorded behavior: with one job created in each of two regions and BatchGetJobs asked for BOTH names, each regional endpoint returns exactly the job it owns in Jobs and exactly the other name in JobsNotFound — the same split, mirrored. (The related, already-filed cause 20261025T022820Z-glue-7-net-new-root-causes-gettables-expression.md, Cause 4, records the same absence of region isolation for StartJobRun/GetJobRuns and carries neither of the two replay keys below.)

Emulator: BatchGetJobs through the us-west-2 endpoint returns BOTH jobs in Jobs — the one created through us-west-2 and the one created through us-east-1 — and JobsNotFound is empty. Both checks assert the same split and both see the merged store: "us-west-2: BatchGetJobs found ['sim2real-gjwr-bg-<id>-foreign', 'sim2real-gjwr-bg-<id>-local'], expected ['sim2real-gjwr-bg-<id>-local']".

Why: a client relying on AWS's regional isolation is told a job exists in a region that does not own it, so a cross-region misconfiguration is never surfaced: it reads back a job definition (and can go on to start billable runs) through the wrong endpoint, and two same-named jobs in two regions collide into one record. The partial-response contract is also unobservable — JobsNotFound never carries anything, so a caller cannot detect a genuinely missing name.

Fix: key the Glue job store by region in the Glue service state, and make the name-resolution path of the BatchGetJobs handler (services/service-com-amazonaws-glue-awsglue/src/ops/batch_get_jobs.rs) resolve only jobs owned by the request's region, placing every other requested name into JobsNotFound. The same region key must be honoured by GetJob/ListJobs/DeleteJob so a job is visible from exactly one region.

Traces: glue.gjwr-batch-get-jobs-reports-the-other-regions-job-as-not-found, glue__iam__s3.gjwr-one-role-backs-jobs-in-both-regions (the second check's IAM half passes — one role ARN is the same entity from both regions — and it fails on the same Glue BatchGetJobs split).

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against this checkout (23/223 checks passed).

glue.gjwr-batch-get-jobs-reports-the-other-regions-job-as-not-found [mismatch]:
  File "check_glue_gjwr_batch_get_jobs_reports_the_other_regions_job_as_not_found.py", line 36, in run
  assert found == [want], f"{region}: BatchGetJobs found {found!r}, expected [{want!r}]"
  AssertionError: us-west-2: BatchGetJobs found ['sim2real-gjwr-bg-812808dee442-foreign', 'sim2real-gjwr-bg-812808dee442-local'], expected ['sim2real-gjwr-bg-812808dee442-local']

glue__iam__s3.gjwr-one-role-backs-jobs-in-both-regions [mismatch]:
  File "check_glue__iam__s3_gjwr_one_role_backs_jobs_in_both_regions.py", line 40, in run
  assert mine == [job_name], f"{region}: BatchGetJobs over both names found {mine!r}, expected [{job_name!r}]"
  AssertionError: us-west-2: BatchGetJobs over both names found ['sim2real-gjwr-both-a8d9fa96952c-us-east-1', 'sim2real-gjwr-both-a8d9fa96952c-us-west-2'], expected ['sim2real-gjwr-both-a8d9fa96952c-us-west-2']

Expected per AWS: Jobs = [the job created through THAT region's endpoint]; JobsNotFound = [the other name]; mirrored for the second region. Observed: Jobs = both names from both endpoints, JobsNotFound empty. Generated job names and the region pair are per-run values, not requirements; the requirement is the found/not-found split per endpoint.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-behavioral-ga-prod-v1, re-running the check ids in trace_ids.
2. With a client pinned to region A, glue:CreateJob(Name='<tok>-a', Role=<any role arn>, Command={'Name':'glueetl','PythonVersion':'3','ScriptLocation':'s3://.../job.py'}, GlueVersion='4.0').
3. With a client pinned to region B, glue:CreateJob(Name='<tok>-b', ...same shape...).
4. From region A call glue:BatchGetJobs(JobNames=['<tok>-a','<tok>-b']) — expect Jobs to contain only '<tok>-a' and JobsNotFound == ['<tok>-b'].
5. From region B call the same BatchGetJobs — expect Jobs to contain only '<tok>-b' and JobsNotFound == ['<tok>-a'].
6. Observed in both steps: Jobs contains both names and JobsNotFound is empty.

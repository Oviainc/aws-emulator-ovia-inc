Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.batch-delete-connection-per-entry-errors-and-valid-deletes-land glue.gjwr-region-sweep-locates-the-job-only-where-it-was-created
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.batch-delete-connection-per-entry-errors-and-valid-deletes-land|run#8|AssertionError|assert pre_message == f"Connection with name {absent} in account {ctx.account} is not found", (
Issue-Key: glue.gjwr-region-sweep-locates-the-job-only-where-it-was-created|run#9|AssertionError|assert family_hits[home] == [decoy], (
Issue-Commit: 2e55e3f174321d59189949bdc160ce3c5e1a8b8d

# Glue: GetConnection EntityNotFoundException message wording is wrong, and ListJobs is not region-scoped (2 net-new root causes)

> _Note: this issue was identified against the code base at commit `2e55e3f174321d59189949bdc160ce3c5e1a8b8d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListJobs, GetConnection

## Description
This issue carries 2 net-new root causes. BOTH must be closed before the issue is done.

=== Cause 1 of 2: GetConnection's EntityNotFoundException message does not use AWS's wording for an absent connection ===

AWS: Smithy GetConnection takes a required `Name` (NameString, @length 1..255) plus an optional `CatalogId`, and models `EntityNotFoundException` as a client error with httpError 400 (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json). The model carries no message template, so this is recorded behavior only: real Glue answers a GetConnection for a name that does not exist with code `EntityNotFoundException` and the message

    Connection with name <Name> in account <accountId> is not found

Fixed text of that message: "Connection with name ", " in account ", " is not found". Variable parts: `<Name>` is the connection name taken verbatim from the request, `<accountId>` is the caller's 12-digit AWS account id. Note that AWS phrases the scope as the ACCOUNT, not a catalog.

Emulator: the same call returns code `EntityNotFoundException` (correct) but the message `Connection <Name> not found in catalog <accountId>` — a different template: the word order is inverted, "with name" is dropped, and the scope noun is "catalog" instead of "account".

Why: clients and tooling that match on Glue's error message (log scrapers, retry/classification wrappers, Terraform/CDK-style providers and test suites that assert the documented message) do not recognise the emulator's text, so a genuinely-absent connection is reported as an unknown failure rather than a not-found. Because the code is right and only the prose differs, the divergence is silent until such a matcher runs.

Fix: the message-construction path of the GetConnection handler (services/service-com-amazonaws-glue-awsglue/src/ops/get_connection.rs) and whatever shared connection-lookup helper it uses to raise EntityNotFoundException: emit AWS's template with the requested name and the request's account id. Any other Glue operation that resolves a connection by name should raise the same text.

Traces: glue.batch-delete-connection-per-entry-errors-and-valid-deletes-land (the check's positive control asserts the exact GetConnection message before exercising BatchDeleteConnection, so it stops on this cause; the BatchDeleteConnection per-entry Succeeded/Errors split it exists to grade was never reached and is NOT claimed by this issue).

=== Cause 2 of 2: ListJobs is not region-scoped — one regional endpoint lists jobs created through another region's endpoint ===

AWS: Smithy ListJobs takes only NextToken / MaxResults / Tags and returns `JobNames` (JobNameList) with NextToken; it is paginated over JobNames. Glue jobs are a REGIONAL resource, so the set of names a regional endpoint can enumerate is exactly the jobs created through that endpoint. Recorded behavior: with one job created through region A's endpoint and a differently-named job created through region B's endpoint, a paginated ListJobs sweep of each endpoint returns only that endpoint's own job; neither endpoint ever names the other region's job.

Emulator: ListJobs through one region's endpoint returns the union of both regions' jobs. The check's sweep saw, from the us-west-2 endpoint, both the job created through us-east-1 and the job created through us-west-2: "us-west-2 lists family jobs ['<family>-RoleReport-us-east-1', '<family>-etl-usage-events-us-west-2'], expected only the decoy ['<family>-etl-usage-events-us-west-2']". (The concrete names and the region pair are per-run values; the requirement is that each endpoint enumerate only its own region's jobs.)

Why: a client that uses a per-region ListJobs sweep to answer "which region actually holds this job" gets every region reported as holding it, so a misdeployment to the wrong region is invisible and cleanup/inventory tooling deletes or reports against the wrong endpoint. Two same-named jobs in two regions also collapse into one record.

Fix: key the Glue job store by region in the Glue service state and make the enumeration path of the ListJobs handler (services/service-com-amazonaws-glue-awsglue/src/ops/list_jobs.rs) return only jobs owned by the request's region, with pagination applied to that region-filtered set. Related open issue 20261025T062017Z-glue-batchgetjobs-is-not-region-scoped-a-job.md records the same missing region key for BatchGetJobs and mentions ListJobs only as a follow-on remark in its Fix section; it does not carry this check's replay key, and 20261025T022820Z-glue-7-net-new-root-causes-gettables-expression.md (Cause 4) records it for StartJobRun/GetJobRuns. Neither covers the ListJobs enumeration path graded here — if the store is re-keyed centrally, this cause is closed by verifying ListJobs with the check id below.

Traces: glue.gjwr-region-sweep-locates-the-job-only-where-it-was-created.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against this checkout: 29/230 checks passed. Both failures below are net-new (neither is marked ALREADY FILED nor OWNED in the report).

Cause 1 — glue.batch-delete-connection-per-entry-errors-and-valid-deletes-land [mismatch]:
  File "check_glue_batch_delete_connection_per_entry_errors_and_valid_deletes_land.py", line 62, in run
  assert pre_message == f"Connection with name {absent} in account {ctx.account} is not found"
  AssertionError: expected the exact EntityNotFoundException message AWS returns, got 'Connection sim2real-behav-conn-d216e3be9dcc not found in catalog 000000000000'

  Expected (AWS): code EntityNotFoundException, HTTP 400, message "Connection with name sim2real-behav-conn-d216e3be9dcc in account <accountId> is not found".
  Observed (emulator): code EntityNotFoundException, message "Connection sim2real-behav-conn-d216e3be9dcc not found in catalog 000000000000".
  Fixed text vs values: "Connection with name " / " in account " / " is not found" are fixed; the connection name comes from the request and the account id from the caller's credentials. The particular name and the account digits above are per-run values, not requirements.

Cause 2 — glue.gjwr-region-sweep-locates-the-job-only-where-it-was-created [mismatch]:
  File "check_glue_gjwr_region_sweep_locates_the_job_only_where_it_was_created.py", line 44, in run
  assert family_hits[home] == [decoy]
  AssertionError: us-west-2 lists family jobs ['sim2real-gjwr-fam-c9123a35d4de-RoleReport-us-east-1', 'sim2real-gjwr-fam-c9123a35d4de-etl-usage-events-us-west-2'], expected only the decoy ['sim2real-gjwr-fam-c9123a35d4de-etl-usage-events-us-west-2']

  Expected (AWS): the us-west-2 endpoint's paginated ListJobs names only the job created through us-west-2; the us-east-1 endpoint names only the job created through us-east-1.
  Observed (emulator): each endpoint names both jobs. Job names and the region pair are per-run values; the requirement is the per-endpoint partition.

## Steps to Reproduce
Cause 1 (GetConnection message):
1. Boot the emulator from this checkout and replay simparity-behavioral-ga-prod-v1, re-running glue.batch-delete-connection-per-entry-errors-and-valid-deletes-land.
2. Call glue:GetConnection(Name='<a-name-that-was-never-created>').
3. Expect ClientError with Error.Code 'EntityNotFoundException', HTTP 400, and Error.Message exactly "Connection with name <that name> in account <your account id> is not found".
4. Observed: Error.Message is "Connection <that name> not found in catalog <account id>".

Cause 2 (ListJobs region scoping):
1. Replay glue.gjwr-region-sweep-locates-the-job-only-where-it-was-created.
2. With a glue client pinned to region A, CreateJob(Name='<tok>-a', Role='arn:aws:iam::<acct>:role/<tok>-a-role', Command={'Name':'glueetl','PythonVersion':'3','ScriptLocation':'s3://<tok>-a-assets/scripts/job.py'}, GlueVersion='4.0').
3. With a glue client pinned to region B, CreateJob(Name='<tok>-b', ...same shape...).
4. From region A, page glue:ListJobs to exhaustion and collect JobNames — expect it to contain '<tok>-a' and NOT '<tok>-b'.
5. From region B, do the same — expect '<tok>-b' and NOT '<tok>-a'.
6. Observed in both steps: JobNames contains both '<tok>-a' and '<tok>-b'.

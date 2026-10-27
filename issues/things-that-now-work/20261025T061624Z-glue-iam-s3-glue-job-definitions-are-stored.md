Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__iam__s3.gjwr-one-role-backs-jobs-in-both-regions
Issue-Key: glue__iam__s3.gjwr-one-role-backs-jobs-in-both-regions|run#8|AssertionError|assert mine == [job_name], f"{region}: BatchGetJobs over both names found {mine!r}, expected [{job_name!r}]"
Issue-Commit: 19a1db04630b2d63bbe4711f31ea8801f0875fe0

# glue__iam__s3: Glue job definitions are stored per account with no Region dimension, so one Region's endpoint returns another Region's jobs

> _Note: this issue was identified against the code base at commit `19a1db04630b2d63bbe4711f31ea8801f0875fe0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetJobs

## Description
Cause 1 of 1: the Glue job-definition namespace is account-global instead of per-Region, so a job created through one Region's endpoint is visible through every other Region's endpoint.

AWS: Glue is a regional service and a job definition lives only in the Region it was created in. Smithy: CreateJob.Name (NameString, len 1..255) is documented as unique within the account *in that Region*; GetJob answers EntityNotFoundException (HTTP 400) for a name the endpoint's Region does not hold; BatchGetJobs returns found definitions in `Jobs` and every name it cannot resolve in `JobsNotFound` (model: BatchGetJobsResponse$Jobs / $JobsNotFound). So with job A created in us-east-1 and job B created in us-west-2, BatchGetJobs([A,B]) against the us-west-2 endpoint returns Jobs=[B] and JobsNotFound=[A], and the mirror-image answer from us-east-1. The IAM role backing both jobs is account-global and is correctly the same entity from both Regions — only the job records are regional.

Emulator: BatchGetJobs issued against the us-west-2 endpoint over both job names returned BOTH definitions (recorded as ['sim2real-gjwr-both-<id>-us-east-1', 'sim2real-gjwr-both-<id>-us-west-2'] where only the us-west-2 name may appear), and nothing was reported in JobsNotFound. The service's own state documentation confirms the cause rather than a serializer slip: services/service-com-amazonaws-glue-awsglue/src/state.rs documents job definitions as living in a single per-account namespace keyed by job name, with no Region key at all, so every Region's endpoint reads and writes the same map.

Why: a client relying on AWS's behavior gets Region isolation wrong in both directions. Listing or batch-getting jobs from one Region silently returns jobs that endpoint cannot run, so a deployment tool that enumerates 'the jobs in this Region' to decide what to create, retire or migrate sees phantom records; a redeploy-to-the-correct-Region remediation appears already done; and a name collision that real Glue permits (the same job name in two Regions) can be mistaken for a duplicate. Conversely JobsNotFound, which is the documented signal a caller uses to detect 'this job does not exist here', can never fire.

Fix: give the job-definition registry in the glue service crate's state a Region dimension (keyed by the Region the request was signed for, alongside the account/catalog id), and have the job handlers — BatchGetJobs, GetJob, CreateJob and the list/update/delete paths that share the registry — resolve names only within the requesting endpoint's Region, with BatchGetJobs placing unresolvable names in JobsNotFound and GetJob answering EntityNotFoundException.

Traces: glue__iam__s3.gjwr-one-role-backs-jobs-in-both-regions. Related but distinct, and already open: 20261025T000319Z-glue-iam-s3-s3-bucket-names-are-a.md covers the opposite defect on S3 (a GLOBAL namespace emulated per-Region); this issue is a REGIONAL namespace emulated globally and is not covered by it.

## Evidence
Failing check glue__iam__s3.gjwr-one-role-backs-jobs-in-both-regions (verdict: mismatch).

AssertionError at check_glue__iam__s3_gjwr_one_role_backs_jobs_in_both_regions.py line 40:
  'us-west-2: BatchGetJobs over both names found ['sim2real-gjwr-both-195298016bd0-us-east-1', 'sim2real-gjwr-both-195298016bd0-us-west-2'], expected ['sim2real-gjwr-both-195298016bd0-us-west-2']'

Expected (AWS): BatchGetJobs against the us-west-2 endpoint over both names -> Jobs contains only the us-west-2 job, the us-east-1 name is returned in JobsNotFound; symmetrically from us-east-1.
Actual (emulator): both job definitions returned from the us-west-2 endpoint; JobsNotFound empty.

The earlier legs of the same check passed, which narrows the cause: GetRole through clients pinned to each Region returned the same RoleId/Arn (IAM is correctly account-global), and each job's GetJob stored the role ARN verbatim. Only the per-Region separation of the job records diverges. Generated job names, the account id and the Region pair are environment detail; the requirement is that a job definition is visible only through the Region it was created in.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Create an IAM role trusting glue.amazonaws.com (account-global; it is shared by both jobs).
3. glue:CreateJob NAME-us-east-1 against the us-east-1 endpoint, with that role ARN.
4. glue:CreateJob NAME-us-west-2 against the us-west-2 endpoint, with the same role ARN.
5. glue:BatchGetJobs(JobNames=[NAME-us-east-1, NAME-us-west-2]) against the us-west-2 endpoint — observe both definitions returned in Jobs and an empty JobsNotFound, where AWS returns only NAME-us-west-2 in Jobs and NAME-us-east-1 in JobsNotFound. Repeat against us-east-1 for the mirror case.
6. glue:GetJob(NAME-us-east-1) against us-west-2 — AWS answers EntityNotFoundException.
7. Re-replay glue__iam__s3.gjwr-one-role-backs-jobs-in-both-regions to confirm the fix.

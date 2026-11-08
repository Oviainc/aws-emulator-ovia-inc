Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplify.start-deployment-rejects-a-job-whose-zip-was-never-uploaded
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify.start-deployment-rejects-a-job-whose-zip-was-never-uploaded|run#8|AssertionError|assert status == "PENDING", f"the refused job is {status!r}, expected it left PENDING"
Issue-Commit: 386e377a24364e2ae13f63408a712b163326bf5e

# Amplify: a CreateDeployment-reserved job reports status CREATED where AWS reports PENDING, so a StartDeployment refused for a missing zip leaves the job in a status no client can poll

> _Note: this issue was identified against the code base at commit `386e377a24364e2ae13f63408a712b163326bf5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJob

## Description
One net-new root cause in the amplify emulator, grounded in a behavioral check that exercises the manual-deployment upload gate.

Cause 1 of 1: the branch job that CreateDeployment reserves is projected with JobSummary.status 'CREATED' instead of 'PENDING', both when it is first reserved and after StartDeployment refuses it because the deployment zip was never PUT to the pre-signed URL.
  AWS: Smithy models CreateDeployment -> POST /apps/{appId}/branches/{branchName}/deployments (output jobId / zipUploadUrl / fileUploadUrls), StartDeployment -> POST /apps/{appId}/branches/{branchName}/deployments/start with output jobSummary @required and errors BadRequestException (400) / NotFoundException (404) / UnauthorizedException (401) / LimitExceededException (429) / InternalFailureException (500), and GetJob -> GET /apps/{appId}/branches/{branchName}/jobs/{jobId} with output job @required (Job{summary: JobSummary, steps}). JobSummary$status targets enum JobStatus with values CREATED | PENDING | PROVISIONING | RUNNING | FAILED | SUCCEED | CANCELLING | CANCELLED. The model does not say which of those a reserved manual-deployment job carries, so for that: recorded behavior only — a job reserved by CreateDeployment and not yet started reads back from GetJob with summary.status 'PENDING', and a StartDeployment that AWS refuses with BadRequestException (HTTP 400) whose message is the fixed text "Deployment file has not been uploaded, please make sure you uploaded the deployment file prior to invoke this api." (no variable parts) does NOT advance or change that status — the job is still 'PENDING' on the next GetJob, and a subsequent StartDeployment after the zip is uploaded drives it to SUCCEED.
  Emulator: the refusal itself is correct — the check's assertion on (BadRequestException, "Deployment file has not been uploaded, please make sure you uploaded the deployment file prior to invoke this api.") passes. The divergence is the status projection: GetJob on that same jobId answers summary.status 'CREATED'. The reserved job is created in the CreateDeployment handler's reserved-job summary with the crate's CREATED job status (services/service-com-amazonaws-amplify-amplify/src/ops/create_deployment.rs, JOB_STATUS_CREATED in services/service-com-amazonaws-amplify-amplify/src/state.rs), and StartDeployment's refusal path leaves it untouched, so the wrong status is what the client reads back.
  Why: a client drives a manual deployment by polling GetJob/ListJobs and waiting for the documented PENDING -> PROVISIONING/RUNNING -> SUCCEED progression, and retries or reports the upload gate while the job is still PENDING. Told 'CREATED' — a JobStatus value AWS does not use for a reserved deployment job — such a client either loops forever waiting for a status it will never see, treats the job as being in an unknown/unsupported state, or wrongly concludes the refused StartDeployment mutated the job. Any status-keyed dashboard or waiter written against real Amplify mis-classifies every manual deployment the emulator reserves.
  Fix: the CreateDeployment handler's reserved-job summary construction and the shared JobSummary status projection in this crate's job state — a reserved, never-started deployment job must project PENDING, and the StartDeployment refusal path must leave that status unchanged.
  Traces: behavioral check amplify.start-deployment-rejects-a-job-whose-zip-was-never-uploaded.

Related open issues, named rather than relied on: 20261105T200342Z-amplify-job-lifecycle-and-projection-reserved-jobs-report.md describes the same CREATED-vs-PENDING projection from the manual-deploy wire fixture (its Cause 1, keyed on amplify-afs-traj-manual-deploy-job-sequencing#task#7) and 20261102T092113Z-amplify-branch-and-job-projection-wrong-stage-default.md covers the earlier shape of those job steps; neither carries this behavioral check's replay key, and the suite reports this check as unowned, so it is filed here with its own grounding. 20261029T215038Z-amplify-job-and-deployment-starts-that-aws-refuses.md covers StartDeployment/StartJob REFUSALS, which this check confirms are already correct, not the status left behind.

## Evidence
Behavioral check amplify.start-deployment-rejects-a-job-whose-zip-was-never-uploaded (suite simparity-behavioral-aws-v1), verdict mismatch:

  File "check_amplify_start_deployment_rejects_a_job_whose_zip_was_never_uploaded.py", line 38, in run
    assert status == "PENDING", f"the refused job is {status!r}, expected it left PENDING"
  AssertionError: the refused job is 'CREATED', expected it left PENDING

Sequence exercised, with the per-run values (app id, branch, job number, region, account, pre-signed host/port) being environment-specific and not part of the requirement:
  1. CreateApp, CreateBranch(branchName='main'), CreateDeployment(appId, 'main') -> jobId + zipUploadUrl. Emulator answers 200; passes.
  2. StartDeployment(appId, 'main', jobId) with nothing uploaded -> expected and observed HTTP 400 BadRequestException, message "Deployment file has not been uploaded, please make sure you uploaded the deployment file prior to invoke this api." (entirely fixed text, no request- or state-derived parts). This assertion PASSES — the error code, status and message match AWS.
  3. GetJob(appId, 'main', jobId) -> $.job.summary.status expected 'PENDING', actual 'CREATED'. This is the divergence.
The check's remaining steps (PUT the zip to the pre-signed URL, re-issue the identical StartDeployment, poll to SUCCEED) were never reached because of the failure at step 3.

## Steps to Reproduce
1. app = amplify.create_app(name=<unique>)['app']; amplify.create_branch(appId=app['appId'], branchName='main').
2. dep = amplify.create_deployment(appId=app['appId'], branchName='main')  # returns jobId and zipUploadUrl.
3. amplify.start_deployment(appId=app['appId'], branchName='main', jobId=dep['jobId']) WITHOUT first PUTting a zip to dep['zipUploadUrl'] -> expect ClientError ('BadRequestException', 'Deployment file has not been uploaded, please make sure you uploaded the deployment file prior to invoke this api.'); this is what the emulator already answers.
4. amplify.get_job(appId=app['appId'], branchName='main', jobId=dep['jobId'])['job']['summary']['status'] -> must be 'PENDING'; the emulator reports 'CREATED'.
5. (Blocked by step 4) PUT a zip to dep['zipUploadUrl'] (expect HTTP 200), re-issue the identical start_deployment, then poll get_job to a terminal status and expect 'SUCCEED'.

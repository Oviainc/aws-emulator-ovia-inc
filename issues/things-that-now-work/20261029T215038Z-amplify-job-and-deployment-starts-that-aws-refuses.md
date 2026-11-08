Issue-Source: trace-sweep-divergence
Issue-Trace: amplify-job-start-and-manage
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amplify-job-start-and-manage amplify-manual-deployment-flow
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify|StartJob|4xx->2xx|$.message
Issue-Key: amplify|StartJob|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: amplify|StartJob|4xx->2xx|status
Issue-Key: amplify|ListJobs|2xx->2xx|$.jobSummaries[*]
Issue-Key: amplify|DeleteBranch|2xx->2xx|$.branch.activeJobId
Issue-Key: amplify|DeleteBranch|2xx->2xx|$.branch.totalNumberOfJobs
Issue-Key: amplify|CreateDeployment|2xx->2xx|$.fileUploadUrls
Issue-Key: amplify|StartDeployment|4xx->2xx|$.message
Issue-Key: amplify|StartDeployment|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: amplify|StartDeployment|4xx->2xx|status
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# Amplify job and deployment starts that AWS refuses are accepted with 200, leaking phantom jobs and counters

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJob

## Description
Five net-new root causes in the amplify service's job/deployment surface, all grounded in replayed wire fixture steps. They share one fault: the emulator has no preconditions on starting work and no job-accounting discipline, so calls AWS refuses with 400 BadRequestException succeed with 200, and the phantom jobs they mint then leak into ListJobs and into the Branch record.

Cause 1 of 5: StartJob on an app that is not connected to a repository provider is accepted (200) instead of refused with 400 BadRequestException.
  AWS: Smithy models StartJob -> POST /apps/{appId}/branches/{branchName}/jobs with `jobType` @required (enum JobType) and modeled error BadRequestException (HTTP 400); JobSummary.jobType documents that RELEASE/RETRY "is available only for apps that are connected to a repository" and that MANUAL "is for a manually deployed app". Recorded behavior: on an app created WITHOUT a repository, StartJob is refused with HTTP 400, x-amzn-errortype `BadRequestException`, message "Operation not supported for app that was not connected to a repository provider. Please use CreateDeployment and StartDeployment operations instead." — entirely fixed text; it carries no value from the request or state.
  Emulator: answers HTTP 200 with a JobSummary (no `message`, no `x-amzn-errortype` header), creating a job on an app that has no repository to build from.
  Why: a client is told the build was started when nothing can build it; it then polls GetJob for a job that will never reflect a real repository build, instead of being redirected to CreateDeployment/StartDeployment as AWS's message explicitly instructs.
  Fix: the StartJob handler's preconditions — reject StartJob on an app with no repository provider with 400 BadRequestException and AWS's fixed message, before any job is minted.
  Traces: amplify-job-start-and-manage#task#3.

Cause 2 of 5: the refused StartJob leaves job history behind, so ListJobs reports a job AWS never created.
  AWS: Smithy models ListJobs -> GET /apps/{appId}/branches/{branchName}/jobs with output `jobSummaries` @required. Recorded behavior: after the refused StartJob above, ListJobs returns an EMPTY jobSummaries list — a refused start creates no job.
  Emulator: returns one JobSummary (jobId '1', status 'PENDING', jobType 'MANUAL', empty commitId/commitMessage) that exists only because the start was wrongly accepted.
  Why: a client reconciling branch state (or a teardown that waits for jobs to finish) sees a PENDING job that will never progress, and any "is a build in flight?" check is permanently true.
  Fix: the StartJob handler plus the job store it writes to — a rejected start must not commit a job record; ListJobs then reports the empty list from state.
  Traces: amplify-job-start-and-manage#task#4 (key $.jobSummaries[0] reported as `extra`).

Cause 3 of 5: DeleteBranch's returned Branch reports a stale activeJobId and a non-zero totalNumberOfJobs where AWS reports no active job and '0'.
  AWS: Smithy models DeleteBranch -> DELETE /apps/{appId}/branches/{branchName} returning Branch, whose `activeJobId` (ActiveJobId, @required, @length 0..1000) is "The ID of the active job for a branch of an Amplify app" and whose `totalNumberOfJobs` (@required, @length 0..1000) is "The total number of jobs that are part of an Amplify app" — both STRING members. Recorded behavior: the deleted branch comes back with `activeJobId` ABSENT and `totalNumberOfJobs` '0', i.e. the branch carries no active job and no job count once its jobs are gone.
  Emulator: returns activeJobId '1' (type_mismatch: expected absent) and totalNumberOfJobs '1'.
  Why: a client that reads the DeleteBranch result (the last view it ever gets of that branch) to confirm nothing is still running sees a live active job on a branch that no longer exists, and job-count bookkeeping is off by the phantom jobs.
  Fix: the Branch projection/accounting behind DeleteBranch — omit activeJobId when no job is active and derive totalNumberOfJobs from the branch's committed job records.
  Traces: amplify-job-start-and-manage#teardown#9 and amplify-manual-deployment-flow#teardown#5 (same cluster; keys $.branch.activeJobId, $.branch.totalNumberOfJobs).

Cause 4 of 5: CreateDeployment returns an empty `fileUploadUrls` map where AWS omits the member for a zip-upload deployment.
  AWS: Smithy models CreateDeployment -> POST /apps/{appId}/branches/{branchName}/deployments with output members `fileUploadUrls` (FileUploadUrls map, @required) and `zipUploadUrl` (UploadUrl, @required), documented such that `zipUploadUrl` is returned "when the fileMap argument is not provided in the request". Recorded behavior: for a request with no `fileMap`, the response omits `fileUploadUrls` entirely and carries only jobId + zipUploadUrl.
  Emulator: returns `fileUploadUrls: {}` (type_mismatch: expected member absent, got {}).
  Why: a client distinguishes the two deployment modes by which member is present; a present-but-empty map makes the per-file mode look selected with zero files, so a client that iterates fileUploadUrls uploads nothing and never uses the zip URL.
  Fix: the CreateDeployment handler's response serialization — emit fileUploadUrls only for the fileMap form of the request.
  Traces: amplify-manual-deployment-flow#task#3.

Cause 5 of 5: StartDeployment is accepted (200) for a deployment whose file was never uploaded, instead of 400 BadRequestException.
  AWS: Smithy models StartDeployment -> POST /apps/{appId}/branches/{branchName}/deployments/start with modeled error BadRequestException (HTTP 400), taking the `jobId` reserved by CreateDeployment. Recorded behavior: calling StartDeployment for a reserved jobId whose zip was never PUT to the returned zipUploadUrl is refused with HTTP 400, x-amzn-errortype `BadRequestException`, message "Deployment file has not been uploaded, please make sure you uploaded the deployment file prior to invoke this api." — entirely fixed text; it carries no value from the request or state.
  Emulator: answers HTTP 200 with a JobSummary (no `message`, no `x-amzn-errortype` header), starting a deployment with no content behind it.
  Why: the upload step silently becomes optional. A client whose upload failed is told the deployment started and then waits on a job that can never serve anything, rather than being told to upload first — exactly the failure mode AWS's message exists to prevent.
  Fix: the StartDeployment handler's precondition on the reserved deployment source — refuse with 400 BadRequestException and AWS's fixed message when no uploaded content is present for the jobId.
  Traces: amplify-manual-deployment-flow#task#4.

Related but NOT covering these: open issue 20261029T185952Z-amplify-manual-deployment-data-plane-the-zipuploadurl-handed.md covers (a) the zipUploadUrl PUT answering 404 and (b) StartJob(RETRY) returning the wrong MESSAGE on a 400 — it is filed against the behavioral corpus and carries no wire replay keys, and neither of its causes is any of the five above (Cause 1 here is StartJob answering 200 with no error at all; Causes 2-5 are untouched by it). Code citations for orientation only, from that issue's commit: services/service-com-amazonaws-amplify-amplify/src/ops/start_job.rs:174-182 and src/ops/create_deployment.rs:302-307.

## Evidence
Cause 1 (amplify-job-start-and-manage#task#3, StartJob): expected HTTP 400, header x-amzn-errortype 'BadRequestException', $.message 'Operation not supported for app that was not connected to a repository provider. Please use CreateDeployment and StartDeployment operations instead.' (all fixed text). Actual: HTTP 200, no $.message, no x-amzn-errortype header.

Cause 2 (amplify-job-start-and-manage#task#4, ListJobs): expected $.jobSummaries == [] (no element 0). Actual: $.jobSummaries[0] present = {'jobArn': 'arn:aws:amplify:<region>:<account>:apps/<appId>/branches/<branch>/jobs/1', 'jobId': '1', 'commitId': '', 'commitMessage': '', 'commitTime': <epoch>, 'startTime': <epoch>, 'status': 'PENDING', 'jobType': 'MANUAL'} — the arn/ids/timestamps are per-run values; the requirement is that the list is empty.

Cause 3 (amplify-job-start-and-manage#teardown#9, also amplify-manual-deployment-flow#teardown#5, DeleteBranch): expected $.branch.activeJobId ABSENT and $.branch.totalNumberOfJobs '0'. Actual: activeJobId '1', totalNumberOfJobs '1'.

Cause 4 (amplify-manual-deployment-flow#task#3, CreateDeployment): expected $.fileUploadUrls ABSENT (request carried no fileMap). Actual: $.fileUploadUrls {}.

Cause 5 (amplify-manual-deployment-flow#task#4, StartDeployment): expected HTTP 400, header x-amzn-errortype 'BadRequestException', $.message 'Deployment file has not been uploaded, please make sure you uploaded the deployment file prior to invoke this api.' (all fixed text). Actual: HTTP 200, no $.message, no x-amzn-errortype header.

## Steps to Reproduce
Causes 1-3:
1. amplify.create_app(name=<unique>) with NO repository -> appId; amplify.create_branch(appId, branchName=<unique>).
2. amplify.start_job(appId=appId, branchName=branchName, jobType='MANUAL').
   Observed: HTTP 200 with a JobSummary. Expected: HTTP 400 BadRequestException 'Operation not supported for app that was not connected to a repository provider. Please use CreateDeployment and StartDeployment operations instead.'
3. amplify.list_jobs(appId, branchName).
   Observed: one PENDING MANUAL job (jobId '1'). Expected: jobSummaries == [].
4. amplify.delete_branch(appId, branchName).
   Observed: branch.activeJobId '1', branch.totalNumberOfJobs '1'. Expected: activeJobId absent, totalNumberOfJobs '0'.

Causes 4-5:
1. amplify.create_app(name=<unique>) -> appId; amplify.create_branch(appId, branchName=<unique>).
2. r = amplify.create_deployment(appId=appId, branchName=branchName)   # no fileMap
   Observed: response contains fileUploadUrls {}. Expected: no fileUploadUrls member (only jobId + zipUploadUrl).
3. WITHOUT uploading anything to r['zipUploadUrl'], call amplify.start_deployment(appId=appId, branchName=branchName, jobId=r['jobId']).
   Observed: HTTP 200 with a JobSummary. Expected: HTTP 400 BadRequestException 'Deployment file has not been uploaded, please make sure you uploaded the deployment file prior to invoke this api.'
4. amplify.delete_branch(appId, branchName) -> expect activeJobId absent and totalNumberOfJobs '0'.

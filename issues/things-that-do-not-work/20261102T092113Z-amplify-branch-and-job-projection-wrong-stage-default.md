Issue-Source: trace-sweep-divergence
Issue-Trace: amplify-afs-traj-branch-before-deploy-and-retry
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amplify-afs-traj-branch-before-deploy-and-retry amplify-afs-traj-manual-deploy-job-sequencing amplify-afs-traj-s3-source-deploy-mistakes
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify|CreateBranch|2xx->2xx|$.branch.stage
Issue-Key: amplify|ListBranches|2xx->2xx|$.branches[*]
Issue-Key: amplify|ListBranches|2xx->2xx|$.branches[*].stage
Issue-Key: amplify|CreateDeployment|2xx->2xx|$.fileUploadUrls.index.html
Issue-Key: amplify|CreateDeployment|2xx->2xx|$.zipUploadUrl
Issue-Key: amplify|GetJob|2xx->4xx|$.job
Issue-Key: amplify|GetJob|2xx->4xx|status
Issue-Key: amplify|StopJob|2xx->4xx|$.jobSummary
Issue-Key: amplify|StopJob|2xx->4xx|status
Issue-Key: amplify|StartDeployment|2xx->2xx|$.jobSummary.commitId
Issue-Key: amplify|StartDeployment|2xx->2xx|$.jobSummary.commitMessage
Issue-Key: amplify|StartDeployment|2xx->2xx|$.jobSummary.commitTime
Issue-Key: amplify|StartDeployment|2xx->2xx|$.jobSummary.jobArn
Issue-Key: amplify|StartDeployment|2xx->2xx|$.jobSummary.jobId
Issue-Key: amplify|StartDeployment|2xx->2xx|$.jobSummary.jobType
Issue-Key: amplify|StartDeployment|2xx->2xx|$.jobSummary.startTime
Issue-Related: 20261029T215038Z-amplify-job-and-deployment-starts-that-aws-refuses.md
Issue-Commit: 798fb32aa8eb1e75369deb4b48c94746eb1ca248

# amplify branch and job projection: wrong stage default, absent members serialized, and reserved jobs invisible

> _Note: this issue was identified against the code base at commit `798fb32aa8eb1e75369deb4b48c94746eb1ca248`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBranch

## Description
Seven net-new root causes in how the amplify control plane projects branch and job state onto the wire, each grounded in a replayed wire fixture step. They share one fault: the emulator's Branch/JobSummary/CreateDeployment projections invent values AWS does not report (defaults, empty strings, empty maps, timestamps) and omit records AWS does report (the reserved deployment job). They are independent defects and must each be closed before this issue is submitted.

Cause 1 of 7: a branch created without `stage` is reported with stage 'DEVELOPMENT' where AWS reports 'NONE'.
  AWS: Smithy models Branch$stage as @required targeting enum Stage (PRODUCTION, BETA, DEVELOPMENT, EXPERIMENTAL, PULL_REQUEST) with no @default, and CreateBranch.stage as optional. Recorded behavior only: CreateBranch with no `stage` returns a branch whose stage is 'NONE' - a value outside the modeled enum, so the model says nothing about this default and the recording is the contract.
  Emulator: reports 'DEVELOPMENT'.
  Why: a client that branches on stage to tell a configured branch from an unconfigured one is told every new branch is a DEVELOPMENT branch, so stage-gating and promotion logic take the wrong path.
  Fix: the CreateBranch handler's default for `stage` and the Branch projection that reports it.
  Note on an existing issue: same root cause as Cause 2 of the open behavioral issue 20261102T085926Z-amplify-control-plane-8-net-new-root-causes.md, which carries only behavioral check ids and no wire replay key; restated here so this wire step is carried as a replay key.
  Traces: amplify-afs-traj-branch-before-deploy-and-retry#task#5.

Cause 2 of 7: the ListBranches projection repeats that wrong stage default AND lists a branch whose creation AWS refused.
  AWS: Smithy models ListBranches -> GET /apps/{appId}/branches with output `branches` @required (list of Branch). Recorded behavior: each listed branch carries the same stage the create/read path reports ('NONE' for a branch created without `stage`), and a branch name AWS refuses never appears in the list at all.
  Emulator: branches[0].stage is 'DEVELOPMENT', and the list carries an extra element for the branch name 'main site' that CreateBranch should have refused (the per-element fields - arn, ids, timestamps - are per-run values; the requirement is that the element is absent).
  Why: ListBranches is how a client reconciles an app's branches; it reports a branch that does not exist on AWS and misreports the stage of the ones that do, so reconciliation and teardown loops act on phantom state.
  Fix: the Branch projection shared by ListBranches (same default as Cause 1); the extra element disappears once CreateBranch refuses the illegal name - filed as Cause 5 of the companion issue 'amplify request refusals: wrong validation message text, wrong error class, and three refusals never made' - so this step must be re-replayed after both.
  Traces: amplify-afs-traj-branch-before-deploy-and-retry#task#7.

Cause 3 of 7: CreateDeployment with a `fileMap` serializes an empty `zipUploadUrl` where AWS omits the member.
  AWS: Smithy models CreateDeployment output members `fileUploadUrls` (FileUploadUrls map, @required) and `zipUploadUrl` (UploadUrl, @required, @length 0..1000), documented so that `zipUploadUrl` is returned when `fileMap` is NOT provided. Recorded behavior: for a request that DOES carry a fileMap, the response carries only jobId and the per-file `fileUploadUrls` entries and omits `zipUploadUrl` entirely (the model's @required is overridden by the recording). The per-file entry for the requested file name also diverges in shape at this step.
  Emulator: emits `zipUploadUrl: ""` (an empty string) alongside the fileUploadUrls map.
  Why: a client picks the deployment mode by which member is present. A present-but-empty zipUploadUrl makes the zip mode look selected with no URL, so a client that prefers the zip path PUTs to the empty string and the upload fails with no indication which mode it should have used.
  Fix: the CreateDeployment handler's response serialization - emit `zipUploadUrl` only for the no-fileMap form of the request, and never as an empty string.
  Traces: amplify-afs-traj-manual-deploy-job-sequencing#task#3.

Cause 4 of 7: the mirror image - CreateDeployment without a `fileMap` serializes an empty `fileUploadUrls` map where AWS omits the member, and the `zipUploadUrl` it hands back does not satisfy the recorded shape.
  AWS: same model members as Cause 3. Recorded behavior: for a request with no fileMap, the response omits `fileUploadUrls` entirely and carries jobId plus a `zipUploadUrl` the client can PUT the deployment zip to - i.e. a URL on the endpoint the client reached the service on.
  Emulator: emits `fileUploadUrls: {}`, and its `zipUploadUrl` fails the step's shape assertion because the handler announces a loopback address and port of its own rather than the endpoint the request arrived on, so the URL it hands back is not reachable by the client that received it.
  Why: a present-but-empty map makes the per-file mode look selected with zero files, so a client that iterates fileUploadUrls uploads nothing and never uses the zip URL; and a client that does use the zip URL cannot reach it.
  Fix: the CreateDeployment handler's response serialization (omit fileUploadUrls for the no-fileMap form) and the upload-URL construction, which must be built from the request's own endpoint rather than a self-chosen address.
  Note on an existing issue: the fileUploadUrls half is the same root cause as Cause 4 of the open wire issue 20261029T215038Z-amplify-job-and-deployment-starts-that-aws-refuses.md, which carries the amplify-manual-deployment-flow#task#3 replay key only - not this step, and not the zipUploadUrl half.
  Traces: amplify-afs-traj-manual-deploy-job-sequencing#task#12.

Cause 5 of 7: CreateDeployment reserves a job id but records no job, so GetJob answers 404 for an id the service just handed out.
  AWS: Smithy models GetJob -> GET /apps/{appId}/branches/{branchName}/jobs/{jobId} with output `job` @required and NotFoundException (404) among its errors. Recorded behavior: the jobId CreateDeployment returns is immediately readable - GetJob reports that job with status 'PENDING' and an empty `steps` list, and its summary's jobArn ends with the job number zero-padded to ten digits (arn:aws:...:apps/<appId>/branches/<branch>/jobs/0000000001) while `jobId` is the bare number ('1'); the commit/time/type members are absent at this point.
  Emulator: answers HTTP 404 with no `job` member.
  Why: the documented manual-deployment flow is CreateDeployment -> upload -> StartDeployment, so a client that polls or displays the reserved job between those steps gets a not-found for an id it was just given, and a teardown that enumerates outstanding deployments cannot find them.
  Fix: the CreateDeployment handler and this crate's job store - commit a PENDING job record under the reserved id at reservation time, which GetJob then reads; the job-record ARN construction must zero-pad the job number to ten digits while jobId stays bare.
  Note on an existing issue: same root cause as Cause 7 of the open behavioral issue 20261102T085926Z-amplify-control-plane-8-net-new-root-causes.md, which carries no wire replay key.
  Traces: amplify-afs-traj-manual-deploy-job-sequencing#task#7.

Cause 6 of 7: StopJob on that reserved deployment answers 404 instead of cancelling it.
  AWS: Smithy models StopJob -> DELETE /apps/{appId}/branches/{branchName}/jobs/{jobId}/stop with output `jobSummary` @required and NotFoundException (404) among its errors. Recorded behavior: StopJob on the id CreateDeployment reserved answers HTTP 200 with a jobSummary whose status is 'CANCELLED' and whose jobArn carries the ten-digit-padded job number, jobId being the bare number.
  Emulator: answers HTTP 404 with no `jobSummary`.
  Why: a reserved deployment can never be cancelled, so the only documented escape from the "last job not finished" state (AWS's own refusal message tells the client to use StopJob) is unavailable, and the branch stays blocked.
  Fix: the same reservation/job-store gap as Cause 5, plus the StopJob handler's lookup, which must find and cancel a reserved PENDING job.
  Traces: amplify-afs-traj-manual-deploy-job-sequencing#task#8, and amplify-afs-traj-manual-deploy-job-sequencing#task#11 in the same cluster.

Cause 7 of 7: StartDeployment's JobSummary invents commit/time/type values AWS omits, leaves the job ARN unpadded, and reports a skewed job number.
  AWS: Smithy models StartDeployment output `jobSummary` @required -> JobSummary, whose commitId/commitMessage/commitTime/startTime/jobType are @required in the model. Recorded behavior overrides that for a manual deployment at start time: commitId, commitMessage, commitTime, startTime, jobType, sourceUrl, sourceUrlType and endTime are all ABSENT, `jobId` is the bare job number, and `jobArn` ends with that number zero-padded to ten digits (.../jobs/0000000001). The job number is the next one for the branch, and a refused StartDeployment consumes none.
  Emulator: emits commitId '' and commitMessage '' (empty strings instead of absent members), commitTime and startTime as wall-clock timestamps, jobType 'MANUAL', a jobArn ending '/jobs/<n>' with no padding, and a job number one higher than AWS's because an earlier StartDeployment that AWS refuses was accepted and consumed a number.
  Why: a client cannot tell "no commit information" from "an empty commit id", so commit-based correlation and display logic treat a manual deployment as a repository build with a blank commit; an unpadded ARN is one AWS would never emit, so IAM resource matching, tagging and log correlation built from it do not match; and the skewed job number means every id the client remembers from the recording points at a different job.
  Fix: the JobSummary projection and the job-record ARN construction in this crate's job state - omit the members AWS omits rather than defaulting them to '' or now(), and pad the ARN's job segment to ten digits. The number skew clears once the refused StartDeployment stops consuming a job id (filed as Cause 5 of the open issue 20261029T215038Z-amplify-job-and-deployment-starts-that-aws-refuses.md, which owns that step's replay key), so re-replay this step after both.
  Traces: amplify-afs-traj-s3-source-deploy-mistakes#task#12.

## Evidence
Cause 1 (amplify-afs-traj-branch-before-deploy-and-retry#task#5, CreateBranch, HTTP 200 both sides): $.branch.stage expected 'NONE', actual 'DEVELOPMENT'.

Cause 2 (amplify-afs-traj-branch-before-deploy-and-retry#task#7, ListBranches, HTTP 200 both sides): $.branches[0].stage expected 'NONE', actual 'DEVELOPMENT'; $.branches[1] expected ABSENT, actual an element for branchName 'main site' (its branchArn, timestamps and ids are per-run values - the requirement is that no element exists for a name AWS refuses).

Cause 3 (amplify-afs-traj-manual-deploy-job-sequencing#task#3, CreateDeployment, HTTP 200 both sides): $.zipUploadUrl expected ABSENT (the request carried a fileMap), actual '' (empty string); $.fileUploadUrls entry for the requested file name also reported divergent at this step.

Cause 4 (amplify-afs-traj-manual-deploy-job-sequencing#task#12, CreateDeployment, HTTP 200 both sides): $.fileUploadUrls expected ABSENT (no fileMap in the request), actual {}; $.zipUploadUrl failed its shape assertion - actual 'https://127.0.0.1:34037/apps/<appId>/deployments/<n>/uploads/<hash>', a loopback host/port the handler chose rather than the endpoint the request was made to (the specific host, port and hash are per-run values).

Cause 5 (amplify-afs-traj-manual-deploy-job-sequencing#task#7, GetJob): expected HTTP 200 with $.job = {'steps': [], 'summary': {'jobId': '1', 'status': 'PENDING', 'jobArn': 'arn:aws:amplify:<region>:<account>:apps/<appId>/branches/main/jobs/0000000001', all commit/time/type members absent}}. Actual: HTTP 404, no $.job. (Region, account and appId are per-run; the requirements are the 200, the PENDING status, the absent members and the ten-digit ARN padding.)

Cause 6 (amplify-afs-traj-manual-deploy-job-sequencing#task#8, also #task#11, StopJob): expected HTTP 200 with $.jobSummary = {'jobId': '1', 'status': 'CANCELLED', 'jobArn': '.../branches/main/jobs/0000000001', commit/time/type members absent}. Actual: HTTP 404, no $.jobSummary.

Cause 7 (amplify-afs-traj-s3-source-deploy-mistakes#task#12, StartDeployment, HTTP 200 both sides): $.jobSummary.commitId expected ABSENT, actual ''; commitMessage expected ABSENT, actual ''; commitTime expected ABSENT, actual a wall-clock timestamp; startTime expected ABSENT, actual a wall-clock timestamp; jobType expected ABSENT, actual 'MANUAL'; jobId expected '1', actual '2'; jobArn expected '.../branches/main/jobs/0000000001', actual '.../branches/main/jobs/2' (unpadded and carrying the skewed number).

## Steps to Reproduce
Cause 1:
1. app = amplify.create_app(name=<unique>)['app'].
2. amplify.create_branch(appId=app['appId'], branchName='main') -> branch.stage must be 'NONE'; observed 'DEVELOPMENT'.

Cause 2:
1. Continue from Cause 1, then amplify.create_branch(appId, branchName='main site') (AWS refuses this - see the companion refusals issue).
2. amplify.list_branches(appId) -> branches must contain exactly one element, for 'main', with stage 'NONE'; observed two elements, the first with stage 'DEVELOPMENT'.

Cause 3:
1. Create an app and branch 'main'.
2. amplify.create_deployment(appId, 'main', fileMap={'index.html': <md5>}) -> the response must carry jobId + fileUploadUrls only, with NO zipUploadUrl member; observed zipUploadUrl ''.

Cause 4:
1. Create an app and branch 'main'.
2. r = amplify.create_deployment(appId, 'main')   # no fileMap
   -> the response must carry no fileUploadUrls member (observed {}), and r['zipUploadUrl'] must be a URL on the endpoint the client called (observed a 127.0.0.1:<port> URL).

Causes 5 and 6:
1. Create an app and branch 'main'; d = amplify.create_deployment(appId, 'main').
2. amplify.get_job(appId, 'main', d['jobId']) -> expect 200 with status 'PENDING', empty steps, and summary.jobArn ending '/jobs/0000000001' while jobId is '1'; observed 404.
3. amplify.stop_job(appId, 'main', d['jobId']) -> expect 200 with jobSummary.status 'CANCELLED'; observed 404.

Cause 7:
1. Create an app and branch 'main'.
2. amplify.start_deployment(appId, 'main', sourceUrl=<an s3 URL AWS refuses, e.g. one not ending in '/'>) -> AWS refuses it and consumes no job number.
3. amplify.start_deployment(appId, 'main', sourceUrl=<a valid s3 prefix URL ending in '/'>, sourceUrlType='BUCKET_PREFIX') -> jobSummary must report jobId '1', jobArn ending '/jobs/0000000001', and NO commitId/commitMessage/commitTime/startTime/jobType members; observed jobId '2', jobArn '.../jobs/2', commitId '', commitMessage '', commitTime/startTime set, jobType 'MANUAL'.

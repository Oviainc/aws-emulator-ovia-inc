Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplify.create-app-rejects-an-unknown-platform amplify.create-branch-applies-defaults-and-derives-a-hyphenated-display-name amplify.create-branch-rejects-a-name-containing-a-space amplify.create-deployment-rejects-a-branch-that-does-not-exist amplify.manual-deployment-job-runs-two-hosting-steps-and-pads-the-job-arn amplify.start-deployment-rejects-a-job-whose-zip-was-never-uploaded amplify.stop-job-cancels-a-pending-deployment-and-list-jobs-orders-newest-first
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify.create-app-rejects-an-unknown-platform|run#4|AssertionError|assert err == (
Issue-Key: amplify.create-branch-applies-defaults-and-derives-a-hyphenated-display-name|run#6|AssertionError|assert defaults == {"stage": "NONE", "displayName": "main", "enableAutoBuild": True, "ttl": "5",
Issue-Key: amplify.create-branch-rejects-a-name-containing-a-space|_error_response#3|AssertionError|raise AssertionError("the call succeeded; real AWS refuses it")
Issue-Key: amplify.create-deployment-rejects-a-branch-that-does-not-exist|run#6|AssertionError|assert err == expected, f"deploying to a missing branch raised {err}, expected {expected}"
Issue-Key: amplify.manual-deployment-job-runs-two-hosting-steps-and-pads-the-job-arn|run#8|AssertionError|assert job["summary"]["jobArn"].endswith("/branches/main/jobs/0000000001"), (
Issue-Key: amplify.start-deployment-rejects-a-job-whose-zip-was-never-uploaded|_client_error#5|AssertionError|raise AssertionError(f"expected a ClientError, call succeeded: {result!r}")
Issue-Key: amplify|GetJob|NotFoundException
Issue-Commit: fec48c0e4a4ef614c94c9aeb73915e609cba5726

# amplify control plane: 8 net-new root causes — enum-refusal value order, CreateBranch defaults/displayName/name charset, CreateDeployment not-found class, unpadded job ARNs, and reserved deployments invisible to GetJob

> _Note: this issue was identified against the code base at commit `fec48c0e4a4ef614c94c9aeb73915e609cba5726`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBranch

## Description
Eight net-new root causes in the amplify service's control plane, each grounded in a failing behavioral check. They are independent defects and must each be closed before this issue is submitted.

Cause 1 of 8: the enum-constraint refusal for an unknown CreateApp platform renders the allowed value set in the model's declaration order instead of AWS's order.
  AWS: Smithy models CreateApp.platform -> enum Platform (smithy_query shape=Platform: values WEB, WEB_DYNAMIC, WEB_COMPUTE). Recorded behavior: CreateApp with platform='STATIC_SITE' is refused with error code ValidationException and the message "1 validation error detected: Value '<platform>' at 'platform' failed to satisfy constraint: Member must satisfy enum value set: [WEB_COMPUTE, WEB_DYNAMIC, WEB]". Fixed text: everything except <platform>, which is the rejected value copied from the request; the member name 'platform' is the member that failed; the bracketed list is a fixed part of this message and AWS renders it WEB_COMPUTE, WEB_DYNAMIC, WEB.
  Emulator: same code and same sentence, but the bracketed set is rendered '[WEB, WEB_DYNAMIC, WEB_COMPUTE]' — the model's declaration order.
  Why: a client (or a human operator) that parses the allowed-value list out of the refusal message, or that asserts on the refusal text it recorded against AWS, sees a different contract than AWS states; message equality checks fail against a real-AWS baseline.
  Fix: the shared enum-constraint refusal renderer this service's validation feeds (the framework refusal wording reachable through EmulatedService::override_refusal) must emit the value order AWS uses for Platform rather than the model's member order.
  Traces: amplify.create-app-rejects-an-unknown-platform.

Cause 2 of 8: a branch created without `stage` gets stage 'DEVELOPMENT' where AWS reports 'NONE'.
  AWS: Smithy models Branch$stage as @required targeting enum Stage (PRODUCTION, BETA, DEVELOPMENT, EXPERIMENTAL, PULL_REQUEST) with no @default. Recorded behavior only: CreateBranch with no `stage` produces a branch whose GetBranch reports stage 'NONE' — a value outside the modeled enum, so the model says nothing about this default and the recording is the contract.
  Emulator: reports stage 'DEVELOPMENT' (see Traces for the file/line).
  Why: a client that branches on stage to tell a configured branch from an unconfigured one is told every new branch is a DEVELOPMENT branch; promotion/stage-gating logic and any assertion on the default read the wrong value.
  Fix: the CreateBranch handler's default for `stage`, and the Branch projection that reports it.
  Traces: amplify.create-branch-applies-defaults-and-derives-a-hyphenated-display-name (the defaults assertion failed with stage 'DEVELOPMENT'); services/service-com-amazonaws-amplify-amplify/src/ops/create_branch.rs:258 and the DEFAULT_STAGE constant at :313.

Cause 3 of 8: `displayName` is echoed from the branch name instead of being derived as a hostname label, so a branch name containing '/' yields a displayName that is not a legal domain prefix.
  AWS: Smithy documents Branch$displayName as "The display name for the branch. This is used as the default domain prefix." Recorded behavior: CreateBranch(branchName='feature/a') with no displayName produces displayName 'feature-a' — the punctuation that cannot appear in a hostname label is replaced by '-'; a name with no such character (e.g. 'main') is unchanged.
  Emulator: displayName is set to the branch name verbatim, so 'feature/a' stays 'feature/a'.
  Why: the displayName IS the branch site's domain prefix, so an un-derived value produces a default domain that cannot be addressed; a client that builds the branch URL from displayName gets a URL with a '/' inside the hostname label.
  Fix: the CreateBranch handler's derivation of `displayName` when the request omits it.
  Traces: amplify.create-branch-applies-defaults-and-derives-a-hyphenated-display-name — the check asserts displayName 'feature-a' for branchName 'feature/a'; that assertion was not reached at replay because Cause 2's assertion fails first, so this cause is code-grounded at services/service-com-amazonaws-amplify-amplify/src/ops/create_branch.rs:249 and must be fixed for the check to pass.

Cause 4 of 8: CreateBranch applies no character-set validation to `branchName`, so a name AWS refuses is accepted with 200.
  AWS: Smithy models CreateBranch.branchName @required, @length 1..255, pattern ^(?s).+$ and BadRequestException (HTTP 400) among its errors. Recorded behavior: a branch name containing a space is refused with HTTP 400, code BadRequestException and the message "The requested branch name is not supported. Failed to create branch. The branch name <branchName> is invalid. The name can only contain letters, numbers, and the symbols /_.-" — all fixed text except <branchName>, which is the name copied from the request. A name using only letters, numbers and /_.- (the check's control, 'rel/v1_0.x-y') is accepted, and the refused name never appears in ListBranches.
  Emulator: the call succeeds (the check reports "the call succeeded; real AWS refuses it"); the handler validates only duplicate names and the per-app branch quota.
  Why: a client is told an unusable branch exists. The name becomes the branch's domain prefix, so the emulator stores branches that can never be addressed, and a client relying on AWS's refusal to validate user input never sees it.
  Fix: the CreateBranch handler's validation, before the branch is built or stored — refuse a branchName containing any character outside letters, digits and /_.- with 400 BadRequestException and AWS's message.
  Traces: amplify.create-branch-rejects-a-name-containing-a-space; services/service-com-amazonaws-amplify-amplify/src/ops/create_branch.rs:219-243 (the only pre-store checks).

Cause 5 of 8: CreateDeployment on a branch the app does not have is refused with the wrong error class — BadRequestException (400) instead of NotFoundException (404).
  AWS: the recorded refusal is code NotFoundException with the message "Branch <branchName> not found." (fixed text apart from <branchName> from the request). Note the model's error list for CreateDeployment (smithy_query operation=CreateDeployment) does NOT include NotFoundException — it lists BadRequestException, InternalFailureException, LimitExceededException, UnauthorizedException — so this is recorded behavior only and the recording overrides the model's list.
  Emulator: answers code BadRequestException with the identical message, by deliberate choice.
  Why: a client branches on the error CODE, not the message: a not-found branch is retried/created on 404 but treated as a malformed request on 400. botocore's generated NotFoundException handler never fires, so error-class-based control flow (and every `except NotFoundException` path) breaks.
  Fix: the CreateDeployment handler's not-found refusal — answer the modeled NotFoundException (404) for an absent app/branch, keeping the message.
  Traces: amplify.create-deployment-rejects-a-branch-that-does-not-exist (got ('BadRequestException','Branch main not found.'), expected ('NotFoundException','Branch main not found.')); services/service-com-amazonaws-amplify-amplify/src/ops/create_deployment.rs:275-293.

Cause 6 of 8: a job's ARN (and the branch's activeJobId) carry the bare job number where AWS zero-pads it to ten digits.
  AWS: Smithy models JobSummary$jobId (pattern ^[0-9]+$) and JobSummary$jobArn (plain string) and Branch$activeJobId. Recorded behavior only: for the same job, `jobId` is the bare number ('1') in CreateDeployment, ListJobs and GetJob, while the ARN's last path segment is the number zero-padded to ten digits — arn:aws:amplify:<region>:<account>:apps/<appId>/branches/<branch>/jobs/0000000001 — and Branch$activeJobId is that same padded form ('0000000001'); totalNumberOfJobs stays '0' for a manual deployment.
  Emulator: the ARN ends '/jobs/1' (unpadded), so the two representations are identical.
  Why: a client that derives the job ARN for IAM resource matching, tagging or log correlation produces an ARN AWS would never emit, and a client that strips the padding from activeJobId to compare it with a jobId (the check does exactly that: activeJobId.lstrip('0')) cannot tell the padded field from the bare one.
  Fix: the job-record ARN construction and the Branch projection's activeJobId in this crate's state/job projection — pad the job number to ten digits in the ARN and in activeJobId, and leave jobId bare.
  Traces: amplify.manual-deployment-job-runs-two-hosting-steps-and-pads-the-job-arn (jobArn '.../branches/main/jobs/1' is not the zero-padded form); the activeJobId assertion in the same check follows it.

Cause 7 of 8: CreateDeployment reserves a job id but records no job, so the reserved deployment is invisible to GetJob (and so cannot be polled or stopped).
  AWS: Smithy models GetJob -> GET /apps/{appId}/branches/{branchName}/jobs/{jobId} with NotFoundException (404) among its errors. Recorded behavior: the jobId CreateDeployment returns is immediately readable — GetJob reports that job with status 'PENDING' and jobType 'MANUAL' before StartDeployment is ever called, and StopJob on it moves it PENDING -> CANCELLED.
  Emulator: GetJob for the jobId CreateDeployment just returned is refused with 404 NotFoundException "Job <jobId> not found." The handler stores only a PendingDeployment reservation and documents that "no job exists under it until that start call".
  Why: the documented manual-deployment flow is CreateDeployment -> upload -> StartDeployment, and a client that polls or cancels the reserved job between those steps gets a not-found for an id the service just handed it; a teardown that cancels outstanding deployments cannot find them.
  Fix: the CreateDeployment handler and this crate's job store — commit a PENDING MANUAL job record under the reserved id at reservation time, which GetJob/ListJobs/StopJob then read.
  Traces: amplify.stop-job-cancels-a-pending-deployment-and-list-jobs-orders-newest-first (NotFoundException "Job 2 not found." from GetJob on the id CreateDeployment returned); services/service-com-amazonaws-amplify-amplify/src/ops/create_deployment.rs:196-199 and :228-243.

Cause 8 of 8: StartDeployment is accepted with 200 for a reserved deployment whose content was never uploaded.
  AWS: Smithy models StartDeployment -> POST /apps/{appId}/branches/{branchName}/deployments/start with BadRequestException (HTTP 400) among its errors. Recorded behavior: calling StartDeployment for a jobId whose zip was never PUT to the returned zipUploadUrl is refused with 400 BadRequestException and the message "Deployment file has not been uploaded, please make sure you uploaded the deployment file prior to invoke this api." — entirely fixed text, carrying nothing from the request or state — and the reserved job stays PENDING. After the zip is PUT to that same URL, the identical call succeeds and the job reaches SUCCEED.
  Emulator: answers HTTP 200 with a JobSummary (status 'PENDING', jobType 'MANUAL'), starting a deployment with no content behind it.
  Why: the upload step silently becomes optional — a client whose upload failed is told the deployment started and then waits on a job that can never publish anything, instead of being told to upload first.
  Fix: the StartDeployment handler's precondition on the reservation's uploaded content — refuse with 400 BadRequestException and AWS's fixed message when no content is present for the jobId, leaving the job PENDING.
  Note on an existing issue: this is the SAME root cause as Cause 5 of the open issue 20261029T215038Z-amplify-job-and-deployment-starts-that-aws-refuses.md, which was filed from the wire corpus and carries only wire replay keys. It is restated here so this behavioral check id is carried as a replay key; a fixer closing that issue's Cause 5 closes this one, and should re-replay the check named below.
  Traces: amplify.start-deployment-rejects-a-job-whose-zip-was-never-uploaded.

## Evidence
Cause 1 (amplify.create-app-rejects-an-unknown-platform): expected ('ValidationException', "1 validation error detected: Value 'STATIC_SITE' at 'platform' failed to satisfy constraint: Member must satisfy enum value set: [WEB_COMPUTE, WEB_DYNAMIC, WEB]"). Actual: ('ValidationException', "1 validation error detected: Value 'STATIC_SITE' at 'platform' failed to satisfy constraint: Member must satisfy enum value set: [WEB, WEB_DYNAMIC, WEB_COMPUTE]"). Only the bracketed value order diverges; 'STATIC_SITE' is the request's value filling the template.

Cause 2 (amplify.create-branch-applies-defaults-and-derives-a-hyphenated-display-name): expected defaults {'stage': 'NONE', 'displayName': 'main', 'enableAutoBuild': True, 'ttl': '5', 'enableBasicAuth': False, 'enablePerformanceMode': False, 'enablePullRequestPreview': False, 'totalNumberOfJobs': '0'}. Actual: identical except 'stage': 'DEVELOPMENT'.

Cause 3 (same check, next assertion): for CreateBranch(branchName='feature/a', stage='PRODUCTION', framework='React', description='probe branch', enableAutoBuild=False) the expected branch carries displayName 'feature-a'. The emulator sets display_name = branch_name (create_branch.rs:249), i.e. 'feature/a'.

Cause 4 (amplify.create-branch-rejects-a-name-containing-a-space): CreateBranch(branchName='release candidate') is expected to raise code BadRequestException with message "The requested branch name is not supported. Failed to create branch. The branch name release candidate is invalid. The name can only contain letters, numbers, and the symbols /_.-" ('release candidate' is the request's value; the rest is fixed). Actual: the call succeeded (AssertionError "the call succeeded; real AWS refuses it") and the branch was created.

Cause 5 (amplify.create-deployment-rejects-a-branch-that-does-not-exist): expected ('NotFoundException', 'Branch main not found.') with HTTP 404. Actual: ('BadRequestException', 'Branch main not found.') with HTTP 400. 'main' is the request's branchName.

Cause 6 (amplify.manual-deployment-job-runs-two-hosting-steps-and-pads-the-job-arn): expected jobArn ending '/branches/main/jobs/0000000001' and branch.activeJobId '0000000001' while jobId/ListJobs report '1'. Actual jobArn 'arn:aws:amplify:us-west-2:000000000000:apps/<appId>/branches/main/jobs/1' (the region/account/appId are per-run values; the requirement is the ten-digit padding of the last segment).

Cause 7 (amplify.stop-job-cancels-a-pending-deployment-and-list-jobs-orders-newest-first): CreateDeployment returned jobId '2'; GetJob(appId, 'main', '2') is expected to report status 'PENDING'. Actual: botocore NotFoundException "An error occurred (NotFoundException) when calling the GetJob operation: Job 2 not found." ('2' is the id CreateDeployment handed back).

Cause 8 (amplify.start-deployment-rejects-a-job-whose-zip-was-never-uploaded): expected ('BadRequestException', 'Deployment file has not been uploaded, please make sure you uploaded the deployment file prior to invoke this api.') with HTTP 400 and the job left PENDING. Actual: HTTP 200 with jobSummary {'jobId': '1', 'status': 'PENDING', 'jobType': 'MANUAL', ...}.

## Steps to Reproduce
Cause 1:
1. amplify.create_app(name=<unique>, platform='STATIC_SITE') -> observe the ValidationException message's bracketed value set; expect '[WEB_COMPUTE, WEB_DYNAMIC, WEB]'.
2. amplify.list_apps -> no app with that name was created (this part already holds).

Causes 2 and 3:
1. app = amplify.create_app(name=<unique>)['app'].
2. amplify.create_branch(appId=app['appId'], branchName='main'); amplify.get_branch(...)['branch'] -> expect stage 'NONE' (observed 'DEVELOPMENT').
3. amplify.create_branch(appId=app['appId'], branchName='feature/a', stage='PRODUCTION', framework='React', description='probe branch', enableAutoBuild=False); amplify.get_branch(appId, 'feature/a')['branch'] -> expect displayName 'feature-a' (observed 'feature/a').

Cause 4:
1. app = amplify.create_app(name=<unique>)['app'].
2. amplify.create_branch(appId=app['appId'], branchName='release candidate') -> expect 400 BadRequestException with the message above; observed: 200 and a created branch.
3. amplify.create_branch(appId=app['appId'], branchName='rel/v1_0.x-y') -> must still succeed; amplify.list_branches(appId) must then list only 'rel/v1_0.x-y'.

Cause 5:
1. app = amplify.create_app(name=<unique>)['app'] (no branches).
2. amplify.create_deployment(appId=app['appId'], branchName='main') -> expect code NotFoundException (HTTP 404), message 'Branch main not found.'; observed code BadRequestException (HTTP 400).
3. amplify.create_branch(appId, 'main'); amplify.create_deployment(appId, 'main') -> jobId must be '1' (the refused call consumes no job number).

Cause 6:
1. Create an app and branch 'main'; run a manual deployment (create_deployment -> PUT the zip to zipUploadUrl -> start_deployment) and poll get_job to SUCCEED.
2. job['summary']['jobArn'] must end '/branches/main/jobs/0000000001' and get_branch(...)['branch']['activeJobId'] must be '0000000001', while job['summary']['jobId'] and list_jobs report '1'.

Cause 7:
1. Create an app and branch 'main'; complete one manual deployment (job '1').
2. dep = amplify.create_deployment(appId, 'main') -> jobId '2'.
3. amplify.get_job(appId, 'main', dep['jobId']) -> expect status 'PENDING'; observed 404 NotFoundException 'Job 2 not found.'
4. amplify.stop_job(appId, 'main', dep['jobId']) -> expect the job to reach CANCELLED and list_jobs to report [('2','CANCELLED'), ('1','SUCCEED')] newest first, with activeJobId still on job 1.

Cause 8:
1. Create an app and branch 'main'; dep = amplify.create_deployment(appId, 'main').
2. WITHOUT uploading anything to dep['zipUploadUrl'], call amplify.start_deployment(appId, 'main', jobId=dep['jobId']) -> expect 400 BadRequestException 'Deployment file has not been uploaded, please make sure you uploaded the deployment file prior to invoke this api.' and get_job still 'PENDING'; observed 200.
3. PUT a zip to dep['zipUploadUrl'] (200) and repeat the identical start_deployment -> the job must reach SUCCEED.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplify.manual-deploy-build-and-serve-roundtrip amplify.stopjob-completed-job-rejected amplify.startjob-retry-missing-jobid-rejected
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify.manual-deploy-build-and-serve-roundtrip|_http_put#14|HTTPError|with urllib.request.urlopen(req, timeout=60) as resp:
Issue-Key: amplify.stopjob-completed-job-rejected|_http_put#8|HTTPError|with urllib.request.urlopen(req, timeout=60) as resp:
Issue-Key: amplify.startjob-retry-missing-jobid-rejected|run#7|AssertionError|assert err == (
Issue-Commit: 600bb9247975905af81e2aa2cda8a015f9396423

# Amplify manual-deployment data plane: the zipUploadUrl handed back by CreateDeployment answers 404 to the upload PUT, and StartJob(RETRY) on a repository-less app is refused with the wrong message

> _Note: this issue was identified against the code base at commit `600bb9247975905af81e2aa2cda8a015f9396423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Two net-new root causes in the amplify service, both grounded in failing behavioral checks.

Cause 1 of 2: the upload destination CreateDeployment returns does not accept the deployment upload (404).
  AWS: Smithy (services/service-com-amazonaws-amplify-amplify/smithy-model/com-amazonaws-amplify-amplify.json) models CreateDeployment -> POST /apps/{appId}/branches/{branchName}/deployments, output member `zipUploadUrl` (UploadUrl, @required, @length 0..1000) documented as "When the fileMap argument is not provided in the request, this zipUploadUrl is returned", and the operation doc states the manual-deploy contract: the caller uploads the zipped site to that URL and then calls StartDeployment with the returned `jobId`. On real AWS the URL is a working presigned S3 PUT target: the `PUT <zipUploadUrl>` with the zip body answers 2xx (200, or 204), and the subsequent StartDeployment builds and serves that content. Recorded behavior: the behavioral checks PUT the zip to exactly the URL the emulator returned and require status 200 or 204.
  Emulator: the PUT to the returned `zipUploadUrl` answers HTTP 404 Not Found (`urllib.error.HTTPError: HTTP Error 404: Not Found` raised from the checks' `_http_put`), so no upload ever lands and the whole manual-deployment flow stops at the first step. The URL is minted by `create_deployment.rs` as `https://aws-amplify-{region}-deployment.s3.{region}.amazonaws.com/{appId}/{jobId}/{token}` (see Traces for the file/line), but nothing in the emulator serves or backs that location: no such bucket/object route exists, so object storage answers 404 for the key the reservation handed out. Hypothesis: the deployment bucket named by that URL template is never created, so the S3 side answers NoSuchBucket/NoSuchKey (404) for the presigned PUT.
  Why: a client that follows the documented CreateDeployment -> upload -> StartDeployment sequence cannot deploy anything. The upload fails with a bare 404 instead of succeeding, so every manual deployment, and every subsequent job/StopJob/GetJob behavior that depends on a started deployment, is unreachable — the control plane accepts CreateDeployment/StartDeployment while the data plane it advertises does not exist.
  Fix: the CreateDeployment handler's upload-destination minting plus the object-storage route that must back it (the upload URL must point at a destination this emulator actually serves, and a PUT there must store the bytes that StartDeployment's `Source::Reserved` read then consumes). Belongs in the operation's handler/state, not in the checks.
  Traces: amplify.manual-deploy-build-and-serve-roundtrip (fails at the presigned PUT, before the job-status poll and the site GET it also asserts), amplify.stopjob-completed-job-rejected (fails at the same presigned PUT while setting up an in-flight job). Code citations: services/service-com-amazonaws-amplify-amplify/src/ops/create_deployment.rs:212-222 and :302-307 (upload_url template); services/service-com-amazonaws-amplify-amplify/src/ops/start_deployment.rs:505 (`Source::Reserved` read path that expects the uploaded content).

Cause 2 of 2: StartJob does not refuse jobs on an app with no repository provider; it reports a jobId-validation message instead.
  AWS: Smithy models StartJob -> POST /apps/{appId}/branches/{branchName}/jobs with `jobType` @required (enum MANUAL | RELEASE | RETRY | WEB_HOOK) and `jobId` documented as "The unique ID for an existing job. This is required if the value of jobType is RETRY"; modeled errors include BadRequestException (HTTP 400). Recorded behavior: on an app created WITHOUT a repository, StartJob(jobType=RETRY) with no jobId is refused with HTTP 400 `BadRequestException` and the message "Operation not supported for app that was not connected to a repository provider. Please use CreateDeployment and StartDeployment operations instead." — i.e. AWS evaluates the repository-connectedness of the app BEFORE the jobId requirement. That message is entirely fixed text; it carries no value from the request or state.
  Emulator: refuses with HTTP 400 `BadRequestException` but the message "jobId is required when jobType is RETRY." (fixed text with the jobType interpolated — see start_job.rs:174-182), because the handler checks the jobId/RETRY rule first and never checks whether the app is repository-connected at all.
  Why: a client that branches on the refusal text (or that reads it to decide whether to switch to CreateDeployment/StartDeployment, which is precisely what AWS's message instructs) is told the wrong thing: it believes supplying a jobId would make the call work, when on a repository-less app StartJob is not supported for ANY jobType/jobId combination. Conversely a repository-less app that DOES supply a valid existing jobId would be accepted by the emulator where AWS refuses it.
  Fix: the StartJob handler's validation ordering and precondition set — reject StartJob on an app with no repository provider with 400 BadRequestException and AWS's fixed message, ahead of the RETRY/jobId check.
  Traces: amplify.startjob-retry-missing-jobid-rejected (assertion shows the returned pair ('BadRequestException', 'jobId is required when jobType is RETRY.') against the expected ('BadRequestException', 'Operation not supported for app that was not connected to a repository provider. Please use CreateDeployment and StartDeployment operations instead.')). Code citation: services/service-com-amazonaws-amplify-amplify/src/ops/start_job.rs:174-182.

Related but NOT covering these: open issue 20261029T122518Z-amplify-s3-amplify-hosting-edge-never-applies-an-app-s.md (hosting-edge customRules rewrites, owns amplify__s3.rewrite-rules-...) and 20261023T144746Z-amplify-untagresource-stub.md (UntagResource stub). Neither carries the two causes above.

## Evidence
Cause 1 — expected: PUT to the `zipUploadUrl` returned by CreateDeployment answers 200 or 204 and stores the zip; StartDeployment then builds it and the branch's default domain serves the content. Actual: `urllib.error.HTTPError: HTTP Error 404: Not Found` on the PUT to the emulator-returned upload URL (routed into the emulator via ctx.endpoint_route), in both amplify.manual-deploy-build-and-serve-roundtrip and amplify.stopjob-completed-job-rejected. The URL shape returned is https://aws-amplify-<region>-deployment.s3.<region>.amazonaws.com/<appId>/<jobId>/<token> (create_deployment.rs:302-307); the host/region/appId/jobId/token values differ per run and are not part of the requirement — the requirement is that a PUT to the returned URL is accepted.

Cause 2 — expected (AWS, recorded): HTTP 400, Code `BadRequestException`, Message `Operation not supported for app that was not connected to a repository provider. Please use CreateDeployment and StartDeployment operations instead.` (all fixed text). Actual (emulator): HTTP 400, Code `BadRequestException`, Message `jobId is required when jobType is RETRY.` — fixed text `jobId is required when jobType is ` with the request's jobType value interpolated. Assertion from the suite: AssertionError: ('BadRequestException', 'jobId is required when jobType is RETRY.')

## Steps to Reproduce
Cause 1:
1. amplify.create_app(name=<unique>) -> appId; amplify.create_branch(appId, branchName=<unique>).
2. r = amplify.create_deployment(appId=appId, branchName=branchName) -> note r['jobId'], r['zipUploadUrl'].
3. HTTP PUT a small zip (containing index.html) to r['zipUploadUrl'].
   Observed: 404 Not Found. Expected: 200 or 204.
4. (Blocked by step 3) amplify.start_deployment(appId, branchName, jobId=r['jobId']), poll get_job until terminal -> expect SUCCEED with non-empty steps, and GET https://<branchName>.<app.defaultDomain>/ -> expect the deployed index.html body.

Cause 2:
1. amplify.create_app(name=<unique>) with NO repository -> appId; amplify.create_branch(appId, branchName=<unique>).
2. amplify.start_job(appId=appId, branchName=branchName, jobType='RETRY')  (no jobId).
   Observed: 400 BadRequestException 'jobId is required when jobType is RETRY.'
   Expected: 400 BadRequestException 'Operation not supported for app that was not connected to a repository provider. Please use CreateDeployment and StartDeployment operations instead.'
3. amplify.list_jobs(appId, branchName) -> must still be [] (no job history created by the refused call).

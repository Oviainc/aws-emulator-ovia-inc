Issue-Source: trace-sweep-divergence
Issue-Trace: amplify-s3-prefix-deployment-job-succeeds
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amplify-s3-prefix-deployment-job-succeeds amplify-environment-variables-replace-semantics amplify-job-start-and-manage
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify|GetJob|2xx->2xx|$.job.steps[*]
Issue-Key: amplify|GetJob|2xx->2xx|$.job.steps[*].artifactsUrl
Issue-Key: amplify|GetJob|2xx->2xx|$.job.steps[*].logUrl
Issue-Key: amplify|GetBranch|2xx->2xx|$.branch.activeJobId
Issue-Key: amplify|GetBranch|2xx->2xx|$.branch.totalNumberOfJobs
Issue-Key: amplify|CreateApp|2xx->2xx|$.app.defaultDomain
Issue-Related: 20261105T200342Z-amplify-job-lifecycle-and-projection-reserved-jobs-report.md
Issue-Commit: a86c3493c3f63ce00344d068ccb26649087a2439

# amplify: 3 net-new causes — GetJob reports a one-step job with no step URLs, GetBranch mis-reports activeJobId/totalNumberOfJobs, CreateApp publishes an unreachable defaultDomain

> _Note: this issue was identified against the code base at commit `a86c3493c3f63ce00344d068ccb26649087a2439`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJob

## Description
Three net-new root causes in the amplify service, each grounded in its own replayed wire-fixture step and each clustered separately by the replay. They share one fault: the emulator's projection of a FINISHED deployment and of the app/branch that owns it reports values AWS does not report and omits the ones it does. They are independent defects and all three must be closed before this issue is submitted.

Cause 1 of 3: a manual/S3 deployment job that has reached SUCCEED is reported with a single execution step and no step artifact or log URLs, where AWS reports the full step sequence (a deploy step carrying artifactsUrl and logUrl, followed by a VERIFY step).
  AWS: Smithy (services/service-com-amazonaws-amplify-amplify/smithy-model/com-amazonaws-amplify-amplify.json) models GetJob -> GET /apps/{appId}/branches/{branchName}/jobs/{jobId}, 200, output `job` @required -> Job{summary @required, steps @required (list of Step)}. Step models stepName/startTime/status/endTime @required plus optional logUrl, artifactsUrl, testArtifactsUrl, testConfigUrl, screenshots (map), statusReason, context. The model does not fix the step sequence, so that part is recorded behavior only: a SUCCEED deployment job is reported with MORE THAN ONE step — the first step carries BOTH `artifactsUrl` and `logUrl` (URLs the client can fetch for the deployed artifact and the step log), and it is followed by a step with `stepName` 'VERIFY', `status` 'SUCCEED', its own startTime/endTime, an `artifactsUrl`, no `logUrl`, and a `screenshots` map with one entry per verification device. The requirement is the presence and shape of those members and of the VERIFY step; the URL hosts/paths, timestamps and screenshot URLs are per-environment values and are never the requirement (they are recorded as shape-only assertions).
  Emulator: GetJob on a settled job reports exactly one step and no URLs at all. services/service-com-amazonaws-amplify-amplify/src/state.rs:771-780 (`BranchJob::settle`) replaces `job.steps` with `vec![Step { step_name: self.conclusion.step_name, start_time, status, end_time, status_reason, .. }]` — one step named by `JobConclusion` (DEPLOY for a successful deployment, state.rs:837-844), with logUrl, artifactsUrl, testArtifactsUrl, testConfigUrl and screenshots all unset; StartDeployment inserts the job with `steps: Vec::new()` (src/ops/start_deployment.rs:322). So `$.job.steps[0].artifactsUrl` and `$.job.steps[0].logUrl` are absent and `$.job.steps[1]` (the VERIFY step) does not exist.
  Why: the step list is how a client reports deployment progress and fetches per-step output — a console or CI integration links the build/deploy log from `steps[*].logUrl`, downloads the artifact from `artifactsUrl`, and treats the VERIFY step (with its screenshots map) as the post-deploy verification result. Against the emulator every one of those lookups finds nothing: a client that iterates steps to collect logs/artifacts gets an empty-handed success, and one that asserts a VERIFY step ran concludes the deployment was never verified.
  Fix: the job's step projection in this crate's job state — the `settle`/`JobConclusion` path in src/state.rs that builds the step list must emit the whole step sequence AWS reports for a settled deployment (the deploy step with artifactsUrl+logUrl, then the VERIFY step with its artifactsUrl and screenshots map), with every URL minted on the endpoint the client reached the emulator on rather than left unset.
  Note on existing issues: this cluster ALSO carries the already-known JobSummary defects (commitId/commitMessage reported as '' instead of absent, commitTime/startTime as wall-clock timestamps, jobType 'MANUAL', and `jobArn` ending '/jobs/1' instead of the ten-digit-padded '/jobs/0000000001'); those are the root causes of Cause 7 of the open issue 20261102T092113Z-amplify-branch-and-job-projection-wrong-stage-default.md and of 20261105T200342Z-amplify-job-lifecycle-and-projection-reserved-jobs-report.md, but NEITHER carries this step's replay key, so this step is re-replayed here. The step-list gap itself is carried by no open issue.
  Traces: amplify-s3-prefix-deployment-job-succeeds#task#7.

Cause 2 of 3: GetBranch reports `activeJobId` as the bare job number and `totalNumberOfJobs` as a running count of started jobs, where AWS reports the active job id zero-padded to ten digits and reports totalNumberOfJobs as '0' for a branch deployed this way.
  AWS: Smithy models GetBranch -> GET /apps/{appId}/branches/{branchName}, 200, output `branch` @required -> Branch, whose `activeJobId` (@required, @length 0..1000, "The ID of the active job for a branch of an Amplify app") and `totalNumberOfJobs` (@required, @length 0..1000, "The total number of jobs that are part of an Amplify app") are both STRINGS with no pattern, while JobSummary$jobId is @pattern ^[0-9]+$. The model does not fix their formatting, so this is recorded behavior only: after a deployment job on the branch has succeeded, GetBranch reports `activeJobId` as the job number zero-padded to ten digits — the same padded form AWS uses in the job ARN segment (.../jobs/0000000001) while `JobSummary$jobId` stays the bare number — and reports `totalNumberOfJobs` '0'. The ten-digit padding is the rule; the digits themselves come from the branch's own job sequence.
  Emulator: src/ops/start_deployment.rs:338-339 writes `branch.branch.active_job_id = summary.job_id.clone()` (the bare id, e.g. '1') and `branch.branch.total_number_of_jobs = branch.jobs_started().to_string()` (e.g. '1'), and src/ops/get_branch.rs:171 reports that record through `BranchProjection::reported`. Replay: `$.branch.activeJobId` expected '0000000001', got '1'; `$.branch.totalNumberOfJobs` expected '0', got '1'.
  Why: `activeJobId` is what a client feeds straight back into GetJob/StopJob and what it string-compares against the id embedded in a job ARN; an unpadded value never matches a padded ARN segment, so correlation of a branch with its active job (and any IAM/log matching built from the ARN form) fails, and clients that cache the padded form from AWS see a different id from the emulator. `totalNumberOfJobs` is read as the branch's job counter for display and reconciliation, so a client is told a count AWS does not report for this branch kind.
  Fix: the Branch projection and the branch job-counter writes — `active_job_id` must be minted in the same ten-digit-padded form the job ARN uses (one shared formatting helper for both, so they cannot drift), and `total_number_of_jobs` must report what AWS reports for a branch whose jobs are deployments rather than the emulator's own started-job tally (src/ops/start_deployment.rs around lines 338-339, the `jobs_started` counter documented in src/state.rs:317-324, and the shared Branch projection GetBranch calls).
  Traces: amplify-s3-prefix-deployment-job-succeeds#task#8.

Cause 3 of 3: every app projection publishes `App.defaultDomain` as a `<appId>.amplifyapp.com` host that is not one of the emulator's own hosts, so the address CreateApp/GetApp hand the client cannot be reached.
  AWS: Smithy models CreateApp -> POST /apps, 200, output `app` @required -> App; `App$defaultDomain` is documented "The default domain for the Amplify app." On AWS that domain is the host Amplify's hosting edge actually answers on, so `https://<branch displayName>.<defaultDomain>/` is a live address that returns the deployed page, the branch's basic-auth challenge, or a configured redirect. The requirement is that the member name an address the client that received it can reach; the concrete host and port are environment-specific and are never the requirement (the fixture asserts shape only).
  Emulator: src/ops/create_app.rs sets `default_domain: format!("{app_id}.amplifyapp.com")`, so CreateApp (and every other app reader) announces a hostname in AWS's production amplifyapp.com zone. The replay rejects it as a host the emulator does not own: `$.app.defaultDomain: host_not_own (got '<appId>.amplifyapp.com')`.
  Why: a client takes `defaultDomain` as the app's live hosting address — it is how an Amplify consumer fetches the deployed site, follows a rewrite/redirect rule, or is challenged by branch basic auth. Against the emulator that address resolves off the emulator entirely, so every hosting-dependent flow either fails to connect or talks to something that is not the emulator.
  Fix: the app record's `defaultDomain` must be minted from the emulator's own published hosting address instead of a synthesized AWS zone (src/ops/create_app.rs at the `default_domain` assignment plus the shared app projection that reports it), and the emulator's hosting edge must answer on that announced host.
  Note on existing issues: the open issue 20261109T143214Z-amplify-2-net-new-causes-app-defaultdomain-is.md describes this same defaultDomain fault but is grounded in the BEHAVIORAL corpus and carries only behavioral check ids — it holds no wire replay key, and the replay report did not mark this cluster as owned by it, so the two wire steps are filed here so they are re-replayed with the fix.
  Traces: amplify-environment-variables-replace-semantics#task#1, and amplify-job-start-and-manage#setup#1 in the same cluster.

## Evidence
Cause 1 — amplify-s3-prefix-deployment-job-succeeds#task#7, GetJob, HTTP 200 both sides:
  $.job.steps[0].artifactsUrl: expected PRESENT (shape-asserted URL), got absent.
  $.job.steps[0].logUrl: expected PRESENT (shape-asserted URL), got absent.
  $.job.steps[1]: expected PRESENT — {'stepName': 'VERIFY', 'status': 'SUCCEED', 'startTime': <set>, 'endTime': <set>, 'artifactsUrl': <set>, 'logUrl': None, 'screenshots': {one entry per verification device}, 'context': None, 'statusReason': None, 'testArtifactsUrl': None, 'testConfigUrl': None} — got absent (the emulator's steps list has length 1).
  Same step also: $.job.summary.commitId expected ABSENT / got ''; commitMessage expected ABSENT / got ''; commitTime expected ABSENT / got a wall-clock timestamp; jobType expected ABSENT / got 'MANUAL'; jobArn expected '.../branches/main/jobs/0000000001' (ten-digit padded) / got '.../branches/main/jobs/1'. (appId, region, account, timestamps and URL values are per-run; the requirements are the presence of the step members, the second VERIFY step, the absent summary members and the ARN padding.)
  Source: services/service-com-amazonaws-amplify-amplify/src/state.rs:771-780 (`settle` builds `vec![Step { .. }]`, one step, no logUrl/artifactsUrl/screenshots); src/ops/start_deployment.rs:322 (`steps: Vec::new()`).

Cause 2 — amplify-s3-prefix-deployment-job-succeeds#task#8, GetBranch, HTTP 200 both sides:
  $.branch.activeJobId: expected '0000000001' (the active job number zero-padded to ten digits), got '1'.
  $.branch.totalNumberOfJobs: expected '0', got '1'.
  Source: src/ops/start_deployment.rs:338-339 — `branch.branch.active_job_id = summary.job_id.clone(); branch.branch.total_number_of_jobs = branch.jobs_started().to_string();`, reported by src/ops/get_branch.rs:171 via `BranchProjection::reported`.

Cause 3 — amplify-environment-variables-replace-semantics#task#1, CreateApp, HTTP 200 both sides (cluster also covers amplify-job-start-and-manage#setup#1):
  $.app.defaultDomain: host_not_own — got 'd2v7o19j0e8a65.amplifyapp.com', a host on AWS's production amplifyapp.com zone that this emulator does not serve; expected a host the emulator itself answers on (asserted by shape, not by value — the appId label and the port are per-run values).
  Source: src/ops/create_app.rs — `default_domain: format!("{app_id}.amplifyapp.com")`.

## Steps to Reproduce
Cause 1 (GetJob step list):
1. app = amplify.create_app(name=<unique>); amplify.create_branch(appId, branchName='main').
2. amplify.start_deployment(appId, 'main', sourceUrl='s3://<bucket>/<prefix>/', sourceUrlType='BUCKET_PREFIX') (or the CreateDeployment + upload + StartDeployment flow).
3. Poll amplify.get_job(appId, 'main', jobId) until summary.status == 'SUCCEED'.
   Expected: job.steps has MORE THAN ONE element — steps[0] carries both artifactsUrl and logUrl, and a later step has stepName 'VERIFY', status 'SUCCEED', an artifactsUrl and a screenshots map.
   Observed: job.steps has exactly one element, with no artifactsUrl, no logUrl and no screenshots; no VERIFY step.

Cause 2 (GetBranch activeJobId / totalNumberOfJobs):
1. Continue from Cause 1 with the job settled SUCCEED.
2. amplify.get_branch(appId=appId, branchName='main').
   Expected: branch.activeJobId is the active job number zero-padded to ten digits (e.g. '0000000001'), matching the padded job segment of the job ARN; branch.totalNumberOfJobs is '0'.
   Observed: activeJobId '1' (unpadded), totalNumberOfJobs '1'.

Cause 3 (defaultDomain unreachable):
1. r = amplify.create_app(name=<unique>).
2. Read r['app']['defaultDomain'].
   Expected: a host the emulator itself serves, so that a GET of https://<branch displayName>.<defaultDomain>/ after a successful deployment is answered by the emulator that published it.
   Observed: '<appId>.amplifyapp.com' — AWS's production zone, which the emulator does not serve (the replay refuses it as not one of the emulator's own hosts).

Replay: re-run the fixture steps named in Issue-Trace/Issue-Step and Issue-Trace-Ids.

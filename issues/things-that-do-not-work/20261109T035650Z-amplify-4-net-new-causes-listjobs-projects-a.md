Issue-Source: trace-sweep-divergence
Issue-Trace: amplify-afs-traj-manual-deploy-job-sequencing
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amplify-afs-traj-manual-deploy-job-sequencing amplify-afs-traj-s3-source-deploy-mistakes amplify-untag-resource-removes-specific-key
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify|ListJobs|2xx->2xx|$.jobSummaries[*]
Issue-Key: amplify|StartDeployment|4xx->2xx|$.message
Issue-Key: amplify|StartDeployment|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: amplify|StartDeployment|4xx->2xx|status
Issue-Key: amplify|UntagResource|2xx->2xx|$headers.content-length
Issue-Related: 20261105T200342Z-amplify-job-lifecycle-and-projection-reserved-jobs-report.md
Issue-Commit: 4267a221d5187b7bd9ef5a3892bc2e0384d6116a

# amplify: 4 net-new causes — ListJobs projects a single job, StartDeployment skips the s3 sourceUrl rule, UntagResource omits content-length

> _Note: this issue was identified against the code base at commit `4267a221d5187b7bd9ef5a3892bc2e0384d6116a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListJobs

## Description
Four net-new root causes in the amplify crate's job-listing projection, StartDeployment input validation, and the no-content response serializer. All four are amplify; none is owned by an open issue. A fixer must close ALL FOUR before submitting.

--- Cause 1 of 4: ListJobs returns only one job summary, and the one it returns is the wrong job with an unpadded jobArn (fixture amplify-afs-traj-manual-deploy-job-sequencing, step task#10) ---
AWS: Smithy: ListJobs GET /apps/{appId}/branches/{branchName}/jobs, output ListJobsResult.jobSummaries (@required list of JobSummary), paginated {items: jobSummaries, pageSize: maxResults, range 0..50}. AWS lists EVERY job that exists on the branch — jobs reserved by CreateDeployment included — newest job first, each as its own JobSummary with its own jobId/jobArn/status. Recorded behavior for the ARN form: the jobArn's trailing job segment is the jobId zero-padded to ten digits (arn:aws:amplify:<region>:<account>:apps/<appId>/branches/<branchName>/jobs/<jobId padded to 10>), while JobSummary.jobId itself is the unpadded decimal string (Smithy: JobId @pattern ^[0-9]+$). In the recorded step three jobs existed, so AWS answered three summaries, newest first: jobId "3" status PENDING, jobId "2" status PENDING, jobId "1" status CANCELLED.
Emulator: answers 200 with exactly ONE element in jobSummaries. $.jobSummaries[1] and $.jobSummaries[2] are missing entirely, and the single element it does emit is the oldest job rather than the newest: $.jobSummaries[0].jobId is "1" where "3" is required, $.jobSummaries[0].status is "CANCELLING" where "PENDING" is required, and $.jobSummaries[0].jobArn ends ".../jobs/1" where ".../jobs/0000000003" is required — so the ARN's job segment is never zero-padded to ten digits. The same element also serializes members AWS leaves absent (commitId and commitMessage as "", commitTime and startTime as a clock timestamp, jobType as "MANUAL"); that absent-member serialization is the fault already open as 20261102T092113Z-amplify-branch-and-job-projection-wrong-stage-default.md, and this cause is the net-new part: the list holds one job instead of all of them, in the wrong order, with an unpadded ARN.
Why: a client polling ListJobs to follow a branch's deploy history sees exactly one job forever. It cannot discover the job CreateDeployment just reserved, cannot tell a cancelled job from a pending one, and any client that parses the jobArn (for tagging or for cross-referencing a job by ARN) builds an ARN that does not match the one AWS mints, so an ARN-keyed lookup misses.
Fix: the ListJobs handler's state query and projection — it must enumerate the branch's whole job set, order it newest jobId first, and the shared JobSummary projector must render jobArn's job segment zero-padded to ten digits while leaving jobId unpadded.
Traces: amplify-afs-traj-manual-deploy-job-sequencing#task#10.

--- Cause 2 of 4: ListJobs still projects a single stale job after a fourth job is created (fixture amplify-afs-traj-manual-deploy-job-sequencing, step task#13) ---
AWS: Smithy: ListJobs, output jobSummaries @required list. By this point in the recording four jobs exist on the branch, so AWS answers four summaries newest-first: jobId "4" PENDING, "3" CANCELLED, "2" PENDING, "1" CANCELLED, each with its own zero-padded jobArn.
Emulator: answers 200 with one element — $.jobSummaries[1], [2] and [3] all missing — and that element is still the oldest job: jobId "1" instead of "4", status "CANCELLING" instead of "PENDING", jobArn ".../jobs/1" instead of ".../jobs/0000000004". This is a second, independently recorded occurrence at a different branch state (four jobs, two of them terminal), which confirms the listing is not merely dropping reserved jobs but is not accumulating the job set at all.
Why: the history never grows, so a client cannot observe any job it created after the first one, and cannot page the branch's jobs at all.
Fix: same component as cause 1 — the ListJobs handler's state query and ordering; this trace must be replayed too, because it is the one that shows the defect persists as jobs accumulate and settle.
Traces: amplify-afs-traj-manual-deploy-job-sequencing#task#13.

--- Cause 3 of 4: StartDeployment accepts an s3 sourceUrl with no trailing slash instead of refusing it (fixture amplify-afs-traj-s3-source-deploy-mistakes, step task#10) ---
AWS: Smithy: StartDeployment POST /apps/{appId}/branches/{branchName}/deployments/start; input sourceUrl @length 0..3000 @pattern ^(s3|https|http):// ; sourceUrlType enum ZIP|BUCKET_PREFIX; modeled errors include BadRequestException (client, httpError 400). The @pattern only constrains the scheme; the trailing-slash rule is recorded behavior only — the recording shows AWS refusing an s3:// sourceUrl that is an Amazon S3 bucket-and-prefix location but does not end in '/' with HTTP 400, an x-amzn-errortype header naming the error shape, and the message "The s3 sourceUrl must end with '/'". That message is entirely fixed text: it carries no value taken from the request or from state, so it must be reproduced verbatim, including the single quotes around the slash.
Emulator: answers 200 and starts the deployment. The whole refusal is absent: status 200 where 400 is required, $.message missing where "The s3 sourceUrl must end with '/'" is required, and the x-amzn-errortype response header missing where AWS sends one.
Why: a client that mistypes an S3 prefix gets a success and a job id. It then polls a job that can never fetch its source, instead of being told immediately which input was wrong — and a client whose error handling branches on BadRequestException for this case never takes that branch.
Fix: StartDeployment's input validation — add the s3 sourceUrl trailing-slash check, refusing with BadRequestException / HTTP 400 and the message above, and ensure the error serializer emits x-amzn-errortype for it.
Traces: amplify-afs-traj-s3-source-deploy-mistakes#task#10.

--- Cause 4 of 4: the UntagResource no-content response omits Content-Length (fixture amplify-untag-resource-removes-specific-key, step task#2) ---
AWS: Smithy: UntagResource DELETE /tags/{resourceArn}, output UntagResourceResponse with no members. Recorded behavior: AWS answers an empty body and still sends a Content-Length header on it (the recording asserts the header's presence, not its value, since the value is fixed by the empty body).
Emulator: answers the right status with the body, but $headers.content-length is missing from the response entirely.
Why: an empty response with neither Content-Length nor a chunked encoding is not a self-delimiting HTTP message. A strict HTTP client or a proxy in front of the emulator has no framing for the body and must either block until the connection closes or reject the response, so a successful untag can surface to the caller as a transport error.
Fix: the crate's response serializer for empty-output operations — emit Content-Length on a no-content response instead of omitting the header.
Traces: amplify-untag-resource-removes-specific-key#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 (13 fixtures, 98 steps, 84 passed, score 85.7%, bootstrap_signal false, emulator_up true). Four net-new clusters, all service=amplify, none carrying stub_path or owned_by:

Cause 1 — amplify-afs-traj-manual-deploy-job-sequencing#task#10, ListJobs, verdict mismatch, expected 200 got 200:
  $.jobSummaries[0].jobId: expected '3', got '1'
  $.jobSummaries[0].status: expected 'PENDING', got 'CANCELLING'
  $.jobSummaries[0].jobArn: expected 'arn:aws:amplify:us-west-2:000000000000:apps/<appId>/branches/main/jobs/0000000003', got '.../jobs/1'  (expected job segment zero-padded to 10 digits; emulator emits the raw id)
  $.jobSummaries[1]: missing (expected the jobId '2' / status 'PENDING' summary)
  $.jobSummaries[2]: missing (expected the jobId '1' / status 'CANCELLED' summary)
  $.jobSummaries[0].commitId / .commitMessage: expected absent, got ''
  $.jobSummaries[0].commitTime / .startTime: expected absent, got a clock timestamp
  $.jobSummaries[0].jobType: expected absent, got 'MANUAL'

Cause 2 — amplify-afs-traj-manual-deploy-job-sequencing#task#13, ListJobs, verdict mismatch, expected 200 got 200:
  $.jobSummaries[0].jobId: expected '4', got '1'
  $.jobSummaries[0].status: expected 'PENDING', got 'CANCELLING'
  $.jobSummaries[0].jobArn: expected '.../jobs/0000000004', got '.../jobs/1'
  $.jobSummaries[1]: missing (expected jobId '3', status 'CANCELLED')
  $.jobSummaries[2]: missing (expected jobId '2', status 'PENDING')
  $.jobSummaries[3]: missing (expected jobId '1', status 'CANCELLED')

Cause 3 — amplify-afs-traj-s3-source-deploy-mistakes#task#10, StartDeployment, verdict mismatch, expected 400 got 200:
  status: expected 400, got 200
  $.message: missing (expected "The s3 sourceUrl must end with '/'")
  $headers.x-amzn-errortype: missing (expected present)

Cause 4 — amplify-untag-resource-removes-specific-key#task#2, UntagResource, verdict mismatch, expected 204 got 204:
  $headers.content-length: missing (expected present)

Model evidence: Smithy ListJobs output jobSummaries @required list(JobSummary), paginated{items: jobSummaries, pageSize: maxResults range 0..50}; JobSummary.jobArn @required @length 0..1000, jobId @required @pattern ^[0-9]+$, status @required enum JobStatus [CREATED, PENDING, PROVISIONING, RUNNING, FAILED, SUCCEED, CANCELLING, CANCELLED]. Smithy StartDeployment input sourceUrl @length 0..3000 @pattern ^(s3|https|http)://, sourceUrlType enum [ZIP, BUCKET_PREFIX], errors include BadRequestException httpError 400. Smithy UntagResource DELETE /tags/{resourceArn}, output UntagResourceResponse with no members.

## Steps to Reproduce
Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.

Cause 1: replay amplify-afs-traj-manual-deploy-job-sequencing up to step task#10 — create an app and a branch, reserve three jobs via CreateDeployment, cancel the first via StopJob, then GET /apps/{appId}/branches/main/jobs. Expect three jobSummaries newest-first (jobId '3' PENDING, '2' PENDING, '1' CANCELLED) with jobArns whose job segment is the jobId zero-padded to ten digits; observe a single summary for jobId '1' with jobArn '.../jobs/1'.

Cause 2: continue the same fixture to step task#13 — reserve a fourth job, then GET the same jobs collection. Expect four jobSummaries newest-first; observe one stale summary for jobId '1'.

Cause 3: replay amplify-afs-traj-s3-source-deploy-mistakes to step task#10 — POST /apps/{appId}/branches/main/deployments/start with an s3:// bucket-and-prefix sourceUrl that does NOT end in '/'. Expect HTTP 400 with x-amzn-errortype set and body message "The s3 sourceUrl must end with '/'"; observe HTTP 200 and a started job.

Cause 4: replay amplify-untag-resource-removes-specific-key to step task#2 — DELETE /tags/{resourceArn}?tagKeys=<key>. Inspect the response headers: expect a Content-Length header on the empty body; observe no Content-Length header at all.

Issue-Source: trace-sweep-divergence
Issue-Trace: batch-consumable-resource-pool-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: batch-consumable-resource-pool-lifecycle batch-scheduling-policy-standalone-lifecycle batch-service-environment-standalone-lifecycle batch-fargate-job-runs-to-succeeded
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch|TagResource|2xx->2xx|$headers.content-length
Issue-Key: batch|UntagResource|2xx->2xx|$headers.content-length
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].attempts
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].container.environment
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].container.exitCode
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].container.fargatePlatformConfiguration
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].container.logStreamName
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].container.mountPoints
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].container.networkInterfaces
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].container.secrets
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].container.taskArn
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].container.ulimits
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].container.volumes
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].dependsOn
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].eksAttempts
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].parameters
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].startedAt
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].status
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].statusReason
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].stoppedAt
Issue-Commit: d38d8ab45cd7905c8c6149b13ae22553e33f698d

# batch: 3 net-new causes — tag/untag 204 responses omit Content-Length and a Fargate job never reaches SUCCEEDED

> _Note: this issue was identified against the code base at commit `d38d8ab45cd7905c8c6149b13ae22553e33f698d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Three net-new root causes in the batch emulator, found by replaying the wire fixture corpus (trace set simparity-wire-aws-v1). Causes 1 and 2 are the same framing defect on the two body-less tag operations; cause 3 is the Fargate job data plane. All three must be closed before this issue is submitted.

Cause 1 of 3: TagResource answers 204 with no Content-Length header.
AWS: Batch TagResource is REST-JSON POST /v1/tags/{resourceArn} with an EMPTY output shape (Smithy: TagResource -> output TagResourceResponse has no members; http {method: POST, uri: /v1/tags/{resourceArn}}; errors ClientException 400, ServerException 500). The recorded AWS response for a successful tag of a batch resource ARN is a body-less success that carries an explicit `Content-Length: 0` header (recorded behavior only for the header itself — the model does not bind headers on this operation). The status itself matches and is not at issue.
Emulator: the same request answers the same status but with NO Content-Length header at all — `$headers.content-length: missing (expected '0', got None)`. The response has no declared body length.
Why: a client that derives framing from Content-Length (HTTP/1.1 SDK transports, connection-reusing pools, and intermediate proxies) is handed a body-less response whose length is undeclared; such a client either reads until close, breaks keep-alive reuse, or — for strict transports — treats the response as malformed. A tagging call that AWS makes cheap and reusable becomes a connection-level hazard.
Fix: the REST-JSON response serializer / response-emission layer for empty-output operations in the batch crate — a body-less response must set Content-Length: 0 rather than omitting the header.
Traces: batch-consumable-resource-pool-lifecycle#task#6, batch-scheduling-policy-standalone-lifecycle#task#5, batch-service-environment-standalone-lifecycle#task#5.

Cause 2 of 3: UntagResource answers 204 with no Content-Length header.
AWS: Batch UntagResource is REST-JSON DELETE /v1/tags/{resourceArn} with tagKeys bound to the `tagKeys` query string, and an EMPTY output shape (Smithy: UntagResource -> output UntagResourceResponse has no members; errors ClientException 400, ServerException 500). The recorded AWS response is body-less and carries `Content-Length: 0` (recorded behavior only for the header).
Emulator: `$headers.content-length: missing (expected '0', got None)` — the untag success response declares no body length.
Why: identical to cause 1 — undeclared framing on a body-less response. It is listed separately because it is a second operation with the same empty-output shape, so a fix applied only to the tag path leaves the untag path diverging.
Fix: same component as cause 1 (REST-JSON empty-output response emission in the batch crate); verify it covers DELETE as well as POST.
Traces: batch-consumable-resource-pool-lifecycle#task#8, batch-scheduling-policy-standalone-lifecycle#task#7, batch-service-environment-standalone-lifecycle#task#7.

Cause 3 of 3: a Fargate job submitted to a valid job queue ends FAILED with an emulator-authored reason, and DescribeJobs reports no attempt, no timestamps and none of JobDetail's empty collections or applied defaults.
AWS: a container job submitted with platformCapabilities FARGATE to a healthy job queue progresses through the JobStatus enum (Smithy: JobStatus = SUBMITTED, PENDING, RUNNABLE, STARTING, RUNNING, SUCCEEDED, FAILED) to SUCCEEDED when its container exits 0, and DescribeJobs (POST /v1/describejobs -> DescribeJobsResponse.jobs: JobDetailList) then reports:
  - status SUCCEEDED and statusReason the fixed AWS text `Essential container in task exited` (fixed text; no variable parts),
  - startedAt and stoppedAt (Smithy: JobDetail.startedAt/stoppedAt, Long epoch milliseconds — values are per-run and are NOT part of the requirement, only their presence is),
  - attempts: one AttemptDetail (Smithy: JobDetail.attempts -> AttemptDetails) whose container (AttemptContainerDetail) carries taskArn, exitCode 0, logStreamName and networkInterfaces[].{attachmentId, privateIpv4Address}, plus startedAt, stoppedAt and the same statusReason,
  - the modeled empty collections and applied defaults AWS always materializes on a described job: dependsOn [], parameters {}, container.volumes [], container.environment [], container.mountPoints [], container.ulimits [], container.secrets [], container.exitCode 0, container.taskArn, container.logStreamName, container.networkInterfaces, container.fargatePlatformConfiguration {platformVersion: "LATEST"}, and eksAttempts [].
Emulator: the job settles FAILED instead of SUCCEEDED (the replay's poll gives up: "'FAILED' is in fail_on after 2 attempt(s)"), with statusReason set to the emulator-authored sentence `This emulator does not run containers, so the job's container was never started` — text AWS never emits for any status. attempts, startedAt, stoppedAt, dependsOn, parameters, eksAttempts and every container member listed above are absent from the JobDetail (reported as `missing`), so the described job carries neither an attempt record nor the empty-collection/default members.
Why: every client that waits for a Batch job to succeed (the SDK waiters, CI pipelines, CloudFormation/Step Functions integrations, and any poller written against the documented status ladder) concludes the workload failed, and is handed a failure reason that matches no documented AWS statusReason. Code that reads attempts[0].container.exitCode / taskArn / logStreamName to locate the task or its /aws/batch/job log stream dereferences absent members, and code that iterates container.environment or parameters without a null guard breaks on None where AWS guarantees a list or map.
Why it is filed here: the replay reports this step as owned by no open issue. Open issues 20261106T090000Z-batch-s3-a-placed-job-s-container-is-never-run, 20261107T210820Z-batch-the-job-data-plane-never-executes-a, 20261107T153156Z-batch-logs-a-placed-batch-job-ends-failed and 20261107T153156Z-batch-ecr-lambda-logs-describejobs-reports-no-attempts describe the same family on other fixtures, but none of them carries this fixture/step as its replay key, so this binding is unexercised by any filed fix; close it against this fixture even if those are fixed.
Fix: the SubmitJob/job-lifecycle state machine for FARGATE platform capability (job state advance to SUCCEEDED with a synthesized attempt and timestamps), plus the DescribeJobs serializer, which must emit JobDetail's empty collections and applied defaults instead of omitting them.
Traces: batch-fargate-job-runs-to-succeeded#task#17.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 85.5%, 23 mismatched of 165 steps, emulator up and serving).

Cause 1 — TagResource, expected 204 / actual 204, fixture batch-consumable-resource-pool-lifecycle step task#6:
  $headers.content-length: missing (expected '0', got None)
  also affected: batch-scheduling-policy-standalone-lifecycle#task#5, batch-service-environment-standalone-lifecycle#task#5

Cause 2 — UntagResource, expected 204 / actual 204, fixture batch-consumable-resource-pool-lifecycle step task#8:
  $headers.content-length: missing (expected '0', got None)
  also affected: batch-scheduling-policy-standalone-lifecycle#task#7, batch-service-environment-standalone-lifecycle#task#7

Cause 3 — DescribeJobs, expected 200 / actual 200, fixture batch-fargate-job-runs-to-succeeded step task#17:
  $.jobs[0].status: value_mismatch (expected 'SUCCEEDED', got 'FAILED')
  $.jobs[0].status: poll: 'FAILED' is in fail_on after 2 attempt(s)
  $.jobs[0].statusReason: value_mismatch (expected 'Essential container in task exited', got "This emulator does not run containers, so the job's container was never started")
  $.jobs[0].attempts: missing (expected one AttemptDetail with container.{taskArn, exitCode 0, logStreamName, networkInterfaces[].{attachmentId, privateIpv4Address}}, startedAt, stoppedAt, statusReason 'Essential container in task exited'; got None)
  $.jobs[0].startedAt / $.jobs[0].stoppedAt: missing
  $.jobs[0].dependsOn: missing (expected [])
  $.jobs[0].parameters: missing (expected {})
  $.jobs[0].container.volumes / .environment / .mountPoints / .ulimits / .secrets: missing (expected [])
  $.jobs[0].container.exitCode: missing (expected 0)
  $.jobs[0].container.taskArn / .logStreamName / .networkInterfaces: missing
  $.jobs[0].container.fargatePlatformConfiguration: missing (expected {'platformVersion': 'LATEST'})
  $.jobs[0].eksAttempts: missing (expected [])

Model citations: Smithy TagResource -> TagResourceResponse (no members), http POST /v1/tags/{resourceArn}; UntagResource -> UntagResourceResponse (no members), http DELETE /v1/tags/{resourceArn}; DescribeJobs -> DescribeJobsResponse.jobs: JobDetailList, http POST /v1/describejobs; JobStatus = [SUBMITTED, PENDING, RUNNABLE, STARTING, RUNNING, SUCCEEDED, FAILED]; JobDetail.{attempts: AttemptDetails, statusReason, startedAt, stoppedAt, dependsOn, parameters, container, eksAttempts}; AttemptDetail.{container: AttemptContainerDetail, startedAt, stoppedAt, statusReason}; AttemptContainerDetail.{taskArn, exitCode, logStreamName, networkInterfaces}.

Per-run values (ARNs, task ids, ip addresses, log stream names, timestamps, request ids) are not requirements — only the status, the fixed statusReason text, and the presence/shape of the listed members are.

## Steps to Reproduce
Cause 1: replay fixture batch-consumable-resource-pool-lifecycle step task#6 — create a Batch consumable resource, then call TagResource (POST /v1/tags/{resourceArn}) on its ARN with one tag; inspect the response headers. AWS returns the body-less success with `Content-Length: 0`; the emulator sends no Content-Length. Same with batch-scheduling-policy-standalone-lifecycle#task#5 and batch-service-environment-standalone-lifecycle#task#5.

Cause 2: replay fixture batch-consumable-resource-pool-lifecycle step task#8 — call UntagResource (DELETE /v1/tags/{resourceArn}?tagKeys=<key>) on the same resource and inspect the response headers; Content-Length: 0 is absent. Same with batch-scheduling-policy-standalone-lifecycle#task#7 and batch-service-environment-standalone-lifecycle#task#7.

Cause 3: replay fixture batch-fargate-job-runs-to-succeeded through step task#17 — register a FARGATE container job definition, create a FARGATE compute environment and job queue, SubmitJob, then poll DescribeJobs on the returned jobId. AWS reaches status SUCCEEDED with statusReason 'Essential container in task exited', one attempts[] entry and the full JobDetail/ContainerDetail member set; the emulator settles FAILED with the reason "This emulator does not run containers, so the job's container was never started" and omits attempts, startedAt, stoppedAt, dependsOn, parameters, eksAttempts and the container collections/defaults.

Issue-Source: trace-sweep-divergence
Issue-Trace: emr-containers-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-containers-error-paths
Issue-Shape-Id: com.amazonaws.emrcontainers#AwsChicagoWebService
Issue-Key: emr-containers|CreateVirtualCluster|4xx->2xx|$.message
Issue-Key: emr-containers|CreateVirtualCluster|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: emr-containers|CreateVirtualCluster|4xx->2xx|status
Issue-Key: emr-containers|DescribeJobRun|4xx->4xx|$.message
Issue-Key: emr-containers|DescribeJobRun|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3

# emr-containers error paths: missing input validation on nonexistent and malformed identifiers

> _Note: this issue was identified against the code base at commit `ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVirtualCluster

## Description
Two causes, both the same underlying fault: the emr-containers emulator performs no pre-lookup validation of the identifiers a request carries, so it either accepts a request AWS refuses or refuses it under the wrong error class. This issue is NOT closed until both causes below are fixed and both named fixture steps replay clean.

Cause 1 of 2 — CreateVirtualCluster accepts a container provider whose EKS cluster does not exist (fixture emr-containers-error-paths, step task#2).
AWS: Smithy: CreateVirtualCluster (POST /virtualclusters) takes a required `containerProvider` (ContainerProvider) and models ValidationException and ResourceNotFoundException, both httpError 400. The recording shows AWS validating the referenced EKS cluster before creating anything: HTTP 400 with header x-amzn-errortype: ValidationException and a message naming the missing cluster (recorded example: "Cluster sim2realprobenonexistentcluster00 not found"). The requirement is the status (400) and the error code (ValidationException); the message text is an example, though it is built from the request's cluster id.
Emulator: answers HTTP 200 with a CreateVirtualClusterResponse — no x-amzn-errortype header and no message at all — i.e. it registers a virtual cluster over an EKS cluster that was never created.
Why: a client that provisions EMR-on-EKS gets a success and a virtual cluster id for a container provider that does not exist, so its error handling for a mistyped or deleted EKS cluster never runs and every later job submission against that id fails for an unexplained reason. Negative-path tests that assert ValidationException see a 200.
Fix: CreateVirtualCluster's handler/validation in the emr-containers service crate — resolve `containerProvider.id` against the EKS-cluster state before inserting the virtual cluster, and return the modeled ValidationException (400) when it does not resolve.
Traces: emr-containers-error-paths#task#2.

Cause 2 of 2 — DescribeJobRun classifies a malformed job-run id as ResourceNotFoundException instead of ValidationException (fixture emr-containers-error-paths, step task#4).
AWS: Smithy: DescribeJobRun (GET /virtualclusters/{virtualClusterId}/jobruns/{id}); `id` targets ResourceIdString with @length 1..64 and @pattern ^[0-9a-z]+$, and the operation models both ValidationException and ResourceNotFoundException at httpError 400. The probe id satisfies the modeled constraints but is not a well-formed job-run short id, and the recording shows AWS rejecting it at validation: HTTP 400 with x-amzn-errortype: ValidationException (recorded message example: "Invalid job run short id"), i.e. id-format validation runs before any state lookup. Status and error code are the requirement here.
Emulator: answers HTTP 400 but with x-amzn-errortype: ResourceNotFoundException (message example: "Job run sim2realprobenonexistentresource00 was not found in virtual cluster sim2realprobenonexistentcluster00"), i.e. it skips format validation and reports the result of a failed lookup.
Why: the two errors mean different things to a caller — ValidationException is a non-retryable client bug in the id, ResourceNotFoundException invites poll/retry-until-visible logic. SDK error-type dispatch (catching ValidationException) does not fire, and a caller cannot distinguish "you sent garbage" from "the job run has aged out".
Fix: DescribeJobRun's input validation in the emr-containers service crate — validate the job-run short-id format before the state lookup and raise the modeled ValidationException (400) on a malformed id, leaving ResourceNotFoundException for well-formed ids that do not resolve.
Traces: emr-containers-error-paths#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200, data listener accepting; state_reset_failures 0):

Cause 1 — emr-containers-error-paths#task#2, CreateVirtualCluster:
  expected: status 400, $headers.x-amzn-errortype = 'ValidationException', $.message = 'Cluster sim2realprobenonexistentcluster00 not found'
  actual:   status 200, x-amzn-errortype absent, $.message absent (None)
  divergence_key: service=emr-containers|CreateVirtualCluster|status=400->200|keys=$.message,$headers.x-amzn-errortype,status

Cause 2 — emr-containers-error-paths#task#4, DescribeJobRun:
  expected: status 400, $headers.x-amzn-errortype = 'ValidationException', $.message = 'Invalid job run short id'
  actual:   status 400, $headers.x-amzn-errortype = 'ResourceNotFoundException', $.message = 'Job run sim2realprobenonexistentresource00 was not found in virtual cluster sim2realprobenonexistentcluster00'
  divergence_key: service=emr-containers|DescribeJobRun|status=400->400|keys=$.message,$headers.x-amzn-errortype

Model evidence (services/service-com-amazonaws-emrcontainers-awschicagowebservice/smithy-model/com-amazonaws-emrcontainers-awschicagowebservice.json): CreateVirtualCluster errors = {EKSRequestThrottledException 429, InternalServerException 500, ResourceNotFoundException 400, ValidationException 400}; DescribeJobRun errors = {InternalServerException 500, ResourceNotFoundException 400, ValidationException 400}, input id/virtualClusterId = ResourceIdString @length 1..64 @pattern ^[0-9a-z]+$.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay fixture emr-containers-error-paths against it (trace set simparity-wire-aws-v1).
3. Step task#2: call CreateVirtualCluster with a containerProvider whose EKS cluster id was never created (e.g. id 'sim2realprobenonexistentcluster00', type EKS). Expected HTTP 400 with x-amzn-errortype: ValidationException; observed HTTP 200 with a created virtual cluster.
4. Step task#4: call GET /virtualclusters/{virtualClusterId}/jobruns/{id} (DescribeJobRun) with a malformed job-run short id (e.g. 'sim2realprobenonexistentresource00'). Expected HTTP 400 with x-amzn-errortype: ValidationException; observed HTTP 400 with x-amzn-errortype: ResourceNotFoundException.
5. Re-replay both emr-containers-error-paths#task#2 and emr-containers-error-paths#task#4 to confirm a fix; both must pass.

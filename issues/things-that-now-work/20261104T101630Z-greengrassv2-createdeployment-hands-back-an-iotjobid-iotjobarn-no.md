Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: greengrassv2.create-deployment-active-state-matches-iot-job
Issue-Shape-Id: com.amazonaws.greengrassv2#GreengrassV2
Issue-Key: greengrassv2|DescribeJob|NotImplemented
Issue-Commit: e2a7b75abf70ab47f92f0d33522cc824ac8688ce

# greengrassv2 CreateDeployment hands back an iotJobId/iotJobArn no IoT job backs: iot DescribeJob answers 501 NotImplemented

> _Note: this issue was identified against the code base at commit `e2a7b75abf70ab47f92f0d33522cc824ac8688ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
One net-new root cause, grounded in the single surviving failing behavioral check of this suite run (every other greengrassv2 failure was marked ALREADY FILED or OWNED by an open issue and was skipped).

Cause 1 of 1: a GreengrassV2 deployment's IoT job identity is not resolvable in the emulator's IoT jobs plane.

AWS: GreengrassV2 CreateDeployment (Smithy: POST /greengrass/v2/deployments, success 201; CreateDeploymentResponse.deploymentId NonEmptyString, .iotJobId NonEmptyString, .iotJobArn IoTJobARN pattern ^arn:[^:]*:iot:[^:]+:[0-9]+:job/.+$) creates a real AWS IoT job for the deployment and returns that job's id and ARN. Those identifiers address an existing job in the IoT jobs plane: iot DescribeJob (Smithy: GET /jobs/{jobId}, success 200; DescribeJobResponse.job of shape Job; errors InvalidRequestException 400, ResourceNotFoundException 404, ThrottlingException 400, ServiceUnavailableException 503) answers 200 for that jobId, with job.jobArn equal to the iotJobArn CreateDeployment returned and job.status one of QUEUED or IN_PROGRESS while the deployment is ACTIVE (GetDeployment deploymentStatus=ACTIVE). The check records exactly this linkage against real AWS; the model states the shapes and the ARN pattern, the recording states the status pair, so neither side of the join is emulator-invented.

Emulator: the greengrassv2 side of the flow answers — CreateComponentVersion settles DEPLOYABLE, CreateDeployment returns deploymentId plus a non-empty iotJobId and iotJobArn, and GetDeployment reports the requested targetArn with deploymentStatus=ACTIVE — but the job id it mints addresses nothing. Calling iot DescribeJob with that jobId is refused with HTTP 501, error code `NotImplemented`, message `operation DescribeJob is scaffolded but not implemented in this emulator`; its handler is the untouched scaffold stub (see Traces). Fixed text of that refusal is the whole message, with only the operation name `DescribeJob` substituted — none of it is AWS wording for any modeled DescribeJob error, and 501/NotImplemented is not a modeled status or code for this operation at all. Consequently the deployment's job identity is not merely unverifiable: no IoT job object is created by CreateDeployment for a client to read, cancel or wait on.

Why: a client that drives Greengrass deployments the way AWS does — take iotJobId/iotJobArn from CreateDeployment, then track rollout through the IoT jobs plane (DescribeJob / job executions) — gets a 501 with a non-AWS error code where AWS gives it a QUEUED/IN_PROGRESS job whose jobArn matches. Error handling keyed on ResourceNotFoundException/InvalidRequestException never matches, retry logic treats a 501 as a server fault and loops, and nothing in the emulator ever shows a deployment's job progressing, so deployment-orchestration code cannot be exercised end to end. The ids returned look well-formed, so the divergence surfaces only on the follow-up call.

Fix: iot's DescribeJob handler (services/service-com-amazonaws-iot-awsiotservice/src/ops/describe_job.rs, whose handle() returns crate::scaffolded::Scaffolded::not_implemented("DescribeJob")) must serve the modeled DescribeJobResponse from IoT jobs state, and the greengrassv2 CreateDeployment handler/state must register the job it mints into that shared jobs state (with jobArn equal to the iotJobArn it reports and an initial QUEUED/IN_PROGRESS status) so the two planes join. A jobId that exists in no state must then be refused 404 ResourceNotFoundException, not 501.

Note on existing issues: no open issue names iot DescribeJob — the open iot bootstrap issue (20261102T000217Z-bootstrap-service-com-amazonaws-iot-awsiotservice.md) names UpdateThing, and the open greengrassv2 issues (…-7-net-new-unimplemented-operations-answer-501.md, …-createdeployment-accepts-nonexistent-targets-and-never-supersedes.md) cover greengrassv2's own stubs and target/supersede handling, not the deployment-to-IoT-job linkage. This check is reported by the suite under the `greengrassv2` service heading, which is why it is filed here even though the blocking handler lives in the iot crate; the service token has not been relabelled.

## Evidence
Failing check: greengrassv2.create-deployment-active-state-matches-iot-job [error]

Expected (real AWS, as recorded): CreateDeployment -> 201 with deploymentId, iotJobId, iotJobArn; GetDeployment -> 200 deploymentStatus=ACTIVE, targetArn echoed; iot DescribeJob(jobId=<that iotJobId>) -> 200 with job.jobArn == iotJobArn and job.status in (QUEUED, IN_PROGRESS).

Actual (emulator): the greengrassv2 calls succeeded, then
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeJob operation: operation DescribeJob is scaffolded but not implemented in this emulator
raised at the check's `iot.describe_job(jobId=created["iotJobId"])` step. HTTP status 501, error code `NotImplemented`; AWS models only 400 InvalidRequestException / 404 ResourceNotFoundException / 400 ThrottlingException / 503 ServiceUnavailableException for DescribeJob, and 200 for the resolvable job this flow creates.

Handler confirming the cause: services/service-com-amazonaws-iot-awsiotservice/src/ops/describe_job.rs lines 3-6 (`//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented`) and line 160 (`Err(crate::scaffolded::Scaffolded::not_implemented("DescribeJob"))`).

## Steps to Reproduce
1. Create an IoT thing group and note its thingGroupArn.
2. greengrassv2 CreateComponentVersion with an inline recipe; poll DescribeComponent until componentState=DEPLOYABLE.
3. greengrassv2 CreateDeployment(targetArn=<thingGroupArn>, deploymentName=..., components={<name>: {componentVersion: "1.0.0"}}) and keep deploymentId, iotJobId, iotJobArn from the 201 response.
4. greengrassv2 GetDeployment(deploymentId=...) -> observe deploymentStatus=ACTIVE (passes).
5. iot DescribeJob(jobId=<iotJobId from step 3>) -> emulator answers 501 NotImplemented instead of 200 with job.jobArn == iotJobArn and job.status in (QUEUED, IN_PROGRESS).

Replay: behavioral check greengrassv2.create-deployment-active-state-matches-iot-job

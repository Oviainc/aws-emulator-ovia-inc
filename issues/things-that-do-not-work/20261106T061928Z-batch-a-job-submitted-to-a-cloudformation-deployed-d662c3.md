Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.deploy-job-queue-via-cloudformation-accepts-and-runs-job
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch.deploy-job-queue-via-cloudformation-accepts-and-runs-job|_describe_job#32|Timeout|return batch.describe_jobs(jobs=[job_id])["jobs"][0]
Issue-Commit: 014c70e1db9a03f0ba98b313ccfd7f14084970e5

# batch: a job submitted to a CloudFormation-deployed job queue backed by a MANAGED EC2 compute environment is never dispatched — it never appears in GetJobQueueSnapshot and never leaves its initial status

> _Note: this issue was identified against the code base at commit `014c70e1db9a03f0ba98b313ccfd7f14084970e5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
Cause 1 of 1 — a container job submitted to a CloudFormation-deployed AWS::Batch::JobQueue whose AWS::Batch::ComputeEnvironment is MANAGED/EC2 is accepted but never scheduled: the emulator never lists the outstanding job in the queue's front-of-queue snapshot and never advances the job's status, so the scenario produces no verdict at all within its budget. Failing check: batch.deploy-job-queue-via-cloudformation-accepts-and-runs-job.

AWS: Smithy com.amazonaws.batch#SubmitJob (POST /v1/submitjob, success 200, modeled errors ClientException 400 / ServerException 500) answers 200 {jobArn, jobName, jobId} and Batch then advances the job entirely server-side, with no further client call: SUBMITTED -> PENDING -> RUNNABLE -> STARTING -> RUNNING -> SUCCEEDED|FAILED. While the job waits for capacity, GetJobQueueSnapshot (POST /v1/getjobqueuesnapshot, 200, output frontOfQueue: FrontOfQueueDetail) reports it in frontOfQueue.jobs keyed by its jobArn, and DescribeJobs (POST /v1/describejobs, 200, output jobs: JobDetailList) reports the advancing status. This holds identically for a queue and compute environment created by CloudFormation: once the stack reports CREATE_COMPLETE the declared AWS::Batch::JobQueue is VALID/ENABLED and associated with the declared MANAGED EC2 AWS::Batch::ComputeEnvironment, so a job submitted to it is placed and runs.

Emulator: no wait in the scenario is ever satisfied and the replay carries only "Check timed out after 60.0s" — the emulator returned no refusal and no error, it simply never produced the next observable state. The three waits that can stall, in order, are: DescribeJobQueues(jobQueues=[<stack JobQueueArn>])[0].status == "VALID" after CREATE_COMPLETE; the submitted jobArn appearing in GetJobQueueSnapshot(jobQueue=<stack queue>).frontOfQueue.jobs; and DescribeJobs(jobs=[jobId]).jobs[0].status reaching RUNNING/SUCCEEDED/FAILED. Hypothesis: the batch job placement/state-machine loop never advances a job placed on a MANAGED compute environment and never projects queued jobs into the GetJobQueueSnapshot frontOfQueue view; the CloudFormation-declared path may additionally leave the queue and compute environment unregistered or unassociated in batch state, so the job is never a placement candidate. Supporting evidence from the same replay: every batch data-plane scenario that waits on a job status or a snapshot entry also exhausts its budget (batch__logs 0/10, batch__s3 0/3, batch__ecs__logs 0/2).

Why: this is the normal deployment shape for Batch — queue and compute environment by template, work submitted by API. A client gets a jobId for work that is neither scheduled nor failed: DescribeJobs polls forever with no status transition and no statusReason, and GetJobQueueSnapshot reports an empty queue while a job is outstanding. Every downstream Batch contract (exit codes, attempt timeouts, retries, dependsOn gating, vCPU capacity caps) is unreachable on a CloudFormation-deployed estate.

Fix: the job scheduler/placement loop and job state machine in the batch crate (the component that advances a submitted job's status and publishes queued jobs into the GetJobQueueSnapshot frontOfQueue projection), together with the AWS::Batch::JobQueue and AWS::Batch::ComputeEnvironment resource providers in the cloudformation crate, which must register the queue and the compute environment — and their association and VALID/ENABLED state — in batch state before those resources are reported CREATE_COMPLETE. SubmitJob's own handler already answers 200 and is not the defect.

Related already-open issues that the replay report did NOT mark as owning this check: 20261105T005839Z-batch-a-job-submitted-to-a-cloudformation-deployed.md describes this same scenario but was keyed to a different recorded failure frame (an in-check _poll_until Timeout) and so does not carry this replay key; 20261104T041055Z-batch-a-cloudformation-stack-that-reaches-create-complete.md is the sibling CFN check, which fails earlier with 400 ClientException because the stack's AWS::Batch::JobDefinition is never registered; 20261104T041055Z-batch-array-jobs-unimplemented-managed-ces-have-no.md covers the never-settling job plane for API-created estates. If a fixer closes this one, it should re-replay the check named below rather than assume one of those covers it.

Traces: batch.deploy-job-queue-via-cloudformation-accepts-and-runs-job.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout, check batch.deploy-job-queue-via-cloudformation-accepts-and-runs-job (verdict: mismatch):

  Error: Check timed out after 60.0s

What the check does: creates a stack declaring AWS::IAM::Role (batch service role, AWSBatchServiceRole), AWS::IAM::Role + AWS::IAM::InstanceProfile (ECS instance role), AWS::Batch::ComputeEnvironment (Type MANAGED, ComputeResources Type EC2, MinvCpus 0, MaxvCpus 2, InstanceTypes [optimal], Subnets/SecurityGroupIds from the shared network) and AWS::Batch::JobQueue (Priority 1, ComputeEnvironmentOrder -> that CE), with ComputeEnvironmentArn/JobQueueArn Outputs; waits for a terminal stack status and asserts CREATE_COMPLETE; waits for DescribeJobQueues(jobQueues=[JobQueueArn])[0].status == "VALID" (120s); registers a container job definition (image public.ecr.aws/docker/library/busybox:1.36, command ["echo", ...], resourceRequirements VCPU 1 / MEMORY 512); SubmitJob to the stack queue; then
  (a) polls GetJobQueueSnapshot(jobQueue=<stack queue arn>) until the submitted jobArn is in frontOfQueue.jobs (90s budget), and
  (b) polls DescribeJobs(jobs=[jobId]) until status is in (RUNNING, SUCCEEDED, FAILED) (600s budget), asserting status != FAILED.

Expected (real AWS): both waits are satisfied — the job is listed in frontOfQueue.jobs while queued, and DescribeJobs reports RUNNING and then SUCCEEDED.
Actual (emulator): no wait is ever satisfied; the scenario is killed by the harness budget. The emulator produced neither a frontOfQueue entry for the outstanding job nor any status transition after SubmitJob's 200, and returned no error code or statusReason to explain it.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. aws cloudformation create-stack --stack-name s --capabilities CAPABILITY_IAM --template-body <template declaring a batch service role, an ECS instance role + instance profile, AWS::Batch::ComputeEnvironment (Type MANAGED, ComputeResources Type EC2, MinvCpus 0, MaxvCpus 2, InstanceTypes [optimal], Subnets/SecurityGroupIds from the shared VPC) and AWS::Batch::JobQueue (Priority 1, ComputeEnvironmentOrder -> that CE), with Outputs ComputeEnvironmentArn and JobQueueArn>.
3. Wait for CREATE_COMPLETE and read the JobQueueArn output.
4. aws batch describe-job-queues --job-queues <JobQueueArn> -> expect status VALID, state ENABLED; aws batch describe-compute-environments --compute-environments <ComputeEnvironmentArn> -> expect status VALID with a backing ecsClusterArn.
5. aws batch register-job-definition --job-definition-name jd --type container --container-properties '{"image":"public.ecr.aws/docker/library/busybox:1.36","command":["echo","hi"],"resourceRequirements":[{"type":"VCPU","value":"1"},{"type":"MEMORY","value":"512"}]}'
6. aws batch submit-job --job-name j --job-queue <JobQueueArn> --job-definition jd -> 200 with a jobId.
7. aws batch get-job-queue-snapshot --job-queue <JobQueueArn> -> on AWS the outstanding job's jobArn is in frontOfQueue.jobs; on the emulator it never appears.
8. aws batch describe-jobs --jobs <jobId> repeatedly -> on AWS the status advances to RUNNING and then SUCCEEDED; on the emulator it never reaches RUNNING, SUCCEEDED or FAILED.
9. Re-replay batch.deploy-job-queue-via-cloudformation-accepts-and-runs-job.

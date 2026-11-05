Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.deploy-job-queue-via-cloudformation-accepts-and-runs-job
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch.deploy-job-queue-via-cloudformation-accepts-and-runs-job|_poll_until#32|Timeout|return last
Issue-Commit: 0a8b28aea83ccc3190789fc779439d6e199d02f9

# batch: a job submitted to a CloudFormation-deployed job queue backed by a MANAGED EC2 compute environment never appears in GetJobQueueSnapshot and never reaches RUNNING/SUCCEEDED (1 cause)

> _Note: this issue was identified against the code base at commit `0a8b28aea83ccc3190789fc779439d6e199d02f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
Cause 1 of 1 — a container job submitted to a CloudFormation-deployed AWS::Batch::JobQueue whose AWS::Batch::ComputeEnvironment is MANAGED/EC2 is accepted but never dispatched: it is never listed in the queue's front-of-queue snapshot and never leaves its initial status, so the check exhausts its budget with no verdict.

AWS: Smithy com.amazonaws.batch#SubmitJob (POST /v1/submitjob, success 200, modeled errors ClientException 400 / ServerException 500) returns {jobArn, jobName, jobId} and Batch then advances the job server-side with no further client calls: SUBMITTED -> PENDING -> RUNNABLE -> STARTING -> RUNNING -> SUCCEEDED|FAILED. While the job is waiting for capacity it is reported by GetJobQueueSnapshot (POST /v1/getjobqueuesnapshot, 200) in frontOfQueue.jobs with its jobArn, and DescribeJobs (POST /v1/describejobs, 200) reports the advancing status. This holds for a queue and compute environment created by CloudFormation exactly as for ones created through the Batch API: once the stack reports CREATE_COMPLETE the queue is VALID/ENABLED and backed by the declared MANAGED EC2 compute environment, so a job submitted to it runs.

Emulator: the scenario never produces a verdict — the only failure detail the replay carries is "Check timed out after 60.0s", i.e. one of the check's waits never becomes true within the harness budget. The waits that can stall are, in order: queue status VALID after CREATE_COMPLETE, the submitted jobArn appearing in GetJobQueueSnapshot().frontOfQueue.jobs, and DescribeJobs().jobs[0].status reaching RUNNING/SUCCEEDED/FAILED. A previous replay of this same check recorded the stall inside the check's _poll_until helper with a Timeout, and the emulator's own Batch job plane is known not to advance submitted jobs (the queue/CE are created and SubmitJob answers 200, but no status transition follows). Hypothesis: the job state machine/placement loop never advances a job placed on a MANAGED compute environment, so neither the snapshot entry nor a RUNNING/terminal status ever appears; the CloudFormation-declared path may additionally leave the queue or CE unassociated so the job is never a candidate for placement.

Why: this is the normal deployment shape for Batch — queue and compute environment by template, work submitted by API. A client gets a jobId for work that is never scheduled and never fails: DescribeJobs polls forever, GetJobQueueSnapshot shows an empty queue while a job is outstanding, and no statusReason explains it. Every downstream contract (exit codes, timeouts, retries, dependsOn, capacity caps) is unreachable on a CFN-deployed estate.

Fix: the job scheduler/placement loop in the batch crate — the state machine that advances a submitted job's status and publishes queued jobs into the GetJobQueueSnapshot frontOfQueue projection — together with the AWS::Batch::JobQueue / AWS::Batch::ComputeEnvironment resource providers in the cloudformation crate, which must register the queue and compute environment (and their association and VALID/ENABLED state) in batch state before the resources are reported CREATE_COMPLETE. SubmitJob's handler itself already answers 200 and is not the defect.

Related open issues that do NOT carry this check's replay key and therefore do not cover it: 20261104T041055Z-batch-array-jobs-unimplemented-managed-ces-have-no.md (Cause 3 describes the same never-settling job plane, but for API-created estates and with a different recorded failure frame), and 20261104T041055Z-batch-a-cloudformation-stack-that-reaches-create-complete.md (the sibling CFN check, where the stack's AWS::Batch::JobDefinition is never registered and SubmitJob refuses it with 400 ClientException). This check reaches submission successfully and then stalls, so it is a distinct observation that must be re-replayed on its own.

Traces: batch.deploy-job-queue-via-cloudformation-accepts-and-runs-job.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout, check batch.deploy-job-queue-via-cloudformation-accepts-and-runs-job (verdict: mismatch):

  Error: Check timed out after 60.0s

The check: creates a stack declaring AWS::IAM::Role (batch service role), AWS::IAM::Role + AWS::IAM::InstanceProfile (ECS instance role), AWS::Batch::ComputeEnvironment (Type MANAGED, ComputeResources Type EC2, MinvCpus 0, MaxvCpus 2, InstanceTypes [optimal], shared-network Subnets/SecurityGroupIds) and AWS::Batch::JobQueue (Priority 1, ComputeEnvironmentOrder -> the CE), with ComputeEnvironmentArn/JobQueueArn Outputs; waits for CREATE_COMPLETE; waits for DescribeJobQueues(jobQueues=[JobQueueArn])[0].status == VALID; registers a container job definition (busybox, command echo, VCPU 1 / MEMORY 512); SubmitJob to the stack queue; then
  (a) polls GetJobQueueSnapshot(jobQueue=<stack queue arn>) until the submitted jobArn is in frontOfQueue.jobs (90s budget), and
  (b) polls DescribeJobs(jobs=[jobId]) until status in (RUNNING, SUCCEEDED, FAILED) (600s budget), asserting status != FAILED.

Expected (real AWS): both waits are satisfied — the job is listed in frontOfQueue while queued and DescribeJobs reports RUNNING and then SUCCEEDED.
Actual (emulator): no wait is ever satisfied; the check is killed by the harness budget, so the emulator produced neither a snapshot entry for the outstanding job nor any status transition after SubmitJob's 200.

Supporting context from the same replay: every batch data-plane check that waits on a job status or a snapshot entry also exhausts its budget (0/10 in batch__logs, 0/3 in batch__s3, 0/2 in batch__ecs__logs), and the sibling CFN check batch.deploy-job-queue-via-cloudformation-processes-job fails earlier with 400 ClientException "JobDefinition name <name> does not exist or is not in ACTIVE status." — i.e. CFN-declared Batch resources are not materialised in batch state.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. aws cloudformation create-stack --stack-name s --capabilities CAPABILITY_IAM --template-body <template declaring a batch service role, an ECS instance role + instance profile, AWS::Batch::ComputeEnvironment (Type MANAGED, ComputeResources Type EC2, MinvCpus 0, MaxvCpus 2, InstanceTypes [optimal], Subnets/SecurityGroupIds from the shared VPC) and AWS::Batch::JobQueue (Priority 1, ComputeEnvironmentOrder -> that CE), with Outputs ComputeEnvironmentArn and JobQueueArn>.
3. Wait for CREATE_COMPLETE and read the JobQueueArn output.
4. aws batch describe-job-queues --job-queues <JobQueueArn>  -> expect status VALID, state ENABLED; aws batch describe-compute-environments --compute-environments <ComputeEnvironmentArn> -> expect status VALID with a backing ecsClusterArn.
5. aws batch register-job-definition --job-definition-name jd --type container --container-properties '{"image":"public.ecr.aws/docker/library/busybox:1.36","command":["echo","hi"],"resourceRequirements":[{"type":"VCPU","value":"1"},{"type":"MEMORY","value":"512"}]}'
6. aws batch submit-job --job-name j --job-queue <JobQueueArn> --job-definition jd  -> 200 with a jobId.
7. aws batch get-job-queue-snapshot --job-queue <JobQueueArn>  -> on AWS the outstanding job's jobArn is in frontOfQueue.jobs; on the emulator it never appears.
8. aws batch describe-jobs --jobs <jobId> repeatedly -> on AWS the status advances to RUNNING and then SUCCEEDED; on the emulator it never reaches RUNNING, SUCCEEDED or FAILED.
9. Re-replay batch.deploy-job-queue-via-cloudformation-accepts-and-runs-job.

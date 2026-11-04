Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.deploy-job-queue-via-cloudformation-processes-job
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch|SubmitJob|ClientException
Issue-Commit: c2d690adb5c99cf0fe6b339d9b4e7b6ccf196d09

# batch: a CloudFormation stack that reaches CREATE_COMPLETE with an AWS::Batch::JobDefinition never registers it, so SubmitJob refuses the definition as non-existent

> _Note: this issue was identified against the code base at commit `c2d690adb5c99cf0fe6b339d9b4e7b6ccf196d09`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
Cause 1 of 1 — an AWS::Batch::JobDefinition created by a CloudFormation stack is not registered with Batch.
AWS: when a stack containing AWS::Batch::ComputeEnvironment + AWS::Batch::JobQueue + AWS::Batch::JobDefinition reaches CREATE_COMPLETE, the job definition has been registered (equivalent to RegisterJobDefinition) and is ACTIVE at revision 1; the stack's JobDefinitionArn output resolves, and SubmitJob naming that definition by name is accepted with HTTP 200 {jobArn, jobName, jobId} (Smithy SubmitJob, POST /v1/submitjob, success 200, errors ClientException 400 / ServerException 500).
Emulator: the stack does reach CREATE_COMPLETE and the queue works, but SubmitJob with the job-definition name the template declared is refused — HTTP 400 ClientException "JobDefinition name <jobDefinitionName> does not exist or is not in ACTIVE status." In that message the fixed text is "JobDefinition name " … " does not exist or is not in ACTIVE status." and the variable part is the definition the request named; the point of this cause is the verdict (refused at all), not the wording — the wording of SubmitJob's not-found message is already filed as Cause 5 of open issue 20261102T113649Z-batch-job-api-refusal-contracts-diverge-two-wrong.md.
Why: CloudFormation is the normal way a Batch estate is deployed. A stack that reports CREATE_COMPLETE while one of its resources was never created makes the deployment a lie: every subsequent call that uses the resource fails as if the operator had never declared it, and there is nothing in the stack events to point at.
Fix: the AWS::Batch::JobDefinition resource provider in the cloudformation crate — it must register the job definition in the batch crate's state (and record the resulting ARN/revision as the resource's physical id) before the resource is reported CREATE_COMPLETE.
Traces: batch.deploy-job-queue-via-cloudformation-processes-job.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout, check batch.deploy-job-queue-via-cloudformation-processes-job:

botocore.errorfactory.ClientException: An error occurred (ClientException) when calling the SubmitJob operation: JobDefinition name sim2real-batch-cfn-jd-9c2407346d7e does not exist or is not in ACTIVE status.

The check had already passed the stack gate (it polls for a terminal stack status and asserts CREATE_COMPLETE) and submits with jobQueue=<stack queue name>, jobDefinition=<the JobDefinitionName parameter the stack declared>. Expected: HTTP 200 with a jobId, as on real AWS. Actual: HTTP 400 ClientException as above.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. aws cloudformation create-stack with a template declaring AWS::Batch::ComputeEnvironment (MANAGED/FARGATE), AWS::Batch::JobQueue, and AWS::Batch::JobDefinition (Type container, PlatformCapabilities [FARGATE], a JobDefinitionName parameter).
3. Wait for CREATE_COMPLETE.
4. aws batch describe-job-definitions --job-definitions <JobDefinitionName> -> nothing registered; and aws batch submit-job --job-queue <JobQueueName> --job-definition <JobDefinitionName> --job-name j -> 400 ClientException "JobDefinition name <name> does not exist or is not in ACTIVE status." On AWS the submit returns 200 with a jobId.
5. Re-replay batch.deploy-job-queue-via-cloudformation-processes-job.

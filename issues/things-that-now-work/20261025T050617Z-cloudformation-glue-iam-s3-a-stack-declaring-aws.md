Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__glue__iam__s3.deploy-gjwr-redeploy-whole-stack-to-expected-region-then-retire-the-misdeployed-one
Issue-Key: cloudformation__glue__iam__s3|GetJob|EntityNotFoundException
Issue-Commit: 4244fec3063c58aa4a2476d7619962eecf74c002

# cloudformation__glue__iam__s3: a stack declaring AWS::Glue::Job reaches CREATE_COMPLETE and resolves the job's physical id, but Glue has no such job (1 cause)

> _Note: this issue was identified against the code base at commit `4244fec3063c58aa4a2476d7619962eecf74c002`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJob

## Description
One net-new root cause in the bootstrapped cloudformation/glue/iam/s3 surface. The failing check is not marked ALREADY FILED or OWNED in the replay report.

Cause 1 of 1: a CloudFormation stack that declares an AWS::Glue::Job resource reports CREATE_COMPLETE, and CloudFormation itself resolves the job's physical resource id back to the owning stack, but the Glue service never holds that job — the first Glue call against the deployed name is refused as not found.

AWS: when CreateStack reports CREATE_COMPLETE for a template containing AWS::Glue::Job, the job exists in Glue in that stack's region and Glue GetJob(JobName=<physical id>) returns 200 with Job.Name equal to the physical id and Job.Role equal to the role ARN the template passed (Smithy: Glue GetJob.JobName @required NameString length 1-255; output member Job; modeled errors EntityNotFoundException/InvalidInputException/OperationTimeoutException (client, HTTP 400) and InternalServiceException (500)). The resource provider's create is Glue CreateJob (Smithy: CreateJob.Name/@required, Role/@required, Command/@required; output Name), so a stack that reaches CREATE_COMPLETE implies CreateJob succeeded. Reaching a Glue job through its stack is also how DescribeStackResources(PhysicalResourceId=...) is meant to be joined to the real resource.

Emulator: the stack settles CREATE_COMPLETE, its Outputs carry the derived job name, and cloudformation DescribeStackResources(PhysicalResourceId=<job name>) resolves it to the stack — but glue GetJob(JobName=<that same name>) is refused EntityNotFoundException (HTTP 400), e.g. "Job sim2real-gjwr-fix-<token>-RoleReport-<account>-us-east-1 not found". No Glue job was ever materialized for the deployed resource (nor in the stack's own region), so CloudFormation's success and its resource index both describe a resource that does not exist in the owning service.

Why: a client that provisions Glue jobs through CloudFormation — the ordinary path — gets a green deploy and then a 400 EntityNotFoundException on every subsequent Glue call (GetJob, StartJobRun, UpdateJob, tagging), so no deploy/verify, region-remediation or teardown workflow can run. Worse, DescribeStackResources corroborates the lie, so the usual "ask CloudFormation which stack owns this physical id" reconciliation reports a healthy mapping to a nonexistent job, and stack-status-gated CI never fires.

Fix: the AWS::Glue::Job resource provider in the CloudFormation service crate must actually create the job in the Glue service state for the stack's region during stack create/update (and delete it on stack delete), and must fail the resource — rolling the stack back — when that create does not succeed. Note the open issue 20261022T131025Z-glue-createjob-is-a-501-stub-gettable-ignores.md reports Glue CreateJob itself as a 501 scaffold stub; that stub is a separate cause and does not cover this one, because here CloudFormation reported CREATE_COMPLETE rather than surfacing the failure, which is the defect being filed.

Traces: cloudformation__glue__iam__s3.deploy-gjwr-redeploy-whole-stack-to-expected-region-then-retire-the-misdeployed-one.

## Evidence
Expected: after CreateStack on the glue-job-stack template reaches CREATE_COMPLETE, glue GetJob(JobName=Outputs.GlueJobName) returns 200 and Job.Role equals Outputs.GlueJobRoleArn.

Actual (replay): botocore.errorfactory.EntityNotFoundException: An error occurred (EntityNotFoundException) when calling the GetJob operation: Job sim2real-gjwr-fix-c1f046d306c8-RoleReport-000000000000-us-east-1 not found

The check reached that call only after the stack reported CREATE_COMPLETE, its DeploymentRegion output matched, and cloudformation DescribeStackResources(PhysicalResourceId=<same job name>) resolved to the stack name — i.e. CloudFormation claims to own a job Glue does not have. (The job name, account id, token and region in the message are environment values, not requirements; the requirement is that GetJob resolve the deployed job rather than answer EntityNotFoundException.)

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. CreateStack with a template containing AWS::Glue::Job (Name derived via Fn::Sub from ${AWS::AccountId}/${AWS::Region}, Role = Fn::GetAtt of an AWS::IAM::Role, Command {Name: glueetl, PythonVersion: 3, ScriptLocation: s3://...}), Capabilities=[CAPABILITY_NAMED_IAM]. Wait for CREATE_COMPLETE.
3. Read Outputs.GlueJobName and confirm cloudformation DescribeStackResources(PhysicalResourceId=<that name>) resolves to the stack.
4. Call glue GetJob(JobName=<that name>) in the stack's region — observe EntityNotFoundException/400 instead of the job.
5. Re-replay cloudformation__glue__iam__s3.deploy-gjwr-redeploy-whole-stack-to-expected-region-then-retire-the-misdeployed-one to confirm the fix.

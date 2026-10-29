Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__glue__iam__s3.deploy-gjwr-redeploy-whole-stack-to-expected-region-then-retire-the-misdeployed-one
Issue-Key: cloudformation__glue__iam__s3|GetJob|EntityNotFoundException
Issue-Commit: 4244fec3063c58aa4a2476d7619962eecf74c002

# CloudFormation never provisions AWS::Glue::Job: after CREATE_COMPLETE the stack exports the job name but glue:GetJob answers EntityNotFoundException

> _Note: this issue was identified against the code base at commit `4244fec3063c58aa4a2476d7619962eecf74c002`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: a CloudFormation stack that declares an AWS::Glue::Job reaches CREATE_COMPLETE and publishes the job's physical name through its Outputs, but the job is never registered in Glue, so the Glue API cannot see it.

AWS: a stack containing AWS::Glue::Job creates a real Glue job. After CREATE_COMPLETE, Ref of that resource is the job's physical name and glue:GetJob(JobName=<that name>) returns the Job (Smithy: GetJob input JobName (NameString, required, length 1-255); modeled errors EntityNotFoundException (client, 400), InvalidInputException, OperationTimeoutException, InternalServiceException). EntityNotFoundException is reserved for a job that does not exist; a job the stack just created does exist, in the region the stack was deployed to, and its Role is the role the stack created. CloudFormation also keeps that resource addressable through DescribeStackResources(PhysicalResourceId=<job name>).

Emulator: both halves of the deploy complete (CREATE_COMPLETE, DeploymentRegion/GlueJobName/GlueJobRoleArn outputs present, and DescribeStackResources(PhysicalResourceId=<job name>) does resolve the job to its stack), yet the very next call, glue:GetJob on that exact exported name in that stack's region, is refused 400 EntityNotFoundException (example message: "Job sim2real-gjwr-fix-<token>-RoleReport-000000000000-us-east-1 not found"). So CloudFormation's bookkeeping records the resource while the Glue service state never receives it. Hypothesis (neither the model nor the recording states which): either the AWS::Glue::Job provider is declaration-only and writes nothing to Glue, or it writes the job into a different region's partition than the region the stack was created in — the suite's open issue 20261025T015306Z-cloudformation-is-not-region-scoped-stack-names-and.md records region leakage elsewhere in CloudFormation.

Why: a client that deploys its Glue ETL job through CloudFormation (the normal way) cannot use it afterwards. Every downstream call keyed on the stack's exported job name — GetJob, StartJobRun, UpdateJob, TagResource, or a deploy-region remediation that redeploys the stack and then verifies each region's job — fails with a not-found for a resource the stack says it owns. CREATE_COMPLETE becomes an unreliable signal: the control plane claims success while the data plane has nothing.

Fix: the AWS::Glue::Job resource provider in CloudFormation's provisioning engine must create (and on stack delete, remove) the job in the Glue service state for the stack's own region, using the rendered Name and the GetAtt'd role ARN. This is the same component as cause 8 of the open issue 20261025T015306Z-cloudformation-cannot-deploy-large-parts-of-the-resource.md, which covers AWS::Glue::Database / AWS::Glue::Table never being created; that issue does not name AWS::Glue::Job and does not carry this check's replay key (cloudformation__glue__iam__s3 | GetJob | EntityNotFoundException), so this cause is still open work.

Traces: cloudformation__glue__iam__s3.deploy-gjwr-redeploy-whole-stack-to-expected-region-then-retire-the-misdeployed-one.

## Evidence
Replayed check cloudformation__glue__iam__s3.deploy-gjwr-redeploy-whole-stack-to-expected-region-then-retire-the-misdeployed-one ([error]):

  botocore.errorfactory.EntityNotFoundException: An error occurred (EntityNotFoundException) when calling the GetJob operation: Job sim2real-gjwr-fix-61d3604983e3-RoleReport-000000000000-us-east-1 not found

(the generated token, account id and region in that name are environment-specific; the requirement is the status/code, not the message text).

Expected: HTTP 200 with Job.Name == the stack's GlueJobName output and Job.Role == the stack's GlueJobRoleArn output.
Actual: HTTP 400, Error.Code EntityNotFoundException, for the name the stack itself exported.

What succeeded first in the same check, showing the stack really did complete:
- CreateStack in the other region -> CREATE_COMPLETE, Outputs.DeploymentRegion == that region;
- CreateStack of the same template under the same stack name in ctx.region -> CREATE_COMPLETE, Outputs.DeploymentRegion == ctx.region;
- both Outputs.GlueJobName values present, distinct, each ending in its own region;
- DescribeStackResources(PhysicalResourceId=<job name>) resolved to the stack, and the foreign region's job id did not resolve.
The first call that touches Glue is the one that fails.

## Steps to Reproduce
1. Boot the emulator and replay the failing check: cloudformation__glue__iam__s3.deploy-gjwr-redeploy-whole-stack-to-expected-region-then-retire-the-misdeployed-one.
2. Minimal repro: cloudformation:CreateStack with a template whose Resources contain an AWS::IAM::Role (trusting glue.amazonaws.com) and an AWS::Glue::Job with Name = {"Fn::Sub": "${JobBaseName}-${AWS::AccountId}-${AWS::Region}"}, Role = {"Fn::GetAtt": ["GlueJobRole", "Arn"]}, Command {Name: glueetl, PythonVersion: 3, ScriptLocation: s3://.../script.py}, GlueVersion 4.0, and an Output GlueJobName = {"Ref": "GlueJob"}; pass Capabilities=["CAPABILITY_NAMED_IAM"].
3. Poll DescribeStacks until CREATE_COMPLETE and read Outputs.GlueJobName.
4. Call glue:GetJob(JobName=<that output>) with a client in the stack's region -> observe 400 EntityNotFoundException instead of the Job.
5. Also call glue:ListJobs in that region to confirm the job is absent, and repeat step 4 with a client in every other region to tell a missing provider apart from a region-partition mismatch.

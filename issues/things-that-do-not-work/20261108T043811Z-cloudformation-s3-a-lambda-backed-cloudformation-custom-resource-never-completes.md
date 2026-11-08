Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3.deploy-cdk-bucket-deployment-lands-prunes-and-retains-on-destroy cloudformation__s3.deploy-cdk-bucket-deployment-denied-put-blocks-create-and-lands-nothing
Issue-Key: cloudformation__s3.deploy-cdk-bucket-deployment-lands-prunes-and-retains-on-destroy|_cdk#25|AssertionError|assert result.returncode == expect_rc, (
Issue-Key: cloudformation__s3.deploy-cdk-bucket-deployment-denied-put-blocks-create-and-lands-nothing|_cdk#27|AssertionError|assert rc == expect_rc, (
Issue-Commit: 8f7c6ce558c7d939146a7c42cc95df5134e5d406

# A Lambda-backed CloudFormation custom resource never completes: Custom::CDKBucketDeployment hangs CREATE_IN_PROGRESS until CFN times out and the stack ROLLBACK_FAILED

> _Note: this issue was identified against the code base at commit `8f7c6ce558c7d939146a7c42cc95df5134e5d406`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One cause, exercised by two checks.

Cause 1 of 1: a stack containing a Lambda-backed custom resource (CDK's aws_s3_deployment.BucketDeployment, logical type Custom::CDKBucketDeployment) never gets a response for the resource's Create, so CloudFormation leaves it CREATE_IN_PROGRESS until its wait expires, marks it CREATE_FAILED with "... was still CREATE_IN_PROGRESS when the wait for it to be created ran out", and the rollback then cannot delete it, ending the stack in ROLLBACK_FAILED. `cdk deploy` consequently exits 1 where it exits 0 against AWS.
Failing check ids: cloudformation__s3.deploy-cdk-bucket-deployment-lands-prunes-and-retains-on-destroy, cloudformation__s3.deploy-cdk-bucket-deployment-denied-put-blocks-create-and-lands-nothing

AWS: recorded behavior only (custom-resource semantics are not in a Smithy model). Recorded: CloudFormation invokes the custom resource's service token (a Lambda function) with the Create request, the function signals back SUCCESS or FAILED, and the resource leaves CREATE_IN_PROGRESS promptly. On SUCCESS the stack reaches CREATE_COMPLETE and the BucketDeployment's side effects are observable: the source bytes land in the destination bucket, an update that drops a source file prunes exactly that key (default prune=true), and `cdk destroy` retains the bucket and its object (default retainOnDelete=true). On a Create the function fails — e.g. the destination bucket's policy Denies s3:PutObject — the resource goes CREATE_FAILED with the function's reason, the stack rolls back cleanly, and no object lands. Either way the resource reaches a terminal state from the function's response, not from a timeout; the resource's physical id and the stack name in the event text are per-run values.

Emulator: the Create is never answered. Both deploys show the identical shape — "Custom::CDKBucketDeployment <stack>-DeployCustomResource218AF6A4-<id> was still CREATE_IN_PROGRESS when the wait for it to be created ran out", then "ROLLBACK_FAILED | AWS::CloudFormation::Stack | <stack> The following resource(s) failed to delete: [DeployCustomResource218AF6A4].", and `cdk deploy --require-approval never` exits 1. Because the create never completes, neither the happy path (land/prune/retain) nor the authorization-refusal path can be observed at all: the negative check's own control deploy against an unrestricted bucket fails the same way.

Why: every CDK/SAM construct implemented as a custom resource (BucketDeployment, auto-delete-objects, CDK bootstrap helpers, Lambda@Edge wiring, SSM/parameter providers) becomes undeployable. A client gets a stack stuck in ROLLBACK_FAILED — which blocks subsequent UpdateStack and DeleteStack without a skip list — instead of a working stack, and the CLI's non-zero exit breaks any pipeline that deploys CDK apps. It also masks the authorization behavior the negative check exists to prove, so a real "bucket policy denies the deployment's Put" regression cannot be detected.

Fix: the CloudFormation provisioner's custom-resource handling — the component that invokes the resource's ServiceToken Lambda with the Create/Update/Delete request payload (including ResponseURL) and applies the function's SUCCESS/FAILED response to the resource's state. It must drive the resource to a terminal state from that response rather than from the wait timeout, and the rollback path must then be able to delete it.

Traces: cloudformation__s3.deploy-cdk-bucket-deployment-lands-prunes-and-retains-on-destroy (happy path: land, prune on update, retain on destroy); cloudformation__s3.deploy-cdk-bucket-deployment-denied-put-blocks-create-and-lands-nothing (negative: bucket policy Denies s3:PutObject, plus a control deploy against an unrestricted bucket). The open issue 20261105T143253Z-cloudformation-lambda-s3-sqs-a-stack-whose-custom.md records a custom-resource defect for the cloudformation__lambda__s3__sqs suite; it does not carry either of these replay keys.

## Evidence
Check: cloudformation__s3.deploy-cdk-bucket-deployment-lands-prunes-and-retains-on-destroy
  AssertionError: `cdk deploy --require-approval never` exited 1, expected 0; output tail includes:
    "ROLLBACK_FAILED | AWS::CloudFormation::Stack | <stack> The following resource(s) failed to delete: [DeployCustomResource218AF6A4]."
    "CREATE_FAILED | Custom::CDKBucketDeployment | Deploy/CustomResource/Default (DeployCustomResource218AF6A4) Custom::CDKBucketDeployment <stack>-DeployCustomResource218AF6A4-<id> was still CREATE_IN_PROGRESS when the wait for it to be created ran out"
    "DeploymentError: Resource updates failed"

Check: cloudformation__s3.deploy-cdk-bucket-deployment-denied-put-blocks-create-and-lands-nothing
  AssertionError: `cdk deploy --require-approval never` exited 1, expected 0; the same CREATE_IN_PROGRESS-timeout / ROLLBACK_FAILED pair, and on the *control* stack (the unrestricted bucket, expected rc 0).

Expected (AWS): rc 0 for both the lifecycle deploy and the control deploy; the custom resource reaches CREATE_COMPLETE from the function's SUCCESS response; the payload object exists in the destination bucket; an update dropping a source file prunes exactly that key; `cdk destroy` leaves bucket and object in place. For the denied deploy: the resource reaches CREATE_FAILED from the function's FAILED response, the stack rolls back to a clean state, and no object lands.

Actual (emulator): the custom resource is never answered, so it times out in CREATE_IN_PROGRESS, the stack ends ROLLBACK_FAILED with the resource undeletable, and the CLI exits 1. Stack names, physical resource ids and timestamps in the output are per-run values; the shape (timeout in CREATE_IN_PROGRESS, then ROLLBACK_FAILED naming the custom resource) is the requirement that diverges.

## Steps to Reproduce
1. Bootstrap a CDK toolkit stack in the emulator (cdk bootstrap with a unique qualifier).
2. Stage a CDK app whose stack declares an aws_s3_deployment.BucketDeployment (Custom::CDKBucketDeployment) writing a small source directory into a destination bucket.
3. Run `cdk deploy --require-approval never`.
4. Watch the stack events: Deploy/CustomResource/Default stays CREATE_IN_PROGRESS, then CREATE_FAILED with "... was still CREATE_IN_PROGRESS when the wait for it to be created ran out", then the stack reports ROLLBACK_FAILED listing that resource as undeletable; the CLI exits 1.
5. Repeat with the destination bucket carrying a policy that Denies s3:PutObject: AWS fails the Create with the function's reason and rolls back cleanly, the emulator produces the same timeout/ROLLBACK_FAILED outcome, so the two paths are indistinguishable.

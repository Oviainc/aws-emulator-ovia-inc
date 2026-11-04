Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3.deploy-cdk-bucket-deployment-denied-put-blocks-create-and-lands-nothing cloudformation__s3.deploy-cdk-bucket-deployment-lands-prunes-and-retains-on-destroy
Issue-Key: cloudformation__s3.deploy-cdk-bucket-deployment-denied-put-blocks-create-and-lands-nothing|_cdk#27|AssertionError|assert rc == expect_rc, (
Issue-Key: cloudformation__s3.deploy-cdk-bucket-deployment-lands-prunes-and-retains-on-destroy|_cdk#25|AssertionError|assert result.returncode == expect_rc, (
Issue-Commit: 6ff397d47b7f23b010f8590f84afedb6b986e178

# cloudformation__s3: Ref on AWS::Lambda::LayerVersion resolves to the layer name instead of the LayerVersionArn, so every CDK BucketDeployment stack fails to deploy

> _Note: this issue was identified against the code base at commit `6ff397d47b7f23b010f8590f84afedb6b986e178`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: an AWS::Lambda::LayerVersion resource's physical resource id (what Ref yields) is the layer's name, not its version ARN, so any function that lists it under Layers is refused.
AWS: Smithy: lambda#LayerList member targets lambda#LayerVersionArn, @length 1..2048 and @pattern `^((arn:(aws[a-zA-Z-]*)?:lambda:(eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1}:\d{12}:layer:[a-zA-Z0-9-_]+:[0-9]+)|(arn:[a-zA-Z0-9-]+:lambda:::awslayer:[a-zA-Z0-9-_]+))$`, with an arnReference to AWS::Lambda::LayerVersion. Ref on an AWS::Lambda::LayerVersion therefore returns the version ARN `arn:aws:lambda:<region>:<account>:layer:<layer-name>:<version>` (recorded behavior for the CFN resource), which is exactly what AWS::Lambda::Function's `Layers: [!Ref TheLayer]` needs. A `cdk deploy` of a stack using aws-s3-deployment (which synthesises a DeployAwsCliLayer LayerVersion plus the Custom::CDKBucketDeployment function) exits 0 and reaches CREATE_COMPLETE.
Emulator: Ref resolves to the generated layer NAME (shaped `<stack-name>-DeployAwsCliLayer<hash>-<suffix>`), so the Function resource's CreateFunction call is refused and the resource handler reports `Resource handler returned message: "1 validation error detected: Value '[<layer-name>]' at 'layers' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 2048, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: ((arn:(aws[a-zA-Z-]*)?:lambda:…:layer:[a-zA-Z0-9-_]+:[0-9]+)|(arn:[a-zA-Z0-9-]+:lambda:::awslayer:[a-zA-Z0-9-_]+)), Member must not be null]" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)`. `cdk deploy --require-approval never` exits 1 with "❌ <stack> failed to deploy". The value quoted in the message is a per-run generated name; the defect is that it is a name at all rather than a layer-version ARN.
Why: this blocks the whole CDK aws-s3-deployment pattern (and any hand-written template that passes `!Ref` of a LayerVersion into a function's Layers), and it does so with an error that points at Lambda's input validation rather than at the Ref resolution that produced a bad value. More broadly, a resource type whose physical id is not the identifier AWS returns breaks every cross-resource reference in the template.
Fix: the CloudFormation stack provisioner's AWS::Lambda::LayerVersion resource handler — its physical resource id (and Ref) must be the LayerVersionArn returned by PublishLayerVersion.
Traces: cloudformation__s3.deploy-cdk-bucket-deployment-denied-put-blocks-create-and-lands-nothing and cloudformation__s3.deploy-cdk-bucket-deployment-lands-prunes-and-retains-on-destroy, both failing in `_cdk(ctx, work, ["deploy", "--require-approval", "never"])`.

## Evidence
Expected: `cdk deploy --require-approval never` exits 0 and the stack reaches CREATE_COMPLETE; `!Ref` of the synthesised DeployAwsCliLayer yields `arn:aws:lambda:<region>:<account>:layer:<name>:<version>` and CreateFunction accepts it.
Actual: rc=1, "DeploymentError: Resource updates failed: <stack>/Custom::CDKBucketDeployment…/Resource (AWS::Lambda::Function Custom…) Resource handler returned message: \"1 validation error detected: Value '[<stack>-DeployAwsCliLayer<hash>-<suffix>]' at 'layers' failed to satisfy constraint: Member must satisfy constraint: [… Member must satisfy regular expression pattern: ((arn:(aws[a-zA-Z-]*)?:lambda:…:layer:[a-zA-Z0-9-_]+:[0-9]+)|(arn:[a-zA-Z0-9-]+:lambda:::awslayer:[a-zA-Z0-9-_]+)), Member must not be null]\" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)" — i.e. the Layers member received a bare layer name. Both checks fail identically, with only the per-run stack name, layer suffix and request token differing.

## Steps to Reproduce
Replay either cloudformation__s3.deploy-cdk-bucket-deployment-lands-prunes-and-retains-on-destroy or cloudformation__s3.deploy-cdk-bucket-deployment-denied-put-blocks-create-and-lands-nothing: synthesise a CDK app that uses aws-cdk-lib/aws-s3-deployment BucketDeployment against an S3 bucket and run `cdk deploy --require-approval never`. Equivalently, CreateStack a template with an AWS::Lambda::LayerVersion plus an AWS::Lambda::Function whose `Layers` is `[!Ref TheLayer]`, and inspect the value the Function resource receives.

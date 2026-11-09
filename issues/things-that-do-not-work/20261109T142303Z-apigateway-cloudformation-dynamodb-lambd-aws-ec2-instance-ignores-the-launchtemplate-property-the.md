Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__dynamodb__lambda__s3__ssm__stepfunctions.cdk-reference-estate-deploys-serves-and-destroys
Issue-Key: apigateway__cloudformation__dynamodb__lambda__s3__ssm__stepfunctions.cdk-reference-estate-deploys-serves-and-destroys|_run_cli#13|AssertionError|assert result.returncode == 0, (
Issue-Commit: 5bc50a64f180637d452285d750c7aafc588926d7

# AWS::EC2::Instance ignores the LaunchTemplate property — the resource handler refuses with emulator prose, failing every CDK/CFN deploy that launches from a launch template

> _Note: this issue was identified against the code base at commit `5bc50a64f180637d452285d750c7aafc588926d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::EC2::Instance does not apply its LaunchTemplate property, and the CloudFormation resource handler answers with emulator-authored prose instead of provisioning the instance.

AWS: AWS::EC2::Instance declares a LaunchTemplate property (LaunchTemplateSpecification: LaunchTemplateId / LaunchTemplateName / Version). CloudFormation resolves it and RunInstances is called with LaunchTemplate, so the instance inherits the template's AMI, instance type, instance profile, security groups and metadata options. Smithy (ec2): RunInstances input member LaunchTemplate -> LaunchTemplateSpecification. The resource reaches CREATE_COMPLETE; no refusal exists for a valid LaunchTemplate reference. Recorded behavior only for the exact wording of a real AWS refusal here, because AWS does not refuse this at all.

Emulator: the AWS::EC2::Instance handler fails the resource with `Resource handler returned message: "this emulator does not apply the LaunchTemplate property of AWS::EC2::Instance" (HandlerErrorCode: InvalidRequest)`. The whole fixed text of that message is emulator-authored — AWS has no such message and no such refusal class. The stack (`...-SsmDocAssoc`) therefore fails to deploy and `cdk deploy --all` exits 1 instead of 0.

Why: a client that deploys any CDK/CloudFormation estate whose instances are defined by a launch template (the CDK default for several constructs, and the normal pattern for pinning AMI/profile/metadata options) cannot deploy at all. The failure is also mislabelled InvalidRequest, so a client reads it as a bad template and "fixes" a template that is already correct.

Fix: the AWS::EC2::Instance resource handler in the CloudFormation service — it must resolve the LaunchTemplate property to a launch-template id/version, merge the template's launch parameters under the template's own precedence rules, and pass LaunchTemplate through to RunInstances rather than refusing.

Traces: check apigateway__cloudformation__dynamodb__lambda__s3__ssm__stepfunctions.cdk-reference-estate-deploys-serves-and-destroys.

## Evidence
Expected: `cdk deploy --all` exits 0; the AWS::EC2::Instance resource declaring LaunchTemplate reaches CREATE_COMPLETE and the launched instance carries the template's AMI/profile/security groups.

Actual: `cdk deploy --all` exited 1. Stack event for AWS::EC2::Instance (logical id SsmManagedInstance5E2196A8):
  Resource handler returned message: "this emulator does not apply the LaunchTemplate property of AWS::EC2::Instance"
  (HandlerErrorCode: InvalidRequest)
Fixed text of the diverging message: the whole string `this emulator does not apply the LaunchTemplate property of AWS::EC2::Instance` — it is emulator prose with no variable parts and no AWS counterpart. Variable parts of the envelope (RequestToken, the logical/physical id) are per-run values and are not part of the requirement.

## Steps to Reproduce
1. Synthesize a CloudFormation/CDK stack containing an AWS::EC2::Instance whose Properties set `LaunchTemplate: {LaunchTemplateId: <id>, Version: <v>}` (and no redundant ImageId/InstanceType).
2. CreateStack / `cdk deploy` it against the emulator.
3. Poll DescribeStackEvents: the instance resource goes CREATE_FAILED with ResourceStatusReason `Resource handler returned message: "this emulator does not apply the LaunchTemplate property of AWS::EC2::Instance" (... HandlerErrorCode: InvalidRequest)`, and the stack rolls back instead of reaching CREATE_COMPLETE.

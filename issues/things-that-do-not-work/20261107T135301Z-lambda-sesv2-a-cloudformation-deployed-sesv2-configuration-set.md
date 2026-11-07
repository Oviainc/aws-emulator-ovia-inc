Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sesv2.cfn-deploy-lambda-sesv2-env-configset-arn-resolution
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-env-configset-arn-resolution|_poll_until#5|Timeout|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 76f746b5965121353b22ae227b995a601e952faf

# lambda__sesv2: a CloudFormation-deployed SESv2 configuration-set ARN env reference never settles

> _Note: this issue was identified against the code base at commit `76f746b5965121353b22ae227b995a601e952faf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause under '### lambda__sesv2'; this check is not marked ALREADY FILED or OWNED (the other failing checks in this service are owned by 20261105T080633Z-lambda-sesv2-the-emulator-s-python-lambda-runtime.md, 20261106T151148Z-lambda-sesv2-boto3-client-sesv2-and-boto3-client.md and 20261107T091747Z-lambda-sesv2-five-cloudformation-deployed-sesv2-lambda-checks.md, none of which carries this replay key).

Cause 1 of 1: a stack that passes an AWS::SESV2::ConfigurationSet reference into a function's environment never converges.
AWS: CreateStack with AWS::SESV2::ConfigurationSet plus AWS::Lambda::Function whose Environment resolves Ref/Fn::GetAtt on the configuration set reaches CREATE_COMPLETE, and GetFunctionConfiguration then shows the resolved configuration-set name/ARN in the environment — all within a normal stack poll.
Emulator: the check ends with 'Error: Check timed out after 60.0s' — the stack never reaches a terminal status the check can read, so neither the resolution nor a failure reason is observable.
Why: a client cannot deploy the standard 'pass my SESv2 configuration set to my function' template; the stack appears to hang with no resolved environment and no failure reason.
Fix: the AWS::SESV2::ConfigurationSet resource provider and intrinsic resolution into AWS::Lambda::Function Environment in the cloudformation provisioner.
Traces: lambda__sesv2.cfn-deploy-lambda-sesv2-env-configset-arn-resolution.

## Evidence
'Error: Check timed out after 60.0s' for lambda__sesv2.cfn-deploy-lambda-sesv2-env-configset-arn-resolution; no stack status, no stack event reason and no resolved environment value were produced.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite.
2. Re-replay lambda__sesv2.cfn-deploy-lambda-sesv2-env-configset-arn-resolution (listed under '### lambda__sesv2').
3. CreateStack with an AWS::SESV2::ConfigurationSet and an AWS::Lambda::Function whose Environment.Variables references it, then poll DescribeStacks.

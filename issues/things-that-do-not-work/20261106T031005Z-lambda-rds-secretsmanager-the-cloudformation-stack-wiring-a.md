Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__rds__secretsmanager.deploy-cfn-rds-rotation-lambda-wiring
Issue-Key: lambda__rds__secretsmanager.deploy-cfn-rds-rotation-lambda-wiring|_deploy_cfn#1|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Commit: 1ea8394ddf89938ba9051f7092160f10eda1506a

# lambda__rds__secretsmanager: the CloudFormation stack wiring a rotation Lambda to an RDS master-credential secret always rolls back, so the rotation wiring is never deployed

> _Note: this issue was identified against the code base at commit `1ea8394ddf89938ba9051f7092160f10eda1506a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CloudFormation stack that wires a Secrets Manager rotation Lambda to an RDS DB instance's master credential never reaches CREATE_COMPLETE — it ends ROLLBACK_COMPLETE.

AWS: recorded behavior only (CloudFormation's stack outcome is not expressed in the Smithy model). A template that declares an AWS::RDS::DBInstance, an AWS::SecretsManager::Secret holding its master credential, an AWS::Lambda::Function to rotate it, the AWS::Lambda::Permission granting secretsmanager.amazonaws.com lambda:InvokeFunction, and an AWS::SecretsManager::RotationSchedule binding the secret to that function, deploys to StackStatus CREATE_COMPLETE; DescribeStacks then reports CREATE_COMPLETE and the rotation wiring is readable through DescribeSecret's RotationEnabled/RotationLambdaARN.
Emulator: the create fails and the stack is rolled back — DescribeStacks reports StackStatus ROLLBACK_COMPLETE, so none of the stack's resources survive and the rotation wiring is never established. Every assertion downstream of the deployment is unreachable.
Why: a client cannot provision Secrets Manager rotation for an RDS credential through CloudFormation at all — the documented reference pattern for rotating RDS master passwords is undeployable, and because the stack rolls back the operator is left with no resources and only the stack events to diagnose from.
Fix: the CloudFormation resource providers for the resource types this stack declares (AWS::Lambda::Function, AWS::Lambda::Permission, AWS::SecretsManager::Secret, AWS::SecretsManager::RotationSchedule, AWS::RDS::DBInstance) and the create/rollback path that is failing one of them — the stack's CREATE_FAILED event ResourceStatusReason names the offending resource and must be made to succeed rather than roll back.
Traces: lambda__rds__secretsmanager.deploy-cfn-rds-rotation-lambda-wiring.

## Evidence
AssertionError: Stack sim2real-xsvc-stk-<id> ended in ROLLBACK_COMPLETE (raised from the check's _deploy_cfn helper, which polls DescribeStacks for a terminal status and expects CREATE_COMPLETE). Expected: StackStatus == 'CREATE_COMPLETE' with the rotation Lambda, its invoke permission and the secret's RotationSchedule all created.

## Steps to Reproduce
CreateStack with the check's template (AWS::RDS::DBInstance + AWS::SecretsManager::Secret + AWS::Lambda::Function rotation handler + AWS::Lambda::Permission for secretsmanager.amazonaws.com + AWS::SecretsManager::RotationSchedule), then poll DescribeStacks for a terminal status; it answers ROLLBACK_COMPLETE instead of CREATE_COMPLETE. Read DescribeStackEvents and look for the CREATE_FAILED resource and its ResourceStatusReason.

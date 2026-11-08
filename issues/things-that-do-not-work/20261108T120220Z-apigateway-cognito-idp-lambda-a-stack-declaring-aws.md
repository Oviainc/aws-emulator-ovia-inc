Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.cfn-apigateway-role-trusts-apigateway-service-positive
Issue-Key: apigateway__cognito-idp__lambda.cfn-apigateway-role-trusts-apigateway-service-positive|_deploy_cfn#6|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Commit: 63b1f768cca526e685e8991a6e4ba2a43400c274

# apigateway/cognito-idp/lambda: a stack declaring AWS::ApiGateway::Account with an apigateway-trusting role ends ROLLBACK_FAILED instead of CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `63b1f768cca526e685e8991a6e4ba2a43400c274`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause, grounded in a failing check the replay report marked neither ALREADY FILED nor OWNED.

Cause 1 of 1 — CreateStack of a template whose AWS::IAM::Role trusts apigateway.amazonaws.com and whose AWS::ApiGateway::Account points cloudwatchRoleArn at it terminates in ROLLBACK_FAILED.
AWS: Smithy (cloudformation) CreateStack followed by DescribeStacks reports StackStatus CREATE_COMPLETE for this template; the account-level setting it writes is the one Smithy (apigateway) UpdateAccount (PATCH /account) exposes as Account.cloudwatchRoleArn, a per-account-per-region singleton that CloudFormation sets on create. ROLLBACK_FAILED is the doubly-terminal status AWS reports only when the create failed AND the rollback itself could not complete — it leaves the stack neither usable nor deletable without retaining resources, so it is never the outcome of a well-formed template.
Emulator: DescribeStacks reports StackStatus ROLLBACK_FAILED, so the role is never assumable by API Gateway, the account cloudwatchRoleArn is never set, and the stack is wedged.
Why: a client deploying the standard 'API Gateway may push execution logs to CloudWatch' bootstrap stack gets a stack it can neither use nor clean up, and the downstream execution-log behavior that depends on the account role can never be reached. A rollback that itself fails also means the emulator's create path leaves resources half-built.
Fix: the AWS::ApiGateway::Account resource provider in the CloudFormation engine — its create path and the delete/rollback path that must undo it — together with whatever resource failure inside the stack triggers the rollback in the first place.
Failing check: apigateway__cognito-idp__lambda.cfn-apigateway-role-trusts-apigateway-service-positive.

## Evidence
AssertionError: Stack <stack-name> ended in ROLLBACK_FAILED (raised from the check's _deploy_cfn helper after polling DescribeStacks). Expected StackStatus CREATE_COMPLETE, then get_account().cloudwatchRoleArn equal to the CFN-created role's ARN. The stack name is a per-run generated value and is not part of the requirement. The check carried no 'ALREADY FILED' or 'OWNED by' marker.

## Steps to Reproduce
CreateStack a template containing (a) an AWS::IAM::Role whose AssumeRolePolicyDocument trusts apigateway.amazonaws.com with the AmazonAPIGatewayPushToCloudWatchLogs managed policy, and (b) an AWS::ApiGateway::Account whose CloudWatchRoleArn is !GetAtt of that role; poll DescribeStacks -> expect CREATE_COMPLETE and apigateway GetAccount reporting that cloudwatchRoleArn, observe ROLLBACK_FAILED.

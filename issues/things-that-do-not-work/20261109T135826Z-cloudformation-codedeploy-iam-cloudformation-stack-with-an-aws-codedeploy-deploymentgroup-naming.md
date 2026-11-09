Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__codedeploy.deploy-cfn-codedeploy-deploymentgroup-invalid-role-rejected
Issue-Key: cloudformation__codedeploy.deploy-cfn-codedeploy-deploymentgroup-invalid-role-rejected|run#19|AssertionError|assert status == "ROLLBACK_COMPLETE", f"invalid-role stack ended {status!r}"
Issue-Commit: 74ebb92ce9e8b3d0346604c0e135c3e0968fb0a4

# CloudFormation stack with an AWS::CodeDeploy::DeploymentGroup naming a non-assumable service role reaches CREATE_COMPLETE instead of rolling back (1 cause)

> _Note: this issue was identified against the code base at commit `74ebb92ce9e8b3d0346604c0e135c3e0968fb0a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CloudFormation stack whose AWS::CodeDeploy::DeploymentGroup resource points ServiceRoleArn at an IAM role that does not exist (and so cannot be assumed by codedeploy.amazonaws.com) is created successfully by the emulator.
AWS: the DeploymentGroup resource provider calls codedeploy:CreateDeploymentGroup, which refuses the unusable role with HTTP 400 and the message "AWS CodeDeploy does not have the permissions required to assume the role arn:<partition>:iam::<account>:role/<name>." CloudFormation surfaces that as a CREATE_FAILED event on the DeploymentGroup logical resource whose ResourceStatusReason is 'Resource handler returned message: "AWS CodeDeploy does not have the permissions required to assume the role arn:<partition>:iam::<account>:role/<name>. (Service: CodeDeploy, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)' — the fixed text is the sentence and the parenthetical frames; the account, role name, partition, request id and attempt count come from the request and the run. The stack then rolls back to ROLLBACK_COMPLETE and the AWS::CodeDeploy::Application the stack had already created is deleted, so codedeploy:GetApplication on it answers ApplicationDoesNotExistException "No application found for name: <name>". The same template with an assumable role deploys normally (the positive control in this check passes).
Emulator: the stack with the bogus role ends 'CREATE_COMPLETE' — no CREATE_FAILED event, no handler reason, no rollback, and the application created by the stack survives. This is the CloudFormation/provider-side instance of the acceptance already filed for the direct API in open issue 20261022T171541Z-codedeploy-createdeploymentgroup-and-createdeployment-accept-input-real-aws.md, which does not carry this check's replay key and covers only the direct CreateDeploymentGroup call, not the stack outcome, the event reason or the rollback.
Why: infrastructure-as-code users rely on a stack failing when a resource is invalid. Here a misconfigured deployment group is reported as a healthy stack, so broken infrastructure is promoted, the rollback path (and the cleanup of resources created earlier in the stack) is never exercised, and the handler-error contract clients key alarms on (HandlerErrorCode InvalidRequest) never appears.
Fix: the AWS::CodeDeploy::DeploymentGroup resource provisioning path in the CloudFormation emulator must propagate the underlying CodeDeploy refusal into a CREATE_FAILED resource event with the handler-reason wrapper and drive the stack to ROLLBACK_COMPLETE — which in turn requires CodeDeploy's CreateDeploymentGroup validation to reject a service role it cannot assume.
Traces: cloudformation__codedeploy.deploy-cfn-codedeploy-deploymentgroup-invalid-role-rejected.

## Evidence
Expected (AWS): wait_stack(...) on the invalid-role stack settles at 'ROLLBACK_COMPLETE'; the CDDeploymentGroup event carries ('AWS CodeDeploy does not have the permissions required to assume the role arn:<partition>:iam::<account>:role/<name>. (Service: CodeDeploy, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)', 'InvalidRequest'); the stack's CDApplication is gone, so GetApplication raises ApplicationDoesNotExistException 'No application found for name: <name>'.

Actual (emulator): "AssertionError: invalid-role stack ended 'CREATE_COMPLETE'" at check_cloudformation__codedeploy_deploy_cfn_codedeploy_deploymentgroup_invalid_role_rejected.py line 250 — the assertion fires before the handler reason and the application-absence assertions can be reached. The control stack (same template, assumable role) reached CREATE_COMPLETE with the expected serviceRoleArn/ec2TagFilters, so only the refusal path diverges.

## Steps to Reproduce
1. iam:CreateRole a role trusting codedeploy.amazonaws.com and deploy the template once with it (control) — the stack reaches CREATE_COMPLETE and the group carries that serviceRoleArn.
2. Build an ARN for a role that was never created: arn:aws:iam::<account>:role/<nonexistent-name>.
3. cloudformation:CreateStack with the same template (AWS::CodeDeploy::Application + AWS::CodeDeploy::DeploymentGroup with one Ec2TagFilter) passing that ARN as the ServiceRoleArn parameter.
4. Poll DescribeStacks(StackName=<stack-id>) to a terminal status — observe 'CREATE_COMPLETE' instead of 'ROLLBACK_COMPLETE'.
5. DescribeStackEvents: no CREATE_FAILED event for the DeploymentGroup logical resource; codedeploy:GetApplication on the stack's application still succeeds instead of raising ApplicationDoesNotExistException.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__rds__secretsmanager.lambda-rds-data-api-execute-statement-positive lambda__rds__secretsmanager.lambda-rds-data-api-executestatement-positive lambda__rds__secretsmanager.lambda-rds-data-api-missing-permission-negative lambda__rds__secretsmanager.lambda-rds-wrong-db-cluster-arn-negative lambda__rds__secretsmanager.deploy-cfn-lambda-execution-role-trusts-lambda-service-positive lambda__rds__secretsmanager.deploy-cfn-lambda-role-wrong-trust-rejected lambda__rds__secretsmanager.deploy-cfn-lambda-rds-secretsmanager-stack lambda__rds__secretsmanager.rds-rotation-lambda-missing-invoke-permission-denied
Issue-Key: lambda__rds__secretsmanager|CreateDBCluster|NotImplemented
Issue-Key: lambda__rds__secretsmanager|GetFunction|ResourceNotFoundException
Issue-Key: lambda__rds__secretsmanager.deploy-cfn-lambda-role-wrong-trust-rejected|run#7|AssertionError|assert status == "ROLLBACK_COMPLETE", f"expected ROLLBACK_COMPLETE, got {status!r}"
Issue-Key: lambda__rds__secretsmanager.deploy-cfn-lambda-rds-secretsmanager-stack|_deploy_cfn#1|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: lambda__rds__secretsmanager.rds-rotation-lambda-missing-invoke-permission-denied|_expect_denied#10|AssertionError|raise AssertionError(
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# lambda__rds__secretsmanager: 5 net-new root causes — CreateDBCluster refuses ManageMasterUserPassword with 501, a CFN-deployed Lambda function does not exist in Lambda, a wrong-trust execution role is accepted, the Aurora+secret+Lambda stack rolls back, and RotateSecret ignores the rotation function's missing invoke permission

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
Five distinct root causes behind the unowned lambda__rds__secretsmanager failures. The replay report marks the Lambda-Invoke-blocked checks of this group as OWNED by 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md and they are skipped here; the causes below are unowned.

Cause 1 of 5: CreateDBCluster refuses the ManageMasterUserPassword member with HTTP 501 NotImplemented.
AWS: CreateDBCluster$ManageMasterUserPassword=true is accepted: RDS generates the master password in Secrets Manager and reports the managed secret as DBCluster$MasterUserSecret (SecretArn/SecretStatus/KmsKeyId), with no MasterUserPassword in the request.
Emulator: the whole call is refused — code 'NotImplemented', message "CreateDBCluster$ManageMasterUserPassword is not implemented in this emulator: it has RDS generate the master user password in Amazon Web Services Secrets Manager and report that secret as MasterUserSecret, and this service mints no such secret. Name MasterUserPassword instead." So no Aurora cluster can be created the way these flows create one, and four RDS Data API checks never reach their subject.
Why: a request AWS accepts gets a 501, and the modelled MasterUserSecret contract is unavailable; every managed-master-password flow is blocked. (The sibling CreateDBInstance gap — ManageMasterUserPassword returning no MasterUserSecret — is already filed as 20261024T030130Z-kms-rds-secretsmanager-rds-managemasteruserpassword-true-returns-no-masterusersecret-so-no.md; that issue does not cover CreateDBCluster's 501 refusal or these checks.)
Fix: the CreateDBCluster handler plus the RDS-to-Secrets-Manager managed-secret minting it needs.
Traces: lambda__rds__secretsmanager.lambda-rds-data-api-execute-statement-positive, lambda__rds__secretsmanager.lambda-rds-data-api-executestatement-positive, lambda__rds__secretsmanager.lambda-rds-data-api-missing-permission-negative, lambda__rds__secretsmanager.lambda-rds-wrong-db-cluster-arn-negative

Cause 2 of 5: a CloudFormation stack declaring AWS::Lambda::Function reaches CREATE_COMPLETE but Lambda does not hold the function.
AWS: after the stack completes, lambda.GetFunction on the function the stack created (by name or by the stack's reported function ARN) returns the function configuration.
Emulator: GetFunction answers ResourceNotFoundException "Function not found: <function arn>" for the function the stack just reported as created.
Why: anything that deploys a Lambda through CloudFormation and then addresses it through the Lambda API breaks — the function exists only inside the stack.
Fix: the CloudFormation AWS::Lambda::Function resource provider — it must create the function through Lambda so Lambda state holds it. Related open issues that do not carry this check: 20261023T004328Z-bedrock-agent-runtime-lambda-s3vectors-cloudformation-stacks-declaring-aws-lambda-function-reach-create.md and 20261024T113144Z-lambda-function-tags-are-absent-from-the-resource.md.
Traces: lambda__rds__secretsmanager.deploy-cfn-lambda-execution-role-trusts-lambda-service-positive

Cause 3 of 5: CreateFunction accepts an execution role whose trust policy does not allow lambda.amazonaws.com, so the stack succeeds where AWS rolls it back.
AWS: CreateFunction validates that the Role can be assumed by lambda.amazonaws.com and refuses a role trusting only another principal (e.g. ec2.amazonaws.com) with InvalidParameterValueException; inside CloudFormation that refusal fails the function resource and the stack ends ROLLBACK_COMPLETE with no function.
Emulator: the stack whose only defect is the role's trust principal reaches 'CREATE_COMPLETE'.
Why: an over-permissive accept hides a real misconfiguration — the same template fails against AWS, and the emulator reports a healthy deployment.
Fix: the CreateFunction handler's execution-role trust validation (and the CFN provider's propagation of that refusal).
Traces: lambda__rds__secretsmanager.deploy-cfn-lambda-role-wrong-trust-rejected

Cause 4 of 5: the Aurora Serverless v2 + Secrets Manager secret + Lambda stack cannot be deployed at all — it ends ROLLBACK_COMPLETE.
AWS: the stack reaches CREATE_COMPLETE and the Lambda then reads the secret and talks to the cluster.
Emulator: the deploy helper raises on the terminal status 'ROLLBACK_COMPLETE' (recorded behavior only — the failing resource's reason is not captured in this check's output, so the specific resource provider at fault must be identified from the stack's events during the fix; causes 1-3 of this issue are the likely contributors since the template declares a DBCluster with a managed master secret and a Lambda).
Why: the whole cross-service deployment path is unavailable.
Fix: whichever resource provider fails in this stack — start from DescribeStackEvents for the rolled-back stack and the CreateDBCluster/AWS::Lambda::Function providers named above.
Traces: lambda__rds__secretsmanager.deploy-cfn-lambda-rds-secretsmanager-stack

Cause 5 of 5: RotateSecret succeeds although the rotation Lambda's resource policy grants secretsmanager.amazonaws.com no invoke permission.
AWS: RotateSecret resolves RotationLambdaARN and verifies it can invoke it; when the function policy grants no such permission the call is refused with AccessDeniedException and the fixed message "Secrets Manager cannot invoke the specified Lambda function. Ensure that the function policy grants access to the principal secretsmanager.amazonaws.com." — and the secret is left unrotated (no new version, AWSCURRENT unchanged). After lambda.AddPermission grants that principal, the identical call succeeds.
Emulator: HTTP 200 with ARN/Name/VersionId, i.e. the rotation is accepted and a version id handed back although no principal may invoke the function.
Why: the permission boundary that protects rotation is not enforced, so a deployment missing the invoke grant looks healthy here and fails in AWS.
Fix: the RotateSecret handler — it must resolve the rotation function's resource policy before accepting the rotation.
Traces: lambda__rds__secretsmanager.rds-rotation-lambda-missing-invoke-permission-denied

## Evidence
All quoted from the behavioral replay report (suite simparity-behavioral-aws-v1), service group lambda__rds__secretsmanager (1/67 passed).

Cause 1 — four checks fail identically:
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDBCluster operation: CreateDBCluster$ManageMasterUserPassword is not implemented in this emulator: it has RDS generate the master user password in Amazon Web Services Secrets Manager and report that secret as MasterUserSecret, and this service mints no such secret. Name MasterUserPassword instead.
(checks lambda-rds-data-api-execute-statement-positive, lambda-rds-data-api-executestatement-positive, lambda-rds-data-api-missing-permission-negative, lambda-rds-wrong-db-cluster-arn-negative)

Cause 2 — lambda__rds__secretsmanager.deploy-cfn-lambda-execution-role-trusts-lambda-service-positive [error]:
botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetFunction operation: Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-xsvc-stk-8421f38b3272-fn

Cause 3 — lambda__rds__secretsmanager.deploy-cfn-lambda-role-wrong-trust-rejected [mismatch]:
AssertionError: expected ROLLBACK_COMPLETE, got 'CREATE_COMPLETE' (the stack's only defect: the execution role's trust policy allows ec2.amazonaws.com instead of lambda.amazonaws.com; the control stack differing only in that principal must deploy).

Cause 4 — lambda__rds__secretsmanager.deploy-cfn-lambda-rds-secretsmanager-stack [mismatch]:
AssertionError: Stack sim2real-xsvc-stk-7cfe6dff606d ended in ROLLBACK_COMPLETE (template: Aurora Serverless v2 cluster + Secrets Manager secret + Lambda).

Cause 5 — lambda__rds__secretsmanager.rds-rotation-lambda-missing-invoke-permission-denied [mismatch]:
AssertionError: expected rotate_secret to be denied for the missing Lambda invoke permission, but it succeeded and returned {'ARN': 'arn:aws:secretsmanager:us-west-2:000000000000:secret:sim2real-behav-rds-secret-c5c6bb8989a0-eJ0GXa', 'Name': 'sim2real-behav-rds-secret-c5c6bb8989a0', 'VersionId': 'f9b30967-f0cb-4aaf-a652-b65ee39245fb', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
The check asserts code == 'AccessDeniedException' and message == "Secrets Manager cannot invoke the specified Lambda function. Ensure that the function policy grants access to the principal secretsmanager.amazonaws.com.", then that VersionIdsToStages is unchanged, then that the same call succeeds after lambda.add_permission grants secretsmanager.amazonaws.com.

## Steps to Reproduce
Build and boot the emulator, then replay the checks in trace_ids.

Manually:
1. rds.create_db_cluster(DBClusterIdentifier=..., Engine='aurora-postgresql', EngineMode='provisioned', MasterUsername=..., ManageMasterUserPassword=True, ...) -> expect a 2xx DBCluster carrying MasterUserSecret.SecretArn, not NotImplemented.
2. Deploy a CloudFormation stack declaring AWS::Lambda::Function; after CREATE_COMPLETE call lambda.get_function(FunctionName=<the stack's function>) -> expect the configuration, not ResourceNotFoundException.
3. Deploy the same stack with the execution role's trust policy naming ec2.amazonaws.com only -> expect ROLLBACK_COMPLETE and no function; with lambda.amazonaws.com -> expect CREATE_COMPLETE.
4. Deploy the Aurora Serverless v2 + secret + Lambda template -> expect CREATE_COMPLETE; if it rolls back, read DescribeStackEvents for the failing resource.
5. Create an RDS-shaped secret and a rotation function WITHOUT calling lambda.add_permission, then secretsmanager.rotate_secret(SecretId=..., RotationLambdaARN=..., RotationRules={'AutomaticallyAfterDays':30}, RotateImmediately=True) -> expect AccessDeniedException with the 'Secrets Manager cannot invoke the specified Lambda function...' message and an unrotated secret; grant the permission and retry -> expect success.

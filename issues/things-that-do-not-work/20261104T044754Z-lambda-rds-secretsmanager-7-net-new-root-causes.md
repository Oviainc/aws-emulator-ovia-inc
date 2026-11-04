Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__rds__secretsmanager.cli-rds-describe-db-proxy-targets lambda__rds__secretsmanager.lambda-rds-connect-via-proxy-positive lambda__rds__secretsmanager.lambda-rds-proxy-missing-permission-denied lambda__rds__secretsmanager.rds-proxy-auth-lambda-missing-permission-denied lambda__rds__secretsmanager.rds-proxy-invokes-lambda-custom-auth-positive lambda__rds__secretsmanager.rds-proxy-lambda-custom-auth-missing-invoke-permission-negative lambda__rds__secretsmanager.deploy-cfn-missing-rds-permission-negative lambda__rds__secretsmanager.deploy-cfn-missing-secretsmanager-permission-negative lambda__rds__secretsmanager.deploy-cfn-rds-lambda-secretsmanager-stack-positive lambda__rds__secretsmanager.deploy-cfn-rds-proxy-lambda-auth-wiring-positive lambda__rds__secretsmanager.lambda-rds-generate-auth-token-positive lambda__rds__secretsmanager.rds-credentials-stored-in-secretsmanager-resource-reference lambda__rds__secretsmanager.rds-secretsmanager-rotation-missing-lambda-permission-negative lambda__rds__secretsmanager.rds-secretsmanager-rotation-updates-credentials-positive lambda__rds__secretsmanager.rds-secretsmanager-rotation-wrong-secret-id-negative
Issue-Key: lambda__rds__secretsmanager.cli-rds-describe-db-proxy-targets|run#43|AssertionError|assert target["RdsResourceId"] == instance_id, (
Issue-Key: rds|RegisterDBProxyTargets|InvalidDBProxyStateFault
Issue-Key: lambda__rds__secretsmanager.rds-proxy-auth-lambda-missing-permission-denied|_wait_for_target_state#28|TimeoutError|raise TimeoutError(f"target for {db_id} on proxy {proxy_name} to reach {terminal_states}: the run budget ended first; not graded")
Issue-Key: lambda__rds__secretsmanager.rds-proxy-invokes-lambda-custom-auth-positive|_wait_for_target_state#21|AssertionError|raise AssertionError(
Issue-Key: lambda__rds__secretsmanager.rds-proxy-lambda-custom-auth-missing-invoke-permission-negative|_error_code#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: lambda__rds__secretsmanager.deploy-cfn-missing-rds-permission-negative|_deploy_cfn#2|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: lambda__rds__secretsmanager.deploy-cfn-missing-secretsmanager-permission-negative|_deploy_cfn#2|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: lambda__rds__secretsmanager.deploy-cfn-rds-lambda-secretsmanager-stack-positive|_deploy_cfn#2|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: lambda__rds__secretsmanager.deploy-cfn-rds-proxy-lambda-auth-wiring-positive|_deploy_cfn#2|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: lambda__rds__secretsmanager.lambda-rds-generate-auth-token-positive|run#29|KeyError|secret_arn = inst["MasterUserSecret"]["SecretArn"]
Issue-Key: lambda__rds__secretsmanager.rds-credentials-stored-in-secretsmanager-resource-reference|_wait_for_instance_available#8|Timeout|resp = rds.describe_db_instances(DBInstanceIdentifier=db_id)
Issue-Key: lambda__rds__secretsmanager.rds-secretsmanager-rotation-missing-lambda-permission-negative|_error_code#17|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: lambda__rds__secretsmanager.rds-secretsmanager-rotation-updates-credentials-positive|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: lambda__rds__secretsmanager.rds-secretsmanager-rotation-wrong-secret-id-negative|_poll_until#30|Timeout|time.sleep(interval_s)
Issue-Commit: 7edae7e43dd2ef932ae2dbfd8dbf1e7a775f7569

# lambda__rds__secretsmanager: 7 net-new root causes — a DB proxy never leaves 'creating' so no target can be registered, DescribeDBProxyTargets reports the wrong RdsResourceId, cross-VPC proxy subnets are accepted, RDS+Lambda+secret stacks roll back, ManageMasterUserPassword mints no secret, and rotation is neither authorized nor executed

> _Note: this issue was identified against the code base at commit `7edae7e43dd2ef932ae2dbfd8dbf1e7a775f7569`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBProxyTargets

## Description
Seven distinct net-new root causes behind the unowned lambda__rds__secretsmanager failures. The checks the report marks OWNED (Lambda Invoke, the 5-cause and 2-cause lambda__rds__secretsmanager issues, the CFN AWS::Lambda::Function issue) and the ModifyDBProxy scaffold stub are skipped here.

Cause 1 of 7: DescribeDBProxyTargets reports RdsResourceId as the internal DbiResourceId instead of the target's DB instance identifier.
AWS: Smithy AmazonRDSv19 DBProxyTarget$RdsResourceId — "The identifier representing the target. It can be the instance identifier for an RDS DB instance, or the cluster identifier for an Aurora DB cluster." For a registered RDS_INSTANCE target it is therefore the DBInstanceIdentifier the caller passed to RegisterDBProxyTargets.
Emulator: it returns the opaque resource id of the form 'db-0000000000000000000000000N' (the DbiResourceId) rather than the instance identifier.
Why: a client matching proxy targets back to the instances it registered cannot — the value never equals any identifier it knows, so target-to-instance correlation and any per-target health lookup keyed on the identifier break.
Fix: the DescribeDBProxyTargets handler / the DBProxyTarget projection in the RDS state module must emit the registered target's DB instance (or cluster) identifier in RdsResourceId.
Traces: lambda__rds__secretsmanager.cli-rds-describe-db-proxy-targets

Cause 2 of 7: a created DB proxy never leaves Status 'creating', so RegisterDBProxyTargets is refused forever and no target ever becomes AVAILABLE.
AWS: after CreateDBProxy the proxy settles to Status 'available' in bounded time; RegisterDBProxyTargets then succeeds and the target's TargetHealth reaches State AVAILABLE, which is what every proxy data-plane flow waits for.
Emulator: the proxy stays in 'creating' indefinitely. RegisterDBProxyTargets is refused with InvalidDBProxyStateFault and the message "DB proxy <proxy-name> is in state creating and cannot take a target registration" (the proxy name is taken from the request; the rest is fixed text), and the checks that poll for a target in {AVAILABLE} either see no target at all (last observed=None) or spend the whole budget.
Why: RDS Proxy is unusable end to end — no target can be registered, so nothing can connect through a proxy and no proxy-scoped permission boundary can be exercised.
Fix: the CreateDBProxy handler / proxy lifecycle in the RDS state module must advance the proxy to 'available', and the target registration path must then produce a target whose TargetHealth reaches AVAILABLE.
Traces: lambda__rds__secretsmanager.lambda-rds-connect-via-proxy-positive, lambda__rds__secretsmanager.lambda-rds-proxy-missing-permission-denied, lambda__rds__secretsmanager.rds-proxy-auth-lambda-missing-permission-denied, lambda__rds__secretsmanager.rds-proxy-invokes-lambda-custom-auth-positive

Cause 3 of 7: CreateDBProxy accepts a VpcSubnetIds set whose subnets live in two different VPCs.
AWS: CreateDBProxy validates the subnet set synchronously and refuses it with code InvalidParameterCombination and the message "Subnets must belong to only one VPC but found multiple VPCs: [<vpc-id>, <vpc-id>]" — the sentence prefix "Subnets must belong to only one VPC but found multiple VPCs: " is fixed text and the bracketed VPC ids come from the subnets in the request (AWS does not order them stably); no proxy object is created, so DescribeDBProxies for that name answers DBProxyNotFoundFault.
Emulator: the call returns HTTP 200 and creates the proxy (DBProxy with Status 'creating', the requested Auth/RoleArn/VpcSubnetIds, an endpoint and timestamps), so the request AWS refuses leaves a live resource behind.
Why: a template or script with subnets spanning two VPCs — a real misconfiguration — deploys cleanly here and fails against AWS, and the negative control that proves the refusal creates nothing cannot run.
Fix: the CreateDBProxy handler's request validation (resolve the subnets against EC2 state and refuse before any proxy record is written).
Traces: lambda__rds__secretsmanager.rds-proxy-lambda-custom-auth-missing-invoke-permission-negative

Cause 4 of 7: every CloudFormation stack in this group that declares RDS + Lambda + a Secrets Manager secret ends ROLLBACK_COMPLETE instead of CREATE_COMPLETE.
AWS: the stacks reach CREATE_COMPLETE and the deployed function then reads the secret and reaches the database; the negative variants are expected to complete too, their defect being an IAM grant that is only observable at call time.
Emulator: the deploy helper raises on the terminal status 'ROLLBACK_COMPLETE' for four different templates (recorded behavior only — these checks do not capture the failing resource's reason, so the provider at fault must be read from DescribeStackEvents; causes 2, 3 and 5 of this issue plus the already-filed AWS::Lambda::Function provider gap are the likely contributors). The already-open 20261029T195009Z-lambda-rds-secretsmanager-5-net-new-root-causes.md records the same rollback for a different template/check and does not carry these four.
Why: the entire cross-service deployment path is unavailable, and the permission-boundary negatives built on top of it never reach their subject.
Fix: whichever resource provider fails in these stacks — start from DescribeStackEvents for the rolled-back stack and the AWS::RDS::DBInstance/DBProxy, AWS::Lambda::Function and AWS::SecretsManager::Secret providers.
Traces: lambda__rds__secretsmanager.deploy-cfn-missing-rds-permission-negative, lambda__rds__secretsmanager.deploy-cfn-missing-secretsmanager-permission-negative, lambda__rds__secretsmanager.deploy-cfn-rds-lambda-secretsmanager-stack-positive, lambda__rds__secretsmanager.deploy-cfn-rds-proxy-lambda-auth-wiring-positive

Cause 5 of 7: CreateDBInstance with ManageMasterUserPassword=true returns no MasterUserSecret, and no RDS-owned secret is created.
AWS: Smithy AmazonRDSv19 models DBInstance$MasterUserSecret: MasterUserSecret {SecretArn, SecretStatus (creating|active|rotating|impaired), KmsKeyId}. With ManageMasterUserPassword=true RDS mints a secret it owns holding {"username","password"}, reports its ARN on the instance, advances SecretStatus to 'active', and that secret's DescribeSecret reports OwningService="rds" and a tag aws:rds:primaryDBInstanceArn equal to the instance ARN.
Emulator: the instance response carries no MasterUserSecret member at all, so reading inst["MasterUserSecret"]["SecretArn"] raises KeyError and the wait-for-'active' loop never completes. The sibling filing 20261024T030130Z-kms-rds-secretsmanager-rds-managemasteruserpassword-true-returns-no-masterusersecret-so-no.md describes the same defect but carries neither of these checks nor the OwningService/aws:rds:primaryDBInstanceArn consequence.
Why: a client that opts into RDS-managed credentials cannot find them, and nothing can prove the secret is RDS-owned or bound to the instance.
Fix: the CreateDBInstance handler must mint the managed secret through Secrets Manager state (OwningService="rds", aws:rds:primaryDBInstanceArn tag) and the DBInstance projection must emit MasterUserSecret.
Traces: lambda__rds__secretsmanager.lambda-rds-generate-auth-token-positive, lambda__rds__secretsmanager.rds-credentials-stored-in-secretsmanager-resource-reference

Cause 6 of 7: RotateSecret succeeds although the rotation function's resource policy grants secretsmanager.amazonaws.com no invoke permission.
AWS: RotateSecret resolves the RotationLambdaARN and verifies it may invoke it; with no such grant the call is refused with AccessDeniedException and the fixed message "Secrets Manager cannot invoke the specified Lambda function. Ensure that the function policy grants access to the principal secretsmanager.amazonaws.com.", and the secret is left unrotated (no new version, AWSCURRENT unchanged). After lambda:AddPermission grants that principal the identical call succeeds.
Emulator: HTTP 200 with ARN/Name/VersionId, i.e. a rotation is accepted and a version id handed back although no principal may invoke the function. The already-open 20261029T195009Z-lambda-rds-secretsmanager-5-net-new-root-causes.md files the same cause for a different check and does not carry this one.
Why: the permission boundary protecting rotation is unenforced, so a deployment missing the invoke grant looks healthy here and fails in AWS.
Fix: the RotateSecret handler must resolve the rotation function's resource policy before accepting the rotation.
Traces: lambda__rds__secretsmanager.rds-secretsmanager-rotation-missing-lambda-permission-negative

Cause 7 of 7: a started rotation never runs, so neither the credential replacement nor the wrong-secret-id refusal that follows it can be observed.
AWS: an accepted rotation invokes the rotation function through createSecret/setSecret/testSecret/finishSecret, a new version becomes AWSCURRENT with the new credential, and a rotation addressed at a SecretId the caller does not own is refused rather than silently accepted.
Emulator: both checks hang to the 60 s harness cap waiting for a rotation effect that never arrives. Related open issue 20261029T210641Z-lambda-rds-secretsmanager-2-net-new-root-causes.md records "a started secret rotation never runs the rotation Lambda (AWSCURRENT never advances)" for other checks and does not carry these two.
Why: any flow that rotates a database credential and then reconnects with it blocks forever.
Fix: the RotateSecret handler's rotation state machine (invoke the function per step, advance AWSCURRENT) — the same component as cause 6.
Traces: lambda__rds__secretsmanager.rds-secretsmanager-rotation-updates-credentials-positive, lambda__rds__secretsmanager.rds-secretsmanager-rotation-wrong-secret-id-negative

## Evidence
All quoted from the behavioral replay report (suite simparity-behavioral-aws-v1, service group lambda__rds__secretsmanager 4/67 passed).

Cause 1 — [mismatch] cli-rds-describe-db-proxy-targets:
AssertionError: expected RdsResourceId='sim2real-proxy-ee83c78e8808-db', got 'db-00000000000000000000000001'

Cause 2 — [error] lambda-rds-connect-via-proxy-positive and [error] lambda-rds-proxy-missing-permission-denied:
botocore.errorfactory.InvalidDBProxyStateFault: An error occurred (InvalidDBProxyStateFault) when calling the RegisterDBProxyTargets operation: DB proxy sim2real-proxy-d14652ef404f is in state creating and cannot take a target registration
[mismatch] rds-proxy-invokes-lambda-custom-auth-positive: AssertionError: target for sim2real-behav-dbi-53a0bc6b1b5f on proxy sim2real-behav-proxy-5b4af7e57251 never reached {'AVAILABLE'} (last observed=None)
[error] rds-proxy-auth-lambda-missing-permission-denied: TimeoutError: target for sim2real-behav-dbi-5c0eb9100c71 on proxy sim2real-behav-proxy-good-7e385d45a302 to reach {'AVAILABLE'}: the run budget ended first; not graded

Cause 3 — [mismatch] rds-proxy-lambda-custom-auth-missing-invoke-permission-negative:
AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'DBProxy': {'DBProxyName': 'sim2real-rdsproxy-bad-d40f9b680404', 'Status': 'creating', 'EngineFamily': 'MYSQL', 'VpcSubnetIds': ['subnet-…03e', 'subnet-…04c'], …}, 'ResponseMetadata': {'HTTPStatusCode': 200, …}}
The check asserts code == 'InvalidParameterCombination', message.startswith("Subnets must belong to only one VPC but found multiple VPCs: ") with both VPC ids named, then that DescribeDBProxies for that name answers DBProxyNotFoundFault.

Cause 4 — four checks fail identically:
AssertionError: Stack sim2real-xsvc-stk-04f978d5e191 ended in ROLLBACK_COMPLETE (deploy-cfn-missing-rds-permission-negative)
AssertionError: Stack sim2real-xsvc-stk-a5757ed073c9 ended in ROLLBACK_COMPLETE (deploy-cfn-missing-secretsmanager-permission-negative)
AssertionError: Stack sim2real-xsvc-stk-e5f83be484f9 ended in ROLLBACK_COMPLETE (deploy-cfn-rds-lambda-secretsmanager-stack-positive)
AssertionError: Stack sim2real-xsvc-stk-ed262ae8373e ended in ROLLBACK_COMPLETE (deploy-cfn-rds-proxy-lambda-auth-wiring-positive)

Cause 5 — [error] lambda-rds-generate-auth-token-positive:
secret_arn = inst["MasterUserSecret"]["SecretArn"] -> KeyError: 'MasterUserSecret'
[mismatch] rds-credentials-stored-in-secretsmanager-resource-reference: Check timed out after 60.0s — its helper polls until the instance is 'available' AND MasterUserSecret.SecretStatus == 'active', then asserts DescribeSecret OwningService == 'rds' and the aws:rds:primaryDBInstanceArn tag equals the instance ARN.

Cause 6 — [mismatch] rds-secretsmanager-rotation-missing-lambda-permission-negative:
AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ARN': 'arn:aws:secretsmanager:us-west-2:000000000000:secret:sim2real-rot-secret-b3539ce1e86f-nxQxwU', 'Name': 'sim2real-rot-secret-b3539ce1e86f', 'VersionId': '20d5b9e1-…', 'ResponseMetadata': {'HTTPStatusCode': 200, …}}

Cause 7 — Check timed out after 60.0s for rds-secretsmanager-rotation-updates-credentials-positive and rds-secretsmanager-rotation-wrong-secret-id-negative.

## Steps to Reproduce
Build and boot the emulator, then replay the checks in trace_ids. Manually:
1. rds.create_db_instance(...); rds.create_db_proxy(...); poll describe_db_proxies -> expect Status 'available'; observed: 'creating' forever. Then register_db_proxy_targets(DBProxyName=…, DBInstanceIdentifiers=[<id>]) -> expect 200; observed InvalidDBProxyStateFault.
2. After a successful registration, describe_db_proxy_targets -> expect RdsResourceId == the DBInstanceIdentifier; observed the 'db-0000…' resource id.
3. create_db_proxy with VpcSubnetIds drawn from two different VPCs -> expect InvalidParameterCombination "Subnets must belong to only one VPC but found multiple VPCs: [vpc-a, vpc-b]" and DescribeDBProxies answering DBProxyNotFoundFault; observed 200 and a live proxy.
4. Deploy each of the four templates (RDS instance/proxy + Lambda + secret) -> expect CREATE_COMPLETE; observed ROLLBACK_COMPLETE (read DescribeStackEvents for the failing resource).
5. create_db_instance(ManageMasterUserPassword=True) -> expect MasterUserSecret {SecretArn, SecretStatus, KmsKeyId} and a Secrets Manager secret with OwningService 'rds' and tag aws:rds:primaryDBInstanceArn; observed no MasterUserSecret member.
6. Create a secret and a rotation function WITHOUT lambda.add_permission, then secretsmanager.rotate_secret(..., RotateImmediately=True) -> expect AccessDeniedException "Secrets Manager cannot invoke the specified Lambda function. Ensure that the function policy grants access to the principal secretsmanager.amazonaws.com."; observed 200. Grant the permission and retry -> expect the rotation to run and AWSCURRENT to advance; observed nothing.

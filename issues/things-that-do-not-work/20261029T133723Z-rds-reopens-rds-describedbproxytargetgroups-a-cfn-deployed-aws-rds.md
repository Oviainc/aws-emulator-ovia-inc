Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.deploy-rds-proxy-cfn-routes-to-target
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBProxyTargetGroups|DBProxyNotFoundFault
Issue-Commit: 9da197090f8814b77d0e74d6f97f3c6f4f69ffc7

# REOPENS rds DescribeDBProxyTargetGroups: a CFN-deployed AWS::RDS::DBProxy reaches CREATE_COMPLETE but RDS never holds the proxy, so the call is refused DBProxyNotFoundFault (1 cause)

> _Note: this issue was identified against the code base at commit `9da197090f8814b77d0e74d6f97f3c6f4f69ffc7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBProxyTargetGroups

## Description
REOPENS 20261029T071525Z-rds-describedbproxytargetgroups-stub.md: its fix left the call refused.

Cause 1 of 1: a CloudFormation stack that declares an `AWS::RDS::DBProxy` (plus its `AWS::RDS::DBProxyTargetGroup`) completes successfully and `!Ref Proxy` yields the requested proxy name, yet RDS's own proxy store never gains that proxy — so every subsequent RDS proxy read for the name CloudFormation just reported as created is refused as if the proxy had never existed. Failing check: rds.deploy-rds-proxy-cfn-routes-to-target.

AWS: `AWS::RDS::DBProxy` is provisioned by CloudFormation through the RDS control plane, so once the stack reaches CREATE_COMPLETE the proxy named by `DBProxyName` is an RDS resource of the account/Region the stack was created in. Smithy (com.amazonaws.rds#AmazonRDSv19): DescribeDBProxyTargetGroups takes `DBProxyName` (required, len 1..63, pattern ^[a-zA-Z](?:-?[a-zA-Z0-9]+)*$) and returns `TargetGroups`; `DBProxyNotFoundFault` (HTTP 404, awsQueryError code `DBProxyNotFoundFault`) is documented as "The specified proxy name doesn't correspond to a proxy owned by your Amazon Web Services account in the specified Amazon Web Services Region" — i.e. it is reserved for a name that is NOT an existing proxy. For a proxy that does exist, AWS answers 200 with the single `default` target group (Status `available`), and DescribeDBProxyTargets then reports the registered instance with `Type` RDS_INSTANCE and `TargetHealth.State` AVAILABLE. Recorded behavior: the stack created without rolling back and its `ProxyName` output equalled the `DBProxyName` parameter the template was given.

Emulator: the stack reports success and returns the proxy name as its output, but `DescribeDBProxyTargetGroups(DBProxyName=<that name>)` answers HTTP 404 `DBProxyNotFoundFault` with message `DBProxy <db_proxy_name> not found.` (fixed text `DBProxy ` ... ` not found.`; the variable part is the `DBProxyName` taken verbatim from the request — in the replay it was the generated name `sim2real-proxy-d3286eab1162`, which is a per-run value, not part of the requirement). The handler itself (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/describe_db_proxy_target_groups.rs, the lookup at `store.db_proxies.get(&(region, input.db_proxy_name))`) is implemented and its 404 is correct for an absent proxy: the absent proxy is the defect. Hypothesis: CloudFormation's provisioning of the RDS proxy resource types is a no-op that mints a physical resource id and marks the resource CREATE_COMPLETE without ever writing RDS's `db_proxies` store (or writes it under a different Region/name key than the signed request Region used for the read).

Why: a client that deploys an RDS Proxy by CloudFormation — the documented way to do it — gets a stack it is told succeeded and a proxy name it cannot use. Every downstream RDS proxy call (DescribeDBProxies, DescribeDBProxyTargetGroups, DescribeDBProxyTargets, RegisterDBProxyTargets, ModifyDBProxy, DeleteDBProxy) is refused 404 for a resource the control plane says exists, so proxy routing/health verification and any teardown of the proxy are impossible, and the stack's own delete path has nothing to delete. Infrastructure-as-code around RDS Proxy cannot be exercised at all.

Fix: CloudFormation's stack provisioner for the RDS resource types — the `AWS::RDS::DBProxy` and `AWS::RDS::DBProxyTargetGroup` resource providers — must create the proxy in RDS's service state (same store `db_proxies`, keyed by the request Region) with its single `default` target group in `available` status and the declared `DBInstanceIdentifiers` registered as RDS_INSTANCE targets reaching AVAILABLE health, and must FAIL the resource (rolling the stack back) if it cannot. Equivalently, the resource provider must go through RDS's CreateDBProxy/RegisterDBProxyTargets state transitions rather than reporting CREATE_COMPLETE for a resource it did not materialise. No change is needed in the DescribeDBProxyTargetGroups handler's 404 path.

Traces: rds.deploy-rds-proxy-cfn-routes-to-target. Reads services/service-com-amazonaws-rds-amazonrdsv19/src/ops/describe_db_proxy_target_groups.rs lines 185-197 (the Region+name lookup that misses). Related but distinct open issue: 20261026T235825Z-lambda-rds-secretsmanager-an-rds-proxy-created-with.md (RequireTLS not enforced on a proxy endpoint) — it does not cover a CFN-deployed proxy being absent from RDS state.

## Evidence
Replaying rds.deploy-rds-proxy-cfn-routes-to-target:

Expected (AWS): after the stack reaches CREATE_COMPLETE, `rds.describe_db_proxy_target_groups(DBProxyName=<ProxyName output>)` returns HTTP 200 with `TargetGroups[0].Status == "available"`, and `rds.describe_db_proxy_targets(DBProxyName=<ProxyName>)` reports the backing instance with `Type == "RDS_INSTANCE"` and `TargetHealth.State == "AVAILABLE"`.

Actual (emulator): the stack created and `_stack_outputs` yielded `ProxyName`, then the very next call failed:

  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.DBProxyNotFoundFault: An error occurred (DBProxyNotFoundFault) when calling the DescribeDBProxyTargetGroups operation: DBProxy sim2real-proxy-d3286eab1162 not found.

Status 404, error code `DBProxyNotFoundFault`, message `DBProxy <DBProxyName> not found.` — fixed text `DBProxy `/` not found.`, variable part the `DBProxyName` echoed from the request (`sim2real-proxy-d3286eab1162` is a per-run generated name). The check never reached its target-group or target assertions.

The previously fixed issue 20261029T071525Z-rds-describedbproxytargetgroups-stub.md (Issue-Trace-Ids: rds.deploy-rds-proxy-cfn-routes-to-target) implemented the operation out of its 501 stub, but the same recorded check is still refused — now with a 404 instead of a 501 — because the proxy CloudFormation claims to have created is not in RDS state.

## Steps to Reproduce
1. Boot the emulator (rds, cloudformation, ec2, iam, secretsmanager).
2. Create a mysql db.t3.micro DB instance in a DB subnet group spanning two AZs and wait for it to become available; create a Secrets Manager secret holding its master credentials and an IAM role trusted by rds.amazonaws.com with secretsmanager:GetSecretValue + kms:Decrypt.
3. CreateStack with a template declaring `AWS::RDS::DBProxy` (EngineFamily MYSQL, RoleArn, VpcSubnetIds, VpcSecurityGroupIds, RequireTLS false, Auth[0]={AuthScheme: SECRETS, IAMAuth: DISABLED, SecretArn}) and `AWS::RDS::DBProxyTargetGroup` (DBProxyName: !Ref Proxy, TargetGroupName default, DBInstanceIdentifiers: [the instance]), with output `ProxyName: !Ref Proxy`; wait for CREATE_COMPLETE.
4. Call `DescribeDBProxyTargetGroups(DBProxyName=<ProxyName output>)`.
   Observe HTTP 404 `DBProxyNotFoundFault` "DBProxy <name> not found." instead of 200 with the `default` target group in `available` status.
5. Call `DescribeDBProxies` / `DescribeDBProxyTargets` with the same name and observe the proxy is absent from RDS state entirely, despite the stack having succeeded.

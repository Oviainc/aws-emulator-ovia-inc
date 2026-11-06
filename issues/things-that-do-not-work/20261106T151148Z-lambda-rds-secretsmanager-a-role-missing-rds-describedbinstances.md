Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__rds__secretsmanager.deploy-cfn-lambda-rds-missing-permission-negative
Issue-Key: lambda__rds__secretsmanager.deploy-cfn-lambda-rds-missing-permission-negative|run#7|AssertionError|assert denied["code"] == "AccessDenied", (
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# lambda__rds__secretsmanager: a role missing rds:DescribeDBInstances yields the placeholder error code 'Unknown' instead of AccessDenied

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBInstances

## Description
One root cause behind 1 failing check.

Cause 1 of 1: an in-function RDS call the execution role does not allow fails with an error code that is not an AWS error code.
AWS: a handler calling DescribeDBInstances with an execution role that does not grant `rds:DescribeDBInstances` is refused with HTTP 403 and error code `AccessDenied` (the RDS query-protocol code; `AccessDeniedException` on JSON services), message `User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: rds:DescribeDBInstances because no identity-based policy allows the rds:DescribeDBInstances action` — fixed text with the account, role name, function name and action taken from the request and the role. The code is always a real, documented AWS code. Model: RDS DescribeDBInstances is modelled in services/service-com-amazonaws-rds-amazonrdsv19 (DBInstanceNotFoundFault etc.); the authorization refusal is the service-common AccessDenied.
Emulator: the check sees error code `'Unknown'` — a placeholder, not an AWS code — for the deliberately under-permissioned role.
Why: a caller that branches on `e.response['Error']['Code'] == 'AccessDenied'` never matches, so a permissions failure is indistinguishable from any other failure; operators and retry logic have nothing to key on, and the CloudFormation-deployed negative test cannot show that the role is actually insufficient.
Fix: the error serializer for refused in-function AWS SDK calls, and the IAM authorization path behind it: a denial must carry the service's own code `AccessDenied` and the standard not-authorized message, never `Unknown`. The open issues 20261104T044754Z-lambda-rds-secretsmanager-7-net-new-root-causes.md and 20261105T081139Z-lambda-rds-secretsmanager-2-net-new-root-causes-e2a51e.md cover other causes of this service and do not carry this replay key.
Traces: lambda__rds__secretsmanager.deploy-cfn-lambda-rds-missing-permission-negative.

## Evidence
Expected (AWS): HTTP 403, Error.Code `AccessDenied`, Error.Message `User: arn:aws:sts::<account>:assumed-role/<role>/<function> is not authorized to perform: rds:DescribeDBInstances because no identity-based policy allows the rds:DescribeDBInstances action`.

Actual (emulator): AssertionError: expected error code AccessDenied for a role missing rds:DescribeDBInstances, got 'Unknown'.

## Steps to Reproduce
1. Deploy a CFN stack with an RDS instance and a Lambda whose execution-role policy omits rds:DescribeDBInstances.
2. Invoke the function; its handler calls boto3.client('rds').describe_db_instances().
3. Expected: Error.Code AccessDenied with the not-authorized message. Observed: Error.Code 'Unknown'.

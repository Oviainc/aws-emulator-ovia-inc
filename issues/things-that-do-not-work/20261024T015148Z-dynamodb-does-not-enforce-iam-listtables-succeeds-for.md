Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__ec2__iam__lambda__s3__sns__sqs__sts.one-denied-principal-error-shape-per-protocol-family
Issue-Key: dynamodb__ec2__iam__lambda__s3__sns__sqs__sts.one-denied-principal-error-shape-per-protocol-family|_denied_call#18|AssertionError|raise AssertionError(
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# DynamoDB does not enforce IAM: ListTables succeeds for a principal with zero attached permissions instead of AccessDeniedException/400

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTables

## Description
Cause 1 of 1: a request signed by an assumed role with no identity policy whatsoever is served normally by DynamoDB.
AWS: an implicit deny is returned as AccessDeniedException with HTTP status 400 (DynamoDB is plain awsJson1_0, so the body carries a "__type" field and no x-amzn-query-error header, and Content-Type is application/x-amz-json-1.0); the message names the action and resource, e.g. 'User: arn:aws:sts::<acct>:assumed-role/<role>/probe is not authorized to perform: dynamodb:ListTables on resource: arn:aws:dynamodb:<region>:<acct>:table/* because no identity-based policy allows the dynamodb:ListTables action'. Smithy: ListTables models no authorization error, so this is recorded behavior only; the status/code pair is the requirement and the message rule is 'names the action and resource'.
Emulator: the call SUCCEEDS (200) for that principal. The check's own controls prove the credentials are the bare role: iam:ListAttachedRolePolicies and iam:ListRolePolicies both return empty and sts:GetCallerIdentity with the same credentials returns arn:aws:sts::<acct>:assumed-role/<role>/probe.
Why: authorization is a behaviour clients depend on — least-privilege test suites, deny-by-default assertions and any negative IAM test pass against the emulator when they would fail in AWS, which is the worst possible direction for an authorization defect. It also means the per-protocol denial shape (400 + awsJson1_0 __type for DynamoDB, as opposed to 403 for SQS/EC2/IAM/SNS/STS/Lambda/S3) can never be observed.
Fix: request authorization for the DynamoDB data plane — evaluate the signing principal's policies before dispatching the operation, and serialize the denial as awsJson1_0 AccessDeniedException with HTTP 400.
Traces: dynamodb__ec2__iam__lambda__s3__sns__sqs__sts.one-denied-principal-error-shape-per-protocol-family.

## Evidence
AssertionError: dynamodb call unexpectedly SUCCEEDED for a principal with zero attached permissions — raised from `_denied_call(ctx, "dynamodb", denied_creds, lambda c: c.list_tables(Limit=1))`. The check then asserts code == 'AccessDeniedException', status == 400, Content-Type == 'application/x-amz-json-1.0', '"__type"' in body and no x-amzn-query-error header.

## Steps to Reproduce
Replay dynamodb__ec2__iam__lambda__s3__sns__sqs__sts.one-denied-principal-error-shape-per-protocol-family. Minimal repro: iam:CreateRole trusted by the account with no attached or inline policies, sts:AssumeRole into it, verify with sts:GetCallerIdentity, then call dynamodb:ListTables(Limit=1) with those credentials.

Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.iam-role-missing-lambda-trust-negative apigateway__dynamodb__lambda__logs.iam-role-wrong-trust-principal-lambda-negative
Issue-Key: apigateway__dynamodb__lambda__logs.iam-role-missing-lambda-trust-negative|_retry_negative_create#10|AssertionError|raise AssertionError(
Issue-Key: apigateway__dynamodb__lambda__logs.iam-role-wrong-trust-principal-lambda-negative|_client_error#9|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded: {result!r}")
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Lambda CreateFunction accepts a role whose trust policy does not allow lambda.amazonaws.com to assume it

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Cause 1 of 1: CreateFunction does not validate the execution role's trust policy.
AWS: CreateFunction resolves the Role ARN and verifies that its trust policy allows lambda.amazonaws.com to sts:AssumeRole; a role trusting only the account root, or trusting a different service principal, is refused with HTTP 400 InvalidParameterValueException. Smithy (lambda): CreateFunction declares InvalidParameterValueException (client, httpError 400) among its errors, and Role is a required member constrained to an IAM role ARN pattern.
Emulator: returns HTTP 201 with a full FunctionConfiguration (State=Pending, StateReasonCode=Creating) for both an un-assumable root-only-trust role and a role whose trust principal is another service.
Why: a deployment with a mis-scoped trust policy appears to succeed, so the misconfiguration is only discovered when the function is invoked (where real AWS would never have created it). Negative-path tests of IAM wiring cannot be written against the emulator.
Fix: the role-resolution/validation step of the CreateFunction handler in the lambda service crate (read the referenced role's AssumeRolePolicyDocument and refuse when lambda.amazonaws.com is not permitted).
Traces: apigateway__dynamodb__lambda__logs.iam-role-missing-lambda-trust-negative, apigateway__dynamodb__lambda__logs.iam-role-wrong-trust-principal-lambda-negative.

## Evidence
AssertionError: expected CreateFunction against a root-only-trust role to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'FunctionName': 'sim2real-behav-badtrust-fn-...', 'Role': 'arn:aws:iam::000000000000:role/sim2real-behav-badtrust-...', 'State': 'Pending', 'StateReasonCode': 'Creating', ...}; AssertionError: expected a ClientError, but the call succeeded: {'HTTPStatusCode': 201, ... 'Role': 'arn:aws:iam::000000000000:role/sim2real-lambda-badtrust-...'}. Expected: HTTP 400, error code InvalidParameterValueException (modeled on CreateFunction).

## Steps to Reproduce
iam.create_role with AssumeRolePolicyDocument trusting only {"AWS": "arn:aws:iam::<acct>:root"} (or Service: ec2.amazonaws.com), then lambda.create_function(Role=<that role arn>, Runtime=python3.12, Handler=index.handler, Code={'ZipFile': ...}). Observe 201 instead of 400 InvalidParameterValueException.

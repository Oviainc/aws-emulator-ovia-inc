Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudtrail.create-stack-with-a-role-cloudformation-cannot-assume-is-rejected-and-audited
Issue-Key: cloudformation__cloudtrail.create-stack-with-a-role-cloudformation-cannot-assume-is-rejected-and-audited|run#33|AssertionError|raise AssertionError(
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# CreateStack accepts a RoleARN CloudFormation cannot assume and returns a StackId instead of refusing it with ValidationError

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CreateStack performs no assumability check on the `RoleARN` it is handed, so a stack is created with a service role whose trust policy does not name cloudformation.amazonaws.com.

AWS: recorded behavior (the check's own measurement, us-west-2, 2026-10-01): `CreateStack` with a RoleARN CloudFormation cannot assume is refused with HTTP 400, SDK error code `ValidationError` and message `Role <arn> is invalid or cannot be assumed`. (Smithy: CloudFormation speaks awsQuery; CreateStack's modeled errors are AlreadyExistsException, InsufficientCapabilitiesException, LimitExceededException, TokenAlreadyExistsException — the refusal above is the service's awsQuery `ValidationError`, i.e. the generic sender-fault code, and the message is built from the request's own RoleARN, so naming that ARN is part of the rule.) The same measurement notes a role whose trust DOES name cloudformation.amazonaws.com is also refused for ~10 s after creation while IAM propagates, so the check only asserts the refusal after a later-created sibling role has been accepted.
Emulator: `CreateStack` with a role whose trust policy names only `lambda.amazonaws.com` returns HTTP 200 and a StackId (`arn:aws:cloudformation:<region>:<account>:stack/<name>/<uuid>`), and the stack proceeds; the role is never resolved or evaluated for assumability.
Why: the principal-boundary negative class is silently unreachable. A client that relies on CloudFormation refusing an unassumable service role gets a stack that appears to deploy under a role it cannot actually use, so misconfigured IAM trust is discovered (if at all) much later and as a provisioning failure rather than as the documented 400 ValidationError; code that asserts on that refusal cannot be tested against the emulator at all.
Fix: the CreateStack handler's request validation in the cloudformation service crate — resolve `RoleARN` in IAM state and check its trust policy admits `cloudformation.amazonaws.com` for sts:AssumeRole before accepting the stack, refusing with the awsQuery `ValidationError` sender fault whose message names the submitted ARN.
Traces: cloudtrail__cloudformation.create-stack-with-a-role-cloudformation-cannot-assume-is-rejected-and-audited.

## Evidence
`AssertionError: CreateStack with a role trusting only lambda.amazonaws.com was ACCEPTED on attempt 1: {'StackId': 'arn:aws:cloudformation:us-west-2:000000000000:stack/sim2real-xsvc-stkbad-<id>/<uuid>', 'OperationId': '...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}` (cloudtrail__cloudformation.create-stack-with-a-role-cloudformation-cannot-assume-is-rejected-and-audited).

The check's recorded AWS oracle (verbatim from its provenance header): "M1 CreateStack with a RoleARN CloudFormation cannot assume fails ValidationError (HTTP 400), message 'Role <arn> is invalid or cannot be assumed'."

## Steps to Reproduce
1. CreateRole with an assume-role trust policy whose only Principal.Service is `lambda.amazonaws.com`.
2. CreateStack(StackName='s-1', TemplateBody=<a template declaring one AWS::SQS::Queue>, RoleARN=<that role's arn>).
   Expect: HTTP 400, code `ValidationError`, message `Role <arn> is invalid or cannot be assumed`.
   Observe: HTTP 200 with a StackId.
3. Repeat with a role whose trust names `cloudformation.amazonaws.com` to confirm the positive control still succeeds.

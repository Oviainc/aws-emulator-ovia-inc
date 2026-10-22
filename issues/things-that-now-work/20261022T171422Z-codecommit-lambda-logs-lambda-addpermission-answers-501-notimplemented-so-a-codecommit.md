Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit__lambda__logs.repository-trigger-invokes-lambda-with-a-payload-naming-the-ref-update
Issue-Key: codecommit__lambda__logs|AddPermission|NotImplemented
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Lambda AddPermission answers 501 NotImplemented, so a CodeCommit trigger can never be authorized to invoke a function

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddPermission

## Description
Cause 1 of 1: lambda AddPermission is an unimplemented scaffold stub, and it is the first call of this check, so the CodeCommit-trigger-to-Lambda path is never exercised.

AWS: lambda AddPermission grants a resource-based policy statement on a function and returns 200 with {Statement} (the JSON of the statement added); it refuses a duplicate StatementId with ResourceConflictException (409) and an unknown function with ResourceNotFoundException (404). Recorded behavior in this check: the statement granting principal codecommit.amazonaws.com with SourceArn = the repository ARN is added, after which a repository trigger whose destinationArn is the function ARN invokes the function directly (no SNS hop) and the invocation is observable in its CloudWatch Logs log group.

Emulator: 'An error occurred (NotImplemented) when calling the AddPermission operation: operation AddPermission is scaffolded but not implemented in this emulator' — HTTP 501 where AWS answers 200.

Why: no cross-service invocation can be authorized at all. Every integration that hangs a Lambda off another service (CodeCommit triggers here, but equally S3, SNS or EventBridge targets) requires AddPermission first, so a client cannot build the wiring, and the downstream payload/log assertions (the ref-update payload naming refs/heads/<branch>) are unreachable.

Fix: the scaffold stub for AddPermission in the lambda crate (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/add_permission.rs) plus the resource-policy state it writes, which the trigger invocation path must consult.

Traces: codecommit__lambda__logs.repository-trigger-invokes-lambda-with-a-payload-naming-the-ref-update.

## Evidence
Expected: lambda.add_permission(FunctionName=F, StatementId='codecommit-invoke', Action='lambda:InvokeFunction', Principal='codecommit.amazonaws.com', SourceArn=<repo arn>) returns 200 with a Statement member.
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AddPermission operation: operation AddPermission is scaffolded but not implemented in this emulator.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. Create a Lambda function and a CodeCommit repository.
3. lambda.add_permission(FunctionName=F, StatementId='codecommit-invoke', Action='lambda:InvokeFunction', Principal='codecommit.amazonaws.com', SourceArn=<repository arn>) -> expect 200 {'Statement': '...'}; observe 501 NotImplemented.
4. Re-replay the check id in trace_ids.

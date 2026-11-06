Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__iam__lambda__s3.sam-policies-shorthand-expansion-deny-probed
Issue-Key: cloudformation__dynamodb__iam__lambda__s3.sam-policies-shorthand-expansion-deny-probed|_execute_change_set#22|AssertionError|assert status == "CREATE_COMPLETE", f"stack {stack_name} ended in {status!r}"
Issue-Commit: cc6137e930728edc173d6e704b5a7c1716e7fffd

# cloudformation__dynamodb__iam__lambda__s3: executing the SAM (AUTO_EXPAND) change set rolls the stack back, so the Policies-shorthand stack never deploys

> _Note: this issue was identified against the code base at commit `cc6137e930728edc173d6e704b5a7c1716e7fffd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteChangeSet

## Description
One net-new behavioral root cause of service `cloudformation__dynamodb__iam__lambda__s3`, grounded in one failing behavioral check.

Cause 1 of 1: a change set created from a SAM template (CAPABILITY_AUTO_EXPAND) that declares an AWS::Serverless::Function with `Policies:` shorthand plus a table and a bucket cannot be executed — ExecuteChangeSet drives the stack to ROLLBACK_COMPLETE instead of CREATE_COMPLETE.
AWS: recorded behavior only (Smithy cloudformation ExecuteChangeSet returns an empty output and the outcome is observed on DescribeStacks StackStatus; Stack.StackStatus is the enum carrying CREATE_COMPLETE / ROLLBACK_COMPLETE). On AWS, the transform expands the shorthand policy names into the role's inline documents, the resources (function, execution role, DynamoDB table, S3 bucket) all create, and the stack reaches CREATE_COMPLETE; the probe function is then invokable. Any resource that genuinely failed would carry its own CREATE_FAILED reason and the stack-level aggregate `The following resource(s) failed to create: [<logical ids>]. Rollback requested by user.`
Emulator: DescribeStacks reports `ROLLBACK_COMPLETE` after ExecuteChangeSet, so no resource of the expanded SAM template survives. (This run observed only the terminal stack status; the per-resource CREATE_FAILED reason is the diagnostic the fixer should read first, and it must identify a real resource defect rather than an emulator limitation.)
Why: the whole SAM `Policies:` surface is unreachable — a client cannot deploy the overwhelmingly common SAM shape (function + SimpleTable/bucket + shorthand policies) at all, and because the stack rolls back it is left with nothing and only an aggregate reason to triage.
Fix: the resource provisioning of the SAM-expanded template in the cloudformation change-set execution path — whichever expanded resource (the generated IAM role / its inline policies, the table, the bucket, or the function) fails to create must create, and the failing resource's reason must name the real cause.
Traces: cloudformation__dynamodb__iam__lambda__s3.sam-policies-shorthand-expansion-deny-probed. The open issue 20261105T085348Z-cloudformation-dynamodb-iam-lambda-s3-the-sam-policies.md covers this check's LATER step (the probe Invoke faulting with Runtime.ImportModuleError / no boto3) and carries that replay key; the check now fails strictly earlier, at ExecuteChangeSet, so that issue does not cover this cause — the stack must deploy before the probe can be reached at all.

## Evidence
cloudformation__dynamodb__iam__lambda__s3.sam-policies-shorthand-expansion-deny-probed [mismatch]:
  AssertionError: stack sim2real-xsvc-stk-<id> ended in 'ROLLBACK_COMPLETE' (raised from _execute_change_set after CreateChangeSet+ExecuteChangeSet on a CAPABILITY_AUTO_EXPAND SAM template).
  Expected (AWS): DescribeStacks StackStatus 'CREATE_COMPLETE' with the SAM transform's expanded function, execution role (inline documents for the shorthand policy names), table and bucket all present. The stack name is per-run and is not part of the requirement.

Smithy consulted: cloudformation ExecuteChangeSet (empty output; outcome read from DescribeStacks), Stack.StackStatus enum (CREATE_COMPLETE / ROLLBACK_COMPLETE), StackEvent.ResourceStatusReason is a free string.

## Steps to Reproduce
1. CreateChangeSet (type CREATE, Capabilities CAPABILITY_IAM + CAPABILITY_AUTO_EXPAND) from a SAM template declaring an AWS::Serverless::Function with `Policies:` shorthand entries (e.g. S3ReadPolicy and DynamoDBCrudPolicy), plus the table and bucket they name.
2. Wait for CREATE_COMPLETE on the change set, then ExecuteChangeSet.
3. Poll DescribeStacks: the stack reaches ROLLBACK_COMPLETE instead of CREATE_COMPLETE. Read DescribeStackEvents for the expanded resources' CREATE_FAILED reasons to see which expanded resource fails.
4. Re-replay: cloudformation__dynamodb__iam__lambda__s3.sam-policies-shorthand-expansion-deny-probed.

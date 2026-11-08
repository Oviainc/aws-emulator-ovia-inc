Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iot__lambda.cfn-iot-authorizer-active-create-and-delete-with-stack cloudformation__iot__lambda.cfn-iot-authorizer-duplicate-name-rejected
Issue-Key: cloudformation__iot__lambda|ListTagsForResource|NotImplemented
Issue-Key: cloudformation__iot__lambda.cfn-iot-authorizer-duplicate-name-rejected|run#18|AssertionError|assert reason == expected_reason, (
Issue-Commit: 755fa21f4b5f658e5c42cafb46f764c1d29f1673

# cloudformation__iot__lambda: 2 net-new causes — iot ListTagsForResource answers 501 NotImplemented, and a stack whose resource create is refused records the wrong CREATE_FAILED reason

> _Note: this issue was identified against the code base at commit `755fa21f4b5f658e5c42cafb46f764c1d29f1673`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
Two root causes behind the two failing `### cloudformation__iot__lambda` checks; neither check is marked ALREADY FILED or OWNED in the suite report.

Cause 1 of 2: iot `ListTagsForResource` is a scaffold stub, so a CFN-provisioned `AWS::IoT::Authorizer` can never be inspected for its tags.
AWS: `iot:ListTagsForResource` returns `{"tags": [...], "nextToken": ...}` for an existing resource ARN; its modeled errors are `InvalidRequestException`, `InternalFailureException`, `ThrottlingException`, `UnauthorizedException` and `ResourceNotFoundException`. `NotImplemented` is not a modeled error for this operation and 501 is not a status it can answer.
Emulator: every call is refused `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListTagsForResource operation: operation ListTagsForResource is scaffolded but not implemented in this emulator`, which aborts the check before it can assert anything about the authorizer.
Why: a client reading back tags on any IoT resource gets an unmodeled error its SDK cannot classify, and no IoT tag-driven workflow (cost allocation, tag-based access control, CFN drift checks) can run at all.
Fix: implement the iot `ListTagsForResource` operation handler over the service's tag state.
Traces: cloudformation__iot__lambda.cfn-iot-authorizer-active-create-and-delete-with-stack.

Cause 2 of 2: when a stack's resource create is refused by the target service, the stack's CREATE_FAILED reason uses the resource-list wording instead of AWS's validation-failure sentence.
AWS: creating a stack whose `AWS::IoT::Authorizer` names an authorizer that already exists rolls the stack back to `ROLLBACK_COMPLETE`, and the stack's own `CREATE_FAILED` event reason is the fixed sentence `Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.` — the only variable part is the error count. (Recorded behavior for this failure class; the per-resource detail lives on the resource event and in DescribeEvents.)
Emulator: the stack's CREATE_FAILED reason is `The following resource(s) failed to create: [Authorizer]. ` — the generic resource-list wording, which is what AWS uses for a provisioning failure, not for a validation refusal.
Why: a client that reads the stack-level reason to decide whether to call `DescribeEvents` for per-property detail is told the wrong failure class and never fetches the detail that explains the refusal.
Fix: the CloudFormation stack-operation failure recorder must emit the validation-failure reason (with the issue count) on the stack's CREATE_FAILED event when a resource create was refused by validation, keeping the resource-list wording for provisioning failures.
Traces: cloudformation__iot__lambda.cfn-iot-authorizer-duplicate-name-rejected.

## Evidence
Cause 1 — expected: `ListTagsForResource` returns the authorizer's tags. Actual: `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListTagsForResource operation: operation ListTagsForResource is scaffolded but not implemented in this emulator`.

Cause 2 — expected stack CREATE_FAILED reason: `'Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.'`. Actual: `'The following resource(s) failed to create: [Authorizer]. '` (the stack did reach ROLLBACK_COMPLETE).

## Steps to Reproduce
1. Create a Lambda function to act as an IoT custom authorizer target.
2. Cause 1: deploy a stack with `AWS::IoT::Authorizer` (explicit AuthorizerName, Status=ACTIVE) pointing at that function, then call `iot:ListTagsForResource` on the authorizer ARN — it must return the tag list.
3. Cause 2: `iot:CreateAuthorizer` with a name, then create a stack whose `AWS::IoT::Authorizer` declares the same name; wait for `ROLLBACK_COMPLETE` and read the stack's own `CREATE_FAILED` `ResourceStatusReason` — it must be the `Validation failed with 1 error(s)...` sentence.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb.cfn-dynamodb-provisioned-throughput-drift-reports-expected-and-actual cloudformation__dynamodb.cfn-gsi-provisioned-throughput-with-pay-per-request-rejected cloudformation__dynamodb.cfn-resource-existence-dynamodb-table cloudformation__dynamodb.deploy-dynamodb-table-value-constraints-rejected cloudformation__dynamodb.stack-tags-propagate-to-table-without-system-tags-and-update-in-place
Issue-Key: cloudformation__dynamodb.cfn-dynamodb-provisioned-throughput-drift-reports-expected-and-actual|run#11|AssertionError|assert clean["StackDriftStatus"] == "IN_SYNC", (
Issue-Key: cloudformation__dynamodb.cfn-gsi-provisioned-throughput-with-pay-per-request-rejected|run#26|AssertionError|assert message == "Property ProvisionedThroughput can't be used with PAY_PER_REQUEST BillingMode.", (
Issue-Key: cloudformation__dynamodb.cfn-resource-existence-dynamodb-table|run#6|WaiterError|cfn.get_waiter("stack_create_complete").wait(
Issue-Key: cloudformation__dynamodb.deploy-dynamodb-table-value-constraints-rejected|run#12|AssertionError|assert stack_failed == [want_reason], (
Issue-Key: cloudformation__dynamodb.stack-tags-propagate-to-table-without-system-tags-and-update-in-place|run#22|AssertionError|assert tag_map_after == {"sim2real-env": "v2"}, (
Issue-Commit: 545841f07feb57b92e392d55db412e521f655a7b

# cloudformation__dynamodb: 5 net-new causes — fresh stacks detect as DRIFTED, DynamoDB resource-handler messages diverge, CREATE_FAILED reasons empty, stack-tag updates do not propagate

> _Note: this issue was identified against the code base at commit `545841f07feb57b92e392d55db412e521f655a7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Five distinct root causes in the CloudFormation<->DynamoDB resource provider. Each is listed with its own failing check ids.

Cause 1 of 5: DetectStackDrift reports a freshly created stack as DRIFTED.
AWS: immediately after a successful CreateStack, `DetectStackDrift` / `DescribeStackResourceDrifts` report the stack `StackDriftStatus: IN_SYNC` and every resource `IN_SYNC`, because the provider's read-back of the resource equals the template's declared properties. A drift appears only after an out-of-band change. Recorded behavior: the check creates the stack, detects drift, and expects IN_SYNC with 0 drifted resources.
Emulator: the drift detection reports `DRIFTED` with 1 drifted resource on a stack nothing has touched — the provider's read-back of `AWS::DynamoDB::Table` ProvisionedThroughput does not round-trip the template's declared value (notably the normalised/defaulted throughput numbers), so every fresh stack looks drifted.
Why: drift detection is the primary tool for detecting manual tampering. A control that always reports DRIFTED is unusable: real tampering cannot be distinguished from the baseline.
Fix: the drift comparison in the CloudFormation drift handler and the AWS::DynamoDB::Table provider's read-back (property normalisation of ProvisionedThroughput/BillingMode defaults).
Traces: cfn-dynamodb-provisioned-throughput-drift-reports-expected-and-actual.

Cause 2 of 5: the DynamoDB provider's refusal message for GSI throughput under PAY_PER_REQUEST is not AWS's.
AWS: creating an `AWS::DynamoDB::Table` whose BillingMode is PAY_PER_REQUEST while a GlobalSecondaryIndex carries ProvisionedThroughput fails the resource with the DynamoDB service's own ValidationException text, surfaced verbatim as `Resource handler returned message: "..."`. The fixed text AWS uses for this class is `One or more parameter values were invalid: ProvisionedThroughput should not be specified for index: <IndexName> when BillingMode is PAY_PER_REQUEST`; `<IndexName>` is the variable part taken from the request.
Emulator: the handler message is `One or more parameter values were invalid: Neither ReadCapacityUnits nor WriteCapacityUnits can be specified when BillingMode is PAY_PER_REQUEST for index: GsiIndex` — invented fixed text, with the index name appended in a different phrasing.
Why: templates are debugged from the handler message, and CI gates and SAM/CDK error matchers key on it; a reworded message breaks both.
Fix: the AWS::DynamoDB::Table provider's validation, which must relay the DynamoDB CreateTable ValidationException message unchanged.
Traces: cfn-gsi-provisioned-throughput-with-pay-per-request-rejected.

Cause 3 of 5: a plain CreateStack of an existing-resource DynamoDB table template rolls back.
AWS: the template in this check creates an `AWS::DynamoDB::Table` and reaches `CREATE_COMPLETE`; the StackCreateComplete waiter succeeds.
Emulator: the stack reaches `ROLLBACK_COMPLETE`, so the waiter raises `botocore.exceptions.WaiterError: Waiter StackCreateComplete failed: ... matched expected path: "ROLLBACK_COMPLETE"`. The table resource fails to create at all.
Why: the most basic CloudFormation lifecycle for the service's flagship resource type never completes, so nothing downstream of the stack can be exercised.
Fix: the AWS::DynamoDB::Table create path in the CloudFormation provider (and its stack-events reason, which must name the rule it enforced).
Traces: cfn-resource-existence-dynamodb-table.

Cause 4 of 5: a failed stack create publishes no CREATE_FAILED reasons.
AWS: when template property validation fails, the stack's failed-resource events carry a StatusReason; for a multi-error validation AWS emits the fixed text `Validation failed with <N> error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.` where `<N>` is the number of detected errors.
Emulator: `DescribeStackEvents` yields an empty list of CREATE_FAILED reasons (`[]`) — the stack fails with no reason recorded at all.
Why: a stack that fails silently gives an operator nothing to act on; `cfn deploy` and CDK print an empty cause.
Fix: the stack-event recording in the CloudFormation create path — every CREATE_FAILED resource event must carry its ResourceStatusReason.
Traces: deploy-dynamodb-table-value-constraints-rejected.

Cause 5 of 5: a stack-level tag update does not propagate to the table.
AWS: stack tags are applied to every taggable resource in the stack, and an `UpdateStack` that changes a stack tag's value re-tags the resource in place; `dynamodb:ListTagsOfResource` on the table ARN then returns the new value (and no `aws:`-prefixed system tags are added).
Emulator: after the stack tag update, `ListTagsOfResource` on the same table ARN still returns the pre-update value — the new value is never pushed to the resource.
Why: tag-based cost allocation, access control (`aws:ResourceTag` conditions) and inventory all read the resource's tags; they silently keep enforcing the old value.
Fix: the UpdateStack tag-propagation step of the CloudFormation provider for AWS::DynamoDB::Table.
Traces: stack-tags-propagate-to-table-without-system-tags-and-update-in-place.

## Evidence
Cause 1: AssertionError: a freshly created stack must detect as IN_SYNC, got 'DRIFTED' (1 drifted)
Cause 2: AssertionError: unexpected resource handler message 'One or more parameter values were invalid: Neither ReadCapacityUnits nor WriteCapacityUnits can be specified when BillingMode is PAY_PER_REQUEST for index: GsiIndex'
Cause 3: botocore.exceptions.WaiterError: Waiter StackCreateComplete failed: Waiter encountered a terminal failure state: For expression "Stacks[].StackStatus" we matched expected path: "ROLLBACK_COMPLETE" at least once
Cause 4: AssertionError: the stack's CREATE_FAILED reasons were [], expected ['Validation failed with 2 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.']
Cause 5: AssertionError: after the stack tag update, ListTagsOfResource on the same table ARN returned {'sim2real-env': 'v1'}, expected exactly {'sim2real-env': 'v2'} (the tag key and both values come from the request; the requirement is that the post-update read equals the updated value)

## Steps to Reproduce
Cause 1: CreateStack with an AWS::DynamoDB::Table carrying ProvisionedThroughput; wait CREATE_COMPLETE; DetectStackDrift; DescribeStackResourceDrifts -> observe DRIFTED on an untouched stack.
Cause 2: CreateStack with BillingMode PAY_PER_REQUEST and a GlobalSecondaryIndex that sets ProvisionedThroughput; read the failed resource event's message.
Cause 3: CreateStack with the plain DynamoDB-table template of cfn-resource-existence-dynamodb-table and wait with the StackCreateComplete waiter.
Cause 4: CreateStack with a template whose table properties violate two value constraints; DescribeStackEvents and collect CREATE_FAILED ResourceStatusReason values.
Cause 5: CreateStack with stack tag k=v1 and a table; UpdateStack with the same template and stack tag k=v2; dynamodb:ListTagsOfResource on the table ARN.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__s3.cfn-stack-update-dynamodb-table-billing-mode-positive
Issue-Key: cloudformation__dynamodb__s3.cfn-stack-update-dynamodb-table-billing-mode-positive|_update_stack_and_wait#12|AssertionError|raise AssertionError(f"Stack update {stack_name} ended in {final_status}")
Issue-Commit: cc6137e930728edc173d6e704b5a7c1716e7fffd

# cloudformation__dynamodb__s3: an UpdateStack flipping a DynamoDB table's BillingMode to PAY_PER_REQUEST fails and rolls back instead of applying in place

> _Note: this issue was identified against the code base at commit `cc6137e930728edc173d6e704b5a7c1716e7fffd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
One net-new behavioral root cause of service `cloudformation__dynamodb__s3`, grounded in one failing behavioral check.

Cause 1 of 1: a stack update that changes an AWS::DynamoDB::Table from `BillingMode: PROVISIONED` (with ProvisionedThroughput) to `BillingMode: PAY_PER_REQUEST` (with ProvisionedThroughput removed) fails and the stack ends UPDATE_ROLLBACK_COMPLETE instead of UPDATE_COMPLETE.
AWS: BillingMode is an in-place, no-interruption update on AWS::DynamoDB::Table. Smithy (dynamodb) UpdateTable accepts `BillingMode` (enum PROVISIONED | PAY_PER_REQUEST) and `ProvisionedThroughput` on the same input, and returns the updated TableDescription; dropping ProvisionedThroughput while switching to PAY_PER_REQUEST is the documented transition. The stack therefore reaches UPDATE_COMPLETE, DescribeTable afterwards reports BillingModeSummary.BillingMode = PAY_PER_REQUEST with the SAME TableArn and physical id, an item written before the update survives it, and a write after it reads back.
Emulator: the stack update ends `UPDATE_ROLLBACK_COMPLETE` — the table resource's update is rejected, so the billing mode never changes.
Why: switching an existing table to on-demand through CloudFormation is a routine, data-preserving operation; here it is impossible, and the client is additionally exposed to the rollback path (the table is reverted to PROVISIONED) with the update reported as failed even though nothing in the template is invalid.
Fix: the AWS::DynamoDB::Table resource provider's update path in the cloudformation resource-provisioning layer — compute the UpdateTable call for a BillingMode change (and the removal of ProvisionedThroughput that accompanies PAY_PER_REQUEST) as a no-interruption in-place update instead of failing the resource.
Traces: cloudformation__dynamodb__s3.cfn-stack-update-dynamodb-table-billing-mode-positive.

## Evidence
cloudformation__dynamodb__s3.cfn-stack-update-dynamodb-table-billing-mode-positive [mismatch]:
  AssertionError: Stack update sim2real-xsvc-stk-<id> ended in UPDATE_ROLLBACK_COMPLETE.
  Expected (AWS): DescribeStacks StackStatus 'UPDATE_COMPLETE', then DescribeTable reporting BillingModeSummary.BillingMode = 'PAY_PER_REQUEST' with the unchanged TableArn, the pre-update item still readable and a post-update write readable. The stack and table names are per-run and are not part of the requirement.

Smithy consulted: dynamodb UpdateTable input members BillingMode (enum) and ProvisionedThroughput, output TableDescription; DescribeTable -> TableDescription. cloudformation Stack.StackStatus enum (UPDATE_COMPLETE / UPDATE_ROLLBACK_COMPLETE); StackEvent.ResourceStatusReason is a free string.

## Steps to Reproduce
1. CreateStack a template with an AWS::DynamoDB::Table carrying `BillingMode: PROVISIONED` and `ProvisionedThroughput: {ReadCapacityUnits: 5, WriteCapacityUnits: 5}`, keyed on pk; wait for CREATE_COMPLETE and PutItem one item.
2. UpdateStack with the same template except `BillingMode: PAY_PER_REQUEST` and no ProvisionedThroughput.
3. Poll DescribeStacks: the stack reaches UPDATE_ROLLBACK_COMPLETE instead of UPDATE_COMPLETE; read DescribeStackEvents for the table resource's UPDATE_FAILED reason.
4. Re-replay: cloudformation__dynamodb__s3.cfn-stack-update-dynamodb-table-billing-mode-positive.

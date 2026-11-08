Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb.deploy-cfn-dynamodb-table-settings-apply-and-flip-in-place
Issue-Key: cloudformation__dynamodb.deploy-cfn-dynamodb-table-settings-apply-and-flip-in-place|run#23|AssertionError|assert stack["StackStatus"] == "UPDATE_COMPLETE", (
Issue-Commit: f4d58d8ba640bd38bd7af13aafdf9baa78485a99

# cloudformation__dynamodb: turning SSE, PITR and contributor insights off on an AWS::DynamoDB::Table fails the resource update, rolling the stack back instead of applying in place

> _Note: this issue was identified against the code base at commit `f4d58d8ba640bd38bd7af13aafdf9baa78485a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
This issue carries 1 net-new root cause.

Cause 1 of 1: an UpdateStack that flips SSESpecification.SSEEnabled, PointInTimeRecoverySpecification.PointInTimeRecoveryEnabled and ContributorInsightsSpecification.Enabled from true to false on an existing AWS::DynamoDB::Table cannot be applied: the resource update fails and the stack ends UPDATE_ROLLBACK_COMPLETE.
Failing check ids for this cause: cloudformation__dynamodb.deploy-cfn-dynamodb-table-settings-apply-and-flip-in-place.
AWS: recorded behavior only for the CloudFormation resource mapping; the underlying DynamoDB calls are modelled (Smithy: UpdateTable.SSESpecification, UpdateContinuousBackups.PointInTimeRecoverySpecification, UpdateContributorInsights.ContributorInsightsAction). The recording shows all three properties being mutable in place on an existing table: the stack reaches UPDATE_COMPLETE, the resource keeps its physical id (the declared table name) and its TableId, DescribeTable then omits SSEDescription entirely (SSEEnabled=false returns the table to the AWS-owned key), DescribeContinuousBackups reports PointInTimeRecoveryStatus DISABLED, DescribeContributorInsights reports ContributorInsightsStatus DISABLED, the unchanged TableClass and TTL stay as created, and an item written before the update survives it.
Emulator: the update is rejected at the resource. DescribeStacks reports StackStatus 'UPDATE_ROLLBACK_COMPLETE' with StackStatusReason 'The following resource(s) failed to update: [Table]. ' — the create phase of the same template (with all three switches on) succeeded, so only the off-flip fails.
Why: any stack that disables table encryption-with-CMK, PITR or contributor insights is un-updatable: CloudFormation reports the whole change rolled back, so operators cannot turn a setting off through IaC and every later update of the same stack inherits a rollback state rather than the intended configuration.
Fix: the CloudFormation provider for AWS::DynamoDB::Table — the in-place update path for SSESpecification, PointInTimeRecoverySpecification and ContributorInsightsSpecification must map each changed property to its DynamoDB control-plane call (UpdateTable / UpdateContinuousBackups / UpdateContributorInsights) and report UPDATE_COMPLETE without replacement, rather than failing the resource.
Traces: cloudformation__dynamodb.deploy-cfn-dynamodb-table-settings-apply-and-flip-in-place.

## Evidence
Expected: after UpdateStack with SSEEnabled=false, PointInTimeRecoveryEnabled=false, ContributorInsightsSpecification.Enabled=false, DescribeStacks reports StackStatus UPDATE_COMPLETE, the resource's physical id and the table's TableId are unchanged, DescribeTable omits SSEDescription, PITR and contributor insights read DISABLED, TableClass/TTL unchanged and the pre-update item is still readable.

Actual: AssertionError: turning the settings off must update the stack; got 'UPDATE_ROLLBACK_COMPLETE': 'The following resource(s) failed to update: [Table]. '

## Steps to Reproduce
1. CreateStack with one AWS::DynamoDB::Table (PAY_PER_REQUEST, a KEYS_ONLY GSI, SSESpecification SSEEnabled=true, TimeToLiveSpecification enabled, PointInTimeRecoverySpecification enabled, ContributorInsightsSpecification enabled, TableClass STANDARD_INFREQUENT_ACCESS) and wait for CREATE_COMPLETE.
2. PutItem one item into the table.
3. UpdateStack with the same template but SSEEnabled, PointInTimeRecoveryEnabled and ContributorInsightsSpecification.Enabled set to false, and poll DescribeStacks to a terminal update status — it reaches UPDATE_ROLLBACK_COMPLETE with reason 'The following resource(s) failed to update: [Table]. ' instead of UPDATE_COMPLETE.
4. Or replay the check id in trace_ids.

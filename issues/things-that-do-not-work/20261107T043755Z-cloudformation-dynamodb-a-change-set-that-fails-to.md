Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb.cdk-import-mapping-file-adopts-a-table-with-its-item-and-refuses-a-missing-one
Issue-Key: cloudformation__dynamodb.cdk-import-mapping-file-adopts-a-table-with-its-item-and-refuses-a-missing-one|run#26|AssertionError|assert (described_cs["Status"], described_cs["StatusReason"]) == ("FAILED", reason), (
Issue-Commit: 12bd58142e134c788a6e6e722018c3787d666c1f

# cloudformation__dynamodb: a change set that fails to import a missing DynamoDB table reports the wrong StatusReason text

> _Note: this issue was identified against the code base at commit `12bd58142e134c788a6e6e722018c3787d666c1f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeChangeSet

## Description
This issue carries 1 net-new root cause.

Cause 1 of 1: when a resource-import change set names a DynamoDB table that does not exist, the change set correctly reaches Status=FAILED but its StatusReason is a generic resource-not-found sentence instead of CloudFormation's DynamoDB import wording.
Failing check ids for this cause: cloudformation__dynamodb.cdk-import-mapping-file-adopts-a-table-with-its-item-and-refuses-a-missing-one.

AWS: Smithy: DescribeChangeSet returns Status (ChangeSetStatus) and StatusReason (ChangeSetStatusReason); the model does not constrain the text, so this is recorded behavior only. The recording shows that for an Import change (ResourceChange.Action=Import, ResourceType=AWS::DynamoDB::Table) whose PhysicalResourceId does not exist, DescribeChangeSet answers HTTP 200 with Status=FAILED and StatusReason exactly "Table: <table-name> does not exist." — fixed text "Table: " + the physical table name from the import mapping + " does not exist." The CLI surfaces that same line verbatim (twice, in cdk's summary and in node's dump of the uncaught error), so the text is part of the contract, not cosmetic.
Emulator: Status=FAILED is right, but StatusReason reads "Resource of type 'AWS::DynamoDB::Table' with identifier '<table-name>' was not found." The assertion fails on the (Status, StatusReason) pair, and the CLI consequently never prints the expected line.
Why: tooling and operators key on this text. `cdk import` greps the change set's StatusReason to explain the refusal, so a diverging sentence makes the CLI report a different (or empty) diagnosis for a mapping that names a missing table, and any automation matching CloudFormation's documented import failure wording does not match.
Fix: the import-failure path that sets ChangeSetStatusReason when a change set's Import change cannot resolve its physical resource — the DynamoDB table resource handler's not-found reason, in the CloudFormation change-set creation/validation component, must produce "Table: <name> does not exist."
Traces: cloudformation__dynamodb.cdk-import-mapping-file-adopts-a-table-with-its-item-and-refuses-a-missing-one.

## Evidence
Expected (recorded AWS): DescribeChangeSet -> Status='FAILED', StatusReason='Table: <missing-table-name> does not exist.'

Actual (emulator): Status='FAILED', StatusReason="Resource of type 'AWS::DynamoDB::Table' with identifier '<missing-table-name>' was not found." (Changes[0].ResourceChange.Action='Import', ResourceType='AWS::DynamoDB::Table', ExecutionStatus='UNAVAILABLE'). AssertionError on the (Status, StatusReason) comparison.

## Steps to Reproduce
1. CreateStack from a template with no resources (or deploy the CDK app's first stage), then create a change set that imports an AWS::DynamoDB::Table whose PhysicalResourceId names a table that was never created (e.g. `cdk import <stack> --resource-mapping <file>`).
2. ListChangeSets and DescribeChangeSet the FAILED change set and read StatusReason: it is the generic "Resource of type 'AWS::DynamoDB::Table' with identifier '<name>' was not found." instead of "Table: <name> does not exist."
3. Or replay the check id in trace_ids.

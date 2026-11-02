Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb.cdk-import-mapping-file-adopts-a-table-with-its-item-and-refuses-a-missing-one
Issue-Key: cloudformation__dynamodb.cdk-import-mapping-file-adopts-a-table-with-its-item-and-refuses-a-missing-one|run#23|AssertionError|assert len(failed_cs) == 1, f"expected one FAILED change set, got {failed_cs!r}"
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudformation/dynamodb: an import change set that names a nonexistent table never reaches FAILED

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
Cause 1 of 1: a resource-import change set over a missing resource is not failed.
AWS: recorded behavior only — `cdk import` with a mapping file that points at an existing DynamoDB table adopts it (the table and its item survive), while a mapping entry naming a table that does not exist produces a change set that settles Status=FAILED with a StatusReason naming the missing resource identifier.
Emulator: no FAILED change set is produced at all ('expected one FAILED change set, got []'), so the missing-resource arm is indistinguishable from success.
Why: a client importing resources gets no signal that an identifier was wrong and may believe an unmanaged resource was adopted.
Fix: the import (ChangeSetType=IMPORT) path of the CreateChangeSet handler — resolve each ResourceToImport identifier and fail the change set when it does not exist.
Traces: cloudformation__dynamodb.cdk-import-mapping-file-adopts-a-table-with-its-item-and-refuses-a-missing-one.

## Evidence
AssertionError: expected one FAILED change set, got [] — after `cdk import` with a mapping file naming a nonexistent DynamoDB table, ListChangeSets/DescribeChangeSet show no change set in Status FAILED.

## Steps to Reproduce
Replay cloudformation__dynamodb.cdk-import-mapping-file-adopts-a-table-with-its-item-and-refuses-a-missing-one: create a DynamoDB table with an item out of band, `cdk import` it via a mapping file (adoption arm), then repeat with a mapping file naming a table that does not exist and list the stack's change sets.

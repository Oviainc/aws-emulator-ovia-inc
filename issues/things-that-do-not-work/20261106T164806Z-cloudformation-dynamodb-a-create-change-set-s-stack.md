Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb.cdk-import-mapping-file-adopts-a-table-with-its-item-and-refuses-a-missing-one
Issue-Key: cloudformation__dynamodb.cdk-import-mapping-file-adopts-a-table-with-its-item-and-refuses-a-missing-one|run#18|AssertionError|assert rc == 0, f"the first deploy exited {rc}; output tail {out[-800:]!r}"
Issue-Commit: 0e907dd5f11711da470907c28603d7f862f446bd

# cloudformation__dynamodb: a CREATE change set's stack stays REVIEW_IN_PROGRESS after ExecuteChangeSet, so `cdk import` of a DynamoDB table cannot deploy (ValidationError "Template does not exist for stack [...] in REVIEW_IN_PROGRESS state")

> _Note: this issue was identified against the code base at commit `0e907dd5f11711da470907c28603d7f862f446bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteChangeSet

## Description
Cause 1 of 1: a stack created through the change-set path (CreateChangeSet with ChangeSetType=CREATE, then ExecuteChangeSet) never leaves REVIEW_IN_PROGRESS. The execute provisions nothing and does not advance the stack status, so the deploy tool's next template read is refused and the deployment aborts. The check that adopts an existing DynamoDB table through a CDK import mapping file never gets to its import leg: it dies on the first `cdk deploy` of the base stack.

AWS: CreateChangeSet(ChangeSetType=CREATE) leaves a new stack in REVIEW_IN_PROGRESS; ExecuteChangeSet then moves it to CREATE_IN_PROGRESS and on to a terminal create status, after which GetTemplate serves the executed template and the stack's AWS::DynamoDB::Table resource exists and is readable through DynamoDB DescribeTable. Smithy: ExecuteChangeSet models only ChangeSetNotFoundException (404, awsQueryError ChangeSetNotFound), InsufficientCapabilitiesException (400), InvalidChangeSetStatusException (400) and TokenAlreadyExistsException (400) — none describes an accepted execute that leaves the stack in review. Smithy: GetTemplate returns TemplateBody (@length min 1) and models only ChangeSetNotFoundException (404). The refusal 'Template does not exist for stack [<stack name>] in REVIEW_IN_PROGRESS state' is AWS's own HTTP 400 ValidationError for a stack with no executed template — fixed text 'Template does not exist for stack [' + '] in REVIEW_IN_PROGRESS state', with the stack name taken from the request. It is correct only before the execute; receiving it after one is the symptom of this defect.

Emulator: after CreateChangeSet+ExecuteChangeSet the stack is still REVIEW_IN_PROGRESS, no table is created, and `cdk deploy` exits 1 with the tail '<stack>: creating CloudFormation changeset...\n<stack>: deploying... [1/1]\n failed to deploy\n ValidationError: Template does not exist for stack [<stack>] in REVIEW_IN_PROGRESS state'.

Why: a client using the CDK's default change-set deployment path gets no DynamoDB table, no stack outputs and no terminal status to wait on, while AWS would have provisioned the table. Resource adoption (`cdk import` with a mapping file), which requires a deployed base stack before the import change set is created, becomes impossible, and so does the paired refusal of a mapping that names a table that does not exist.

Fix: the ExecuteChangeSet handler and the stack state machine it drives in the CloudFormation service — the execute must leave REVIEW_IN_PROGRESS, run the provisioner to a terminal create status, and install the executed change set's template as the stack template.

Note: the same cause is filed under other service headings (e.g. 20261106T153504Z-cloudformation-sqs-executechangeset-on-a-create-change-set.md, 20261106T151148Z-cloudformation-dynamodb-lambda-a-change-set-create-leaves.md); neither carries this heading's check.

Traces: cloudformation__dynamodb.cdk-import-mapping-file-adopts-a-table-with-its-item-and-refuses-a-missing-one.

## Evidence
Expected (AWS): the first `cdk deploy` exits 0; DescribeStacks reports a terminal create status and GetTemplate serves TemplateBody; the declared DynamoDB table exists.
Actual (emulator): 'AssertionError: the first deploy exited 1; output tail ... <stack>: creating CloudFormation changeset...\n<stack>: deploying... [1/1]\n❌ <stack> failed to deploy\n‣ ValidationError: Template does not exist for stack [<stack>] in REVIEW_IN_PROGRESS state'. Fixed text: 'Template does not exist for stack [' ... '] in REVIEW_IN_PROGRESS state'; the bracketed stack name is the per-run value from the request and is not part of the requirement.

## Steps to Reproduce
1. CreateChangeSet(StackName=<new stack name>, ChangeSetType=CREATE, TemplateBody=<template declaring one AWS::DynamoDB::Table>); DescribeStacks shows REVIEW_IN_PROGRESS.
2. ExecuteChangeSet(ChangeSetName=<that change set>, StackName=<new stack name>).
3. Poll DescribeStacks: the stack stays REVIEW_IN_PROGRESS and DynamoDB has no such table; GetTemplate(StackName=<new stack name>) is refused HTTP 400 ValidationError 'Template does not exist for stack [<new stack name>] in REVIEW_IN_PROGRESS state'.
Equivalently: `cdk deploy <stack> --require-approval never` for a CDK app containing a Table, then `cdk import`. Re-replay check cloudformation__dynamodb.cdk-import-mapping-file-adopts-a-table-with-its-item-and-refuses-a-missing-one.

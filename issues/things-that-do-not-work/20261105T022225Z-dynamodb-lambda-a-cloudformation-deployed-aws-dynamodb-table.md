Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda.e2e-lambda-env-var-repoint-denied-by-stale-role-scoping-lifecycle
Issue-Key: dynamodb__lambda|DescribeTable|ResourceNotFoundException
Issue-Commit: 3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713

# dynamodb__lambda: a CloudFormation-deployed AWS::DynamoDB::Table is reported created and named in the stack Outputs, but DescribeTable answers 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTable

## Description
One root cause. The failing check is not marked OWNED or ALREADY FILED in the replay report.

AWS: Smithy dynamodb DescribeTable returns the TableDescription (TableName, KeySchema, AttributeDefinitions, TableStatus) for a table that exists, and ResourceNotFoundException (HTTP 400, x-amzn-errortype ResourceNotFoundException) only when it does not. A stack that declares AWS::DynamoDB::Table with an explicit TableName taken from a stack Parameter and reaches a successful CREATE status has created that table, so DescribeTable(TableName=<that name>) succeeds and reports the declared KeySchema.
Emulator: the deploy phase succeeds and the stack Output echoes the requested table name correctly (the check asserts Outputs.OldTableName equals the parameter it passed and that assertion passes), but DynamoDB has no such table — DescribeTable is refused with `Requested resource not found: Table: <table-name> not found`. The fixed text is `Requested resource not found: Table: ` + `<name>` + ` not found`; the only variable part is the table name from the request.
Why: a client that provisions its tables declaratively and then reads the stack Output to address them is refused on the first data-plane or control-plane call. The whole estate the template describes (table + function + role scoping) is unusable, and the stack's success status is misleading: there is no way to distinguish a deployed table from one that was never created.
Fix: the CloudFormation resource provider for AWS::DynamoDB::Table — it must create the table in the DynamoDB service state under the template's TableName (and report that name as the physical resource id) rather than only completing the stack resource.
Traces: dynamodb__lambda.e2e-lambda-env-var-repoint-denied-by-stale-role-scoping-lifecycle.

## Evidence
Expected: after the stack reaches a successful CREATE status, dynamodb DescribeTable(TableName=<stack Output OldTableName>) returns 200 with Table.KeySchema.
Actual (replay): `[phase 1/5 deploy] ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeTable operation: Requested resource not found: Table: <table-name> not found` — raised from the check's `_key_schema` helper (`ddb.describe_table(TableName=table)["Table"]["KeySchema"]`) on the very name the stack Output reported. The table name is a per-run generated value.

## Steps to Reproduce
1. CreateStack with a template that declares an AWS::DynamoDB::Table whose TableName comes from a stack Parameter, plus Outputs echoing that name.
2. Wait for the stack to reach a successful CREATE status and read the Output — it matches the parameter that was passed.
3. Call dynamodb DescribeTable with that name — it is refused ResourceNotFoundException "Requested resource not found: Table: <name> not found".

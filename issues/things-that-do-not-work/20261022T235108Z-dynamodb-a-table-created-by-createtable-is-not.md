Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__ec2.e2e-ddb-ltr-list-tables-all-regions-lifecycle dynamodb__ec2.ddb-ltr-enabled-region-sweep-finds-tables-only-where-created
Issue-Key: dynamodb__ec2|DescribeTable|ResourceNotFoundException
Issue-Key: dynamodb__ec2|DescribeRegions|NotImplemented
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# DynamoDB: a table created by CreateTable is not found by DescribeTable, and the enabled-region sweep is blocked by the EC2 DescribeRegions stub (2 causes)

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTable

## Description
Two root causes. A fixer must close both before submitting.

Cause 1 of 2: DescribeTable answers ResourceNotFoundException for a table the deploy phase just created.
AWS: after CreateTable returns, DescribeTable on that TableName in the same region returns the table (CREATING then ACTIVE) and never ResourceNotFoundException (Smithy com-amazonaws-dynamodb-dynamodb-20120810: DescribeTable errors include ResourceNotFoundException, InternalServerError; it is raised only for a table that does not exist).
Emulator: the deploy phase creates sim2real-scn-ltr-...-table1-us-west-2 and the immediately following DescribeTable is refused with ResourceNotFoundException: 'Requested resource not found: Table: sim2real-scn-ltr-...-table1-us-west-2 not found'.
Why: a client cannot wait for a table it just created to become ACTIVE, so every create-then-use flow fails. Hypothesis: the created table is not visible under the region/endpoint the subsequent client uses, i.e. table state is not keyed the way the create path keys it.
Fix: DynamoDB table state and the CreateTable/DescribeTable handlers (table lookup key, including region scoping).
Traces: dynamodb__ec2.e2e-ddb-ltr-list-tables-all-regions-lifecycle (phase 1/4 deploy).

Cause 2 of 2: the all-regions table sweep cannot run because EC2 DescribeRegions is a stub.
AWS: ec2:DescribeRegions returns the account's enabled regions, which is how a client enumerates regions before fanning out ListTables (Smithy com-amazonaws-ec2-amazonec2: DescribeRegions -> DescribeRegionsResult.Regions; no modeled errors).
Emulator: DescribeRegions answers NotImplemented ('operation DescribeRegions is scaffolded but not implemented in this emulator'), so the sweep aborts before any DynamoDB assertion runs.
Why: any multi-region inventory tool is dead on arrival, and the DynamoDB behaviour under test is never exercised.
Fix: services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_regions.rs.
Traces: dynamodb__ec2.ddb-ltr-enabled-region-sweep-finds-tables-only-where-created. The suite reports a stub issue naming DescribeRegions filed under the ec2 service for the ec2-led checks it blocks; it does not carry this dynamodb-led check.

## Evidence
Cause 1 — expected: DescribeTable returns the just-created table; actual: _PhaseError: [phase 1/4 deploy] ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeTable operation: Requested resource not found: Table: sim2real-scn-ltr-569cbf64e068-table1-us-west-2 not found.
Cause 2 — expected: DescribeRegions returns the enabled regions; actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeRegions operation: operation DescribeRegions is scaffolded but not implemented in this emulator.

## Steps to Reproduce
Replay the two check ids in trace_ids. Minimal repros: (1) dynamodb:CreateTable then dynamodb:DescribeTable with the same TableName. (2) ec2:DescribeRegions.

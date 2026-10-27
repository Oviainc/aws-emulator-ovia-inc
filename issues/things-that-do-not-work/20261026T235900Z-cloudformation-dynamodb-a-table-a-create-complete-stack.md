Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb.cfn-unsupported-table-property-is-not-silently-discarded
Issue-Key: dynamodb|DescribeTimeToLive|ResourceNotFoundException
Issue-Commit: 281440c6fd2fb349634dcc135fbec4e15edad452

# cloudformation__dynamodb: a table a CREATE_COMPLETE stack declared with TimeToLiveSpecification is unknown to DynamoDB — DescribeTimeToLive answers ResourceNotFoundException for the stack's table

> _Note: this issue was identified against the code base at commit `281440c6fd2fb349634dcc135fbec4e15edad452`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1 — a CloudFormation-provisioned AWS::DynamoDB::Table carrying TimeToLiveSpecification does not exist as far as DynamoDB's TTL read is concerned.
AWS: recorded behavior only — a stack declaring AWS::DynamoDB::Table with TimeToLiveSpecification {AttributeName: <attr>, Enabled: true} reaches CREATE_COMPLETE and the live table honours the property: DescribeTimeToLive(TableName=<the table the template named>) returns TimeToLiveDescription with TimeToLiveStatus ENABLED and AttributeName equal to the attribute the template named. The Smithy model for DynamoDB models ResourceNotFoundException on DescribeTimeToLive only for a table that does not exist, so a table a completed stack created must never produce it.
Emulator: after the stack settles, DescribeTimeToLive on that table name is refused with `ResourceNotFoundException: Requested resource not found: Table: <table-name> not found` — the table the stack reported creating is not resolvable through DynamoDB's TTL read path, so neither the table nor the TTL configuration the template declared can be observed.
Why: the CloudFormation status is the only evidence the stack produces, and it says the table exists; a client that provisions a TTL-backed table by stack and then verifies or uses TTL through DynamoDB gets a 400 ResourceNotFoundException for a resource CloudFormation says it owns. Silent discard of a legal template property (or a table registered under a different identity than the template's TableName) is invisible from the stack alone.
Fix: the AWS::DynamoDB::Table provisioner in the cloudformation crate — it must create the table under the TableName the template names and apply TimeToLiveSpecification to the dynamodb state that DescribeTimeToLive reads (and, if the property cannot be applied, fail the resource instead of reporting CREATE_COMPLETE).
Traces: cloudformation__dynamodb.cfn-unsupported-table-property-is-not-silently-discarded

## Evidence
cloudformation__dynamodb.cfn-unsupported-table-property-is-not-silently-discarded:
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeTimeToLive operation: Requested resource not found: Table: sim2real-xsvc-ttl-<id> not found

The check creates the stack, waits for a CREATE terminal status, and then reads the property back through the owning service (its own comment: "the only way to see [silent discard] is to read the property back through the owning service rather than trusting the stack status"). The TTL attribute under test is "expiresAt" and the expected read is TimeToLiveDescription.TimeToLiveStatus == ENABLED with AttributeName == that attribute.

## Steps to Reproduce
1. CreateStack with a template containing {"Type": "AWS::DynamoDB::Table", "Properties": {"TableName": <name>, "BillingMode": "PAY_PER_REQUEST", "AttributeDefinitions": [...], "KeySchema": [...], "TimeToLiveSpecification": {"AttributeName": "expiresAt", "Enabled": true}}}.
2. Wait until the stack reaches a CREATE terminal state (it reports CREATE_COMPLETE).
3. Call dynamodb DescribeTimeToLive(TableName=<name>): expect TimeToLiveStatus ENABLED on attribute expiresAt; the emulator answers ResourceNotFoundException "Requested resource not found: Table: <name> not found".

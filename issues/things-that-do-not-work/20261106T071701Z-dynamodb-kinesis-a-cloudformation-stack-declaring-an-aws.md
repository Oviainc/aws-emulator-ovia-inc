Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__kinesis.e2e-ddbkds-list-streaming-tables-all-regions-lifecycle
Issue-Key: dynamodb__kinesis.e2e-ddbkds-list-streaming-tables-all-regions-lifecycle|phase#5|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 77172d4b913d3cd289b60abc3e2fa4659d3ee8a2

# dynamodb__kinesis: a CloudFormation stack declaring an AWS::DynamoDB::Table with KinesisStreamSpecification rolls back, so no CFN-built Kinesis streaming destination estate can exist

> _Note: this issue was identified against the code base at commit `77172d4b913d3cd289b60abc3e2fa4659d3ee8a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
ONE net-new root cause. The failing check is marked neither ALREADY FILED nor OWNED in the replay report.

Cause 1 of 1: CreateStack fails on an AWS::DynamoDB::Table whose Properties carry KinesisStreamSpecification, and the stack ends ROLLBACK_COMPLETE.
AWS: recorded behavior only for the stack outcome (CloudFormation resource properties are not in the DynamoDB Smithy model; the underlying data-plane call is EnableKinesisStreamingDestination, whose input is TableName + StreamArn + an optional EnableKinesisStreamingConfiguration). A template declaring an AWS::Kinesis::Stream and a PROVISIONED AWS::DynamoDB::Table whose KinesisStreamSpecification.StreamArn is Fn::GetAtt <Stream>.Arn, alongside a second PAY_PER_REQUEST table with no streaming destination, reaches CREATE_COMPLETE: both tables become ACTIVE, the streaming destination settles (ACTIVE), and the stack's Outputs (StreamArn, StreamTableName, PlainTableName) resolve.
Emulator: the stack goes to `ROLLBACK_COMPLETE` instead of CREATE_COMPLETE, so neither table nor the destination is created and every later phase (listing streaming tables, destination status, destroy) is unreachable. Replay: "[phase 1/4 deploy] stack <name> in <region> reached 'ROLLBACK_COMPLETE', not CREATE_COMPLETE -- read its stack events for the resource that failed".
Why: KinesisStreamSpecification is the only declarative way to attach a DynamoDB change-data-capture destination; a client whose template uses it cannot deploy at all, and because the failure arrives as a rollback rather than a named property error, the client has no indication of which property is unsupported.
Fix: the CloudFormation AWS::DynamoDB::Table resource provider — it must accept KinesisStreamSpecification and map it onto EnableKinesisStreamingDestination (with the referenced stream ARN resolved from the sibling AWS::Kinesis::Stream) during resource create, and surface the real reason in the stack event when a property cannot be honoured.
Traces: dynamodb__kinesis.e2e-ddbkds-list-streaming-tables-all-regions-lifecycle.

## Evidence
Suite simparity-behavioral-aws-v1 replayed against this checkout: 630/1315 passed. Service heading `dynamodb__kinesis` (15/24 passed, 9 failed); dynamodb and kinesis are both stood up (only iot is pending bootstrap). This check is marked neither ALREADY FILED nor OWNED.

[mismatch] dynamodb__kinesis.e2e-ddbkds-list-streaming-tables-all-regions-lifecycle (check line 201):
  AssertionError: [phase 1/4 deploy] stack sim2real-ddbkds-e2e-338f695a699b-home in us-west-2 reached 'ROLLBACK_COMPLETE', not CREATE_COMPLETE -- read its stack events for the resource that failed

The template the phase deploys (deployments/ddbkds-stream-estate/template.json, parameterised physical names):
  Resources.ChangeStream: AWS::Kinesis::Stream { Name: !Ref StreamName, ShardCount: 1 }
  Resources.TableWithStream: AWS::DynamoDB::Table { TableName: !Ref StreamTableName, AttributeDefinitions: [{id,S}], KeySchema: [{id,HASH}], ProvisionedThroughput: {Read 5, Write 5}, KinesisStreamSpecification: { StreamArn: !GetAtt ChangeStream.Arn } }
  Resources.TableWithoutStream: AWS::DynamoDB::Table { TableName: !Ref PlainTableName, AttributeDefinitions: [{id,S}], KeySchema: [{id,HASH}], BillingMode: PAY_PER_REQUEST }
  Outputs: StreamArn (!GetAtt ChangeStream.Arn), StreamTableName (!Ref TableWithStream), PlainTableName (!Ref TableWithoutStream)

  Expected: CREATE_COMPLETE; both tables ACTIVE; DescribeKinesisStreamingDestination(TableWithStream) settles to ACTIVE against the created stream's ARN; the three Outputs resolve, and the later phases can list streaming tables and then destroy the stack.
  Actual: the stack reaches ROLLBACK_COMPLETE in the deploy phase, so nothing downstream runs.

The stack name and region above are this run's generated values, not part of the requirement.

## Steps to Reproduce
1. Boot the emulator from this checkout; point cloudformation, dynamodb and kinesis clients at it.
2. CreateStack with the template above: an AWS::Kinesis::Stream (ShardCount 1) and an AWS::DynamoDB::Table with ProvisionedThroughput {5,5}, KeySchema id(HASH,S) and KinesisStreamSpecification.StreamArn = Fn::GetAtt <Stream>.Arn, plus a second PAY_PER_REQUEST table with no streaming destination.
3. Wait on StackCreateComplete: the stack reaches ROLLBACK_COMPLETE. Call DescribeStackEvents to see which resource failed and with what reason.
4. Expected: CREATE_COMPLETE; DescribeTable on the streamed table is ACTIVE and DescribeKinesisStreamingDestination(TableName=<streamed table>) reports the created stream's ARN with DestinationStatus settling to ACTIVE; the stack Outputs StreamArn / StreamTableName / PlainTableName resolve; DeleteStack then removes both tables and the stream.
   Replay: dynamodb__kinesis.e2e-ddbkds-list-streaming-tables-all-regions-lifecycle

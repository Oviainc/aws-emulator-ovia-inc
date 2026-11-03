Issue-Severity: MEDIUM
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# Lambda CreateEventSourceMapping: the unrecognized-event-source and missing-StartingPosition refusals use the wrong text, and the mapping's settings are not round-tripped (2 causes)

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Two net-new root causes in Lambda's event-source-mapping control plane as exercised with a DynamoDB stream source.

=== Cause 1 of 2: the refusal messages for a DynamoDB-stream event source are not AWS's ===
AWS: CreateEventSourceMapping given a DynamoDB TABLE ARN where a STREAM ARN is required is refused with InvalidParameterValueException whose message is the fixed sentence naming the expected ARN shape — "Unrecognized event source. Expected arn of format arn:aws:dynamodb:<region>:<account-id>:table/TableName/stream/StreamId" — i.e. the template states the required format, it does not echo the submitted ARN or enumerate every supported source type. Separately, creating a mapping over a DynamoDB stream without StartingPosition is refused with AWS's own StartingPosition-required sentence rather than one that interpolates the event-source ARN.
Emulator: the first answers "Unrecognized event source '<submitted table ARN>'. An event source mapping polls an Amazon Kinesis data stream or stream consumer, an Amazon DynamoDB stream, an Amazon Simple Queue Service queue, an Amazon Managed Streaming for Apache Kafka cluster or VPC connection, an Amazon MQ broker or an Amazon DocumentDB change stream" — a catalogue of source types with the caller's ARN interpolated, not AWS's format template. The second answers InvalidParameterValueException "StartingPosition must be specified for the event source '<stream ARN>'", interpolating the ARN where AWS does not.
Why: the message is the only machine-readable statement of what was wrong; a client (or a CDK/CloudFormation provider) that matches AWS's text, or parses the expected ARN format out of it, fails.
Fix: the refusal messages in the Lambda CreateEventSourceMapping handler's event-source validation.
Traces: stream-mapping-rejects-a-table-arn-in-place-of-its-stream-arn, stream-mapping-requires-a-starting-position.

=== Cause 2 of 2: a created mapping does not round-trip the settings it was created with ===
AWS: GetEventSourceMapping/ListEventSourceMappings report back the settings the mapping was created with, and omit members that were never configured rather than reporting them as null. TumblingWindowInSeconds is absent from the response when it was not configured, not present-and-null.
Emulator: the mapping reads back with TumblingWindowInSeconds: None present in the response and the configured set not matching what the stack declared, so a caller cannot confirm what it created.
Why: a client reconciling desired against actual mapping configuration (every IaC provider does this) sees permanent drift, and a null where AWS omits the key breaks strict deserializers.
Fix: the event-source-mapping state and the GetEventSourceMapping/ListEventSourceMappings serializers — persist the configured values and omit unset optional members.
Traces: stream-mappings-round-trip-the-stack-settings-and-name-the-regression-table.

## Evidence
Cause 1 — stream-mapping-rejects-a-table-arn-in-place-of-its-stream-arn: AssertionError: got "Unrecognized event source 'arn:aws:dynamodb:us-west-2:<acct>:table/<table>'. An event source mapping polls an Amazon Kinesis data stream or stream consumer, an Amazon DynamoDB stream, an Amazon Simple Queue Service queue, an Amazon Managed Streaming for Apache Kafka cluster or VPC connection, an Amazon MQ broker or an Amazon DocumentDB change stream", want 'Unrecognized event source. Expected arn of format arn:aws:dynamodb:us-west-2:123456789012:table/TableName/stream/StreamId'.
stream-mapping-requires-a-starting-position: AssertionError: got 'InvalidParameterValueException': "StartingPosition must be specified for the event source 'arn:aws:dynamodb:us-west-2:<acct>:table/<table>/stream/<label>'".

Cause 2 — stream-mappings-round-trip-the-stack-settings-and-name-the-regression-table: AssertionError: mapping <uuid> reads {'StartingPosition': 'LATEST', 'BatchSize': 1, 'MaximumRetryAttempts': 3, 'MaximumBatchingWindowInSeconds': 0, 'ParallelizationFactor': 1, 'MaximumRecordAgeInSeconds': -1, 'BisectBatchOnFunctionError': False, 'TumblingWindowInSeconds': None, 'FunctionResponseTypes': [], 'State': 'Enabled'}.

## Steps to Reproduce
Replay the behavioral suite and re-run the dynamodb__lambda checks in trace_ids. Minimal reproductions: (1) lambda.create_event_source_mapping(FunctionName=fn, EventSourceArn='<table ARN, not stream ARN>', StartingPosition='LATEST') and read Error.Message; (2) lambda.create_event_source_mapping(FunctionName=fn, EventSourceArn='<stream ARN>') with no StartingPosition and read Error.Message; (3) create a mapping with explicit BatchSize/MaximumRetryAttempts and compare get_event_source_mapping's response with what was sent, checking that unset optional members are omitted rather than null.

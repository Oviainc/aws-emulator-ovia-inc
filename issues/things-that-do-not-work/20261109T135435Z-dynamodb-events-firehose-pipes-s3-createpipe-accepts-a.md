Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__events__firehose__pipes__s3.pipes-create-pipe-eventbridge-and-firehose-target-batch-size-caps-rejected
Issue-Key: dynamodb__events__firehose__pipes__s3.pipes-create-pipe-eventbridge-and-firehose-target-batch-size-caps-rejected|_rejected_code#23|AssertionError|raise AssertionError(
Issue-Commit: 3b4b4e2df28997ae7cbb85154b0c83cf06a43ef8

# dynamodb__events__firehose__pipes__s3: CreatePipe accepts a DynamoDB-stream BatchSize above the target's documented cap instead of refusing it with ValidationException

> _Note: this issue was identified against the code base at commit `3b4b4e2df28997ae7cbb85154b0c83cf06a43ef8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
=== Cause 1 of 1: the target-specific BatchSize cap is never enforced, so an over-cap pipe is created ===
Failing check: dynamodb__events__firehose__pipes__s3.pipes-create-pipe-eventbridge-and-firehose-target-batch-size-caps-rejected.
AWS: Smithy PipeSourceDynamoDBStreamParameters.BatchSize targets LimitMax10000 (@range 1..10000), so the member's own range admits 11 and 501; the cap that applies is the TARGET's maximum batch size, which EventBridge Pipes enforces server-side at CreatePipe — 10 for an EventBridge event-bus target and 500 for a Firehose delivery-stream target. Over-cap input is refused with HTTP 400, error code ValidationException, and a message that is the JSON envelope AWS uses for field validation: {"ValidationErrorFields":[{"Name":"BatchSize","Message":"The configured batch size <requested> is greater than the max supported batch size <cap> for target <target service>."}]}. The fixed text is 'ValidationErrorFields', the member name 'BatchSize' and the sentence 'The configured batch size … is greater than the max supported batch size … for target ….'; <requested> is the BatchSize from the request, <cap> the target's maximum (10 for events, 500 for firehose) and <target service> the target's service token ('events', 'firehose') derived from the Target ARN. The refused pipe is not created, so it never appears in ListPipes.
Emulator: the request is ACCEPTED with HTTP 200 and a pipe is minted. For a DynamoDB-stream source with BatchSize=11 and an EventBridge event-bus Target, the response is {'HTTPStatusCode': 200, 'Arn': 'arn:aws:pipes:<region>:<account>:pipe/<name>', 'Name': '<name>', 'DesiredState': 'STOPPED', 'CurrentState': 'CREATING', 'CreationTime': ..., 'LastModifiedTime': ...} — no refusal at all. The in-cap control legs (BatchSize=10 to the event bus, 500 to Firehose) are accepted and echoed correctly, so validation of the member is simply absent rather than mis-scoped.
Why: a client that relies on CreatePipe to reject an over-cap batch size gets a pipe that production refuses — the misconfiguration is discovered only at deploy time against real AWS, and a CI test pinning the ValidationException (its code or its field-level message) passes on the emulator and fails in production. The over-cap pipe also pollutes ListPipes with a resource AWS would never have created.
Fix: CreatePipe's input validation in the pipes service crate must resolve the target type from the Target ARN and reject SourceParameters.*StreamParameters.BatchSize above that target's maximum (10 for an events event bus, 500 for a firehose delivery stream) with ValidationException and the ValidationErrorFields message above, before any pipe state is created; UpdatePipe needs the same rule.
Traces: the check id above.

## Evidence
Replayed suite: simparity-behavioral-aws-v1 — dynamodb__events__firehose__pipes__s3 0/1 (0%), the single check failing and marked neither ALREADY FILED nor OWNED.

Expected (AWS): create_pipe(Name=P, Source=<ddb stream arn>, SourceParameters={'DynamoDBStreamParameters': {'StartingPosition':'LATEST','BatchSize':11}}, Target=<default event bus arn>, RoleArn=R, DesiredState='STOPPED') is refused with ClientError code 'ValidationException' and message '{"ValidationErrorFields":[{"Name":"BatchSize","Message":"The configured batch size 11 is greater than the max supported batch size 10 for target events."}]}', and P does not appear in list_pipes. The Firehose-target leg expects the same shape with 501/500 and 'for target firehose.'
Actual (emulator): "AssertionError: create_pipe('sim2real-xsvc-pipe-…') was ACCEPTED where real AWS rejects it; response {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'Arn': 'arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-xsvc-pipe-…', 'Name': 'sim2real-xsvc-pipe-…', 'DesiredState': 'STOPPED', 'CurrentState': 'CREATING', 'CreationTime': ..., 'LastModifiedTime': ...}". The in-cap control legs (BatchSize=10 to the bus, BatchSize=500 to Firehose) were accepted and echoed by describe_pipe, so the positive path works and only the cap check is missing.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. dynamodb.create_table(...) with StreamSpecification enabled; note the LatestStreamArn. firehose.create_delivery_stream(...) with an ExtendedS3 destination; note its ARN. iam.create_role(R) trusting pipes.amazonaws.com with dynamodb stream read, events:PutEvents and firehose:PutRecordBatch.
3. Control: pipes.create_pipe(Name=P1, RoleArn=R, Source=<stream arn>, SourceParameters={'DynamoDBStreamParameters': {'StartingPosition':'LATEST','BatchSize':10}}, Target=<default bus arn>, DesiredState='STOPPED') — expect accepted and describe_pipe echoing BatchSize 10 (this passes).
4. pipes.create_pipe(Name=P2, same but BatchSize=11) — expect ClientError ValidationException with message '{"ValidationErrorFields":[{"Name":"BatchSize","Message":"The configured batch size 11 is greater than the max supported batch size 10 for target events."}]}'; observe HTTP 200 and a created pipe.
5. Repeat with Target=<firehose delivery stream arn> and BatchSize=501 — expect the same error code with '… max supported batch size 500 for target firehose.'; observe acceptance.
6. pipes.list_pipes(NamePrefix=P2) — expect the refused pipe to be absent.

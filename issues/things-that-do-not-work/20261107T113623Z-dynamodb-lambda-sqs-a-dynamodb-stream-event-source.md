Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__sqs.ddb-stream-esm-filter-criteria-insert-only-not-invoked
Issue-Key: dynamodb__lambda__sqs.ddb-stream-esm-filter-criteria-insert-only-not-invoked|_collect#19|Timeout|resp = sqs.receive_message(
Issue-Commit: 46dd75ad5ad4e002f82bb21fdacb370faf08a983

# dynamodb__lambda__sqs: a DynamoDB-stream event source mapping with FilterCriteria never invokes its function, so the INSERT-only filter check reaches no verdict

> _Note: this issue was identified against the code base at commit `46dd75ad5ad4e002f82bb21fdacb370faf08a983`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: a DynamoDB Streams event source mapping created with StartingPosition=TRIM_HORIZON and FilterCriteria never delivers any stream record to the mapped function, so neither the positive control (the INSERT record) nor the negative observable (no MODIFY record) can be observed and the check never reaches a verdict.

AWS: Smithy (services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json) models CreateEventSourceMapping POST /2015-03-31/event-source-mappings -> 202 EventSourceMappingConfiguration with EventSourceArn, StartingPosition (EventSourcePosition enum, TRIM_HORIZON), BatchSize (range 1..10000), MaximumBatchingWindowInSeconds (0..300), FilterCriteria and a State member. On AWS, once the returned mapping reaches State="Enabled", the Lambda-managed poller reads the DynamoDB stream from the requested starting position and synchronously invokes the function with an event whose Records carry eventName and dynamodb.Keys; FilterCriteria Filters[].Pattern is evaluated per record, and records that do not match the pattern (here eventName not in ["INSERT"]) are discarded by the poller without an invocation while the shard is still advanced past them. Both the discard and the advance are AWS behaviour; the model states only the request/response surface of the mapping, so the runtime half is recorded behaviour only.

Emulator: the mapping is created and reports Enabled (the check's _wait_esm_state step is satisfied), but no invocation ever happens: the output queue the handler writes to stays empty for the INSERT record written before the mapping was created, and the check is killed by the harness with "Check timed out after 60.0s" while still waiting for its positive control. The verdict is reported as a mismatch, not as a refusal — no operation answered an error; the stream-to-Lambda delivery path simply produces nothing.

Why: a client that wires a DynamoDB stream to a Lambda through an event source mapping sees a healthy, Enabled mapping and never receives a single record. Every stream-driven design (change-data-capture, projection/table-to-queue fan-out, audit trails) silently processes nothing, and because nothing is ever delivered, FilterCriteria cannot be distinguished from a filter that drops everything — a test that asserts "the non-matching record was NOT delivered" passes vacuously against this emulator while the matching record is lost too.

Fix: the Lambda event-source-mapping poller for DynamoDB Streams sources — the component that, for an Enabled mapping, reads the stream from StartingPosition, applies FilterCriteria to each record, batches up to BatchSize within MaximumBatchingWindowInSeconds and invokes the target function. The mapping's create/describe handler already serves the configuration; the missing piece is the runtime that consumes the stream and invokes.

Traces: dynamodb__lambda__sqs.ddb-stream-esm-filter-criteria-insert-only-not-invoked (the only failing check for this cause). Related open issues describe the same delivery gap under OTHER service headings and do not cover this check: 20261104T044826Z-dynamodb-lambda-logs-dynamodb-streams-records-never-reach-the-mapped-lambda.md and 20261104T044826Z-dynamodb-events-lambda-cli-driven-eventbridge-rules-and-dynamodb-stream-event.md; the open dynamodb__lambda__sqs issue 20261106T134622Z-dynamodb-lambda-sqs-a-queue-policy-deny-does.md covers SQS-source mappings and a queue-policy Deny, not a DynamoDB-stream source with FilterCriteria.

## Evidence
Expected (AWS): create_event_source_mapping(EventSourceArn=<table LatestStreamArn>, FunctionName=<fn>, StartingPosition='TRIM_HORIZON', BatchSize=10, MaximumBatchingWindowInSeconds=1, FilterCriteria={'Filters':[{'Pattern':'{"eventName": ["INSERT"]}'}]}) returns 202 with State transitioning to 'Enabled'; the poller then invokes the function with the INSERT record for pk=row-1 (event.Records[0].eventName == 'INSERT', dynamodb.Keys.pk.S == 'row-1'), and never invokes it for the MODIFY record written to the same key, even though the shard was read past that MODIFY to reach the INSERT.

Actual (emulator): the mapping is created and reaches Enabled, both put_item calls succeed, and the handler is never invoked — the SQS output queue receives no message at all. The check's wait for its positive control (a body with eventName == 'INSERT', budget 180s) never completes and the harness terminates it: "Error: Check timed out after 60.0s". No HTTP error and no error code is returned by any call in the scenario; the divergence is the total absence of invocation, which also makes the MODIFY-withheld assertion unverifiable.

## Steps to Reproduce
1. dynamodb.create_table(TableName=<t>, KeySchema=[{pk,HASH}], AttributeDefinitions=[{pk,S}], BillingMode='PAY_PER_REQUEST', StreamSpecification={'StreamEnabled': True, 'StreamViewType': 'NEW_AND_OLD_IMAGES'}) and poll describe_table until TableStatus=ACTIVE and LatestStreamArn is present.
2. Create an SQS queue and a python Lambda whose handler forwards each record's eventName and dynamodb.Keys.pk.S to that queue via sqs.send_message, with a role granting dynamodb:DescribeStream/GetRecords/GetShardIterator/ListStreams on the stream ARN and sqs:SendMessage on the queue ARN.
3. put_item(pk='row-1', v='first') then put_item(pk='row-1', v='second') BEFORE creating the mapping (so the stream holds one INSERT followed by one MODIFY).
4. lambda.create_event_source_mapping(EventSourceArn=<LatestStreamArn>, FunctionName=<fn>, StartingPosition='TRIM_HORIZON', BatchSize=10, MaximumBatchingWindowInSeconds=1, FilterCriteria={'Filters':[{'Pattern':'{"eventName": ["INSERT"]}'}]}) and wait until the mapping State is 'Enabled'.
5. Long-poll the output queue for a message whose JSON body has eventName=='INSERT'. Expected: it arrives (and no eventName=='MODIFY' body ever does). Observed: no message arrives at all and the check runs to its deadline.

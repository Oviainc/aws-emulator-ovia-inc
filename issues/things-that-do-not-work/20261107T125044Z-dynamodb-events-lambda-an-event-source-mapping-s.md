Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__events__lambda.ddb-stream-esm-filter-criteria-json-positive dynamodb__events__lambda.cfn-eventbridge-rule-lambda-target-deployment
Issue-Key: dynamodb__events__lambda.ddb-stream-esm-filter-criteria-json-positive|run#17|AssertionError|assert "id-skip" not in observed, f"filtered-out record was delivered to Lambda anyway: {observed!r}"
Issue-Key: dynamodb__events__lambda.cfn-eventbridge-rule-lambda-target-deployment|_poll_until#9|Timeout|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4

# dynamodb__events__lambda: an event source mapping's FilterCriteria is ignored (filtered-out stream records are delivered) and a CloudFormation-deployed EventBridge rule never invokes its permitted Lambda target (2 causes)

> _Note: this issue was identified against the code base at commit `49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
2 net-new root causes in this service group. Neither failing check is marked ALREADY FILED or OWNED, so no open issue carries their replay keys. Open issues that describe neighbouring symptoms but do NOT carry these keys: 20261029T180026Z-dynamodb-events-lambda-eventbridge-rules-never-invoke-their-lambda-targets-ddb.md, 20261104T044826Z-dynamodb-events-lambda-cli-driven-eventbridge-rules-and-dynamodb-stream-event.md and 20261103T062653Z-dynamodb-events-lambda-a-dynamodb-streams-esm-with.md.

Cause 1 of 2: FilterCriteria on a DynamoDB-Streams event source mapping is accepted and then not applied — every record in the shard is delivered to the function, including those the filter pattern excludes. Failing check: dynamodb__events__lambda.ddb-stream-esm-filter-criteria-json-positive.
AWS: Smithy (lambda) CreateEventSourceMapping (POST /2015-03-31/event-source-mappings, 202) and UpdateEventSourceMapping (PUT /2015-03-31/event-source-mappings/{UUID}, 202) both take a FilterCriteria member and echo it back on EventSourceMappingConfiguration together with FilterCriteriaError. AWS evaluates each Filters[].Pattern (an EventBridge-style JSON pattern over the stream record envelope, e.g. {"dynamodb":{"Keys":{"id":{"S":["id-keep"]}}}}) against every record BEFORE invoking the function: a record that does not match is dropped by the poller and the function is never invoked for it. The model states the member; the matching semantics are recorded behavior.
Emulator: the handler observed BOTH the matching and the non-matching record. The check fails with AssertionError: "filtered-out record was delivered to Lambda anyway: {'id-keep', 'id-skip'}" — i.e. the key that the pattern excludes was present in the set of keys the function recorded, so the filter had no effect on delivery (the create/echo of FilterCriteria itself succeeded).
Why: FilterCriteria is the only server-side way to keep unwanted stream traffic out of a function. A client that relies on it gets its handler invoked for records it explicitly excluded — duplicate or illegal side effects (writes, notifications, billing events) for records it believes were never delivered — and pays for invocations AWS would not have made. Negative tests built on a filter silently pass on AWS and fail here, or worse, mutate state here that they do not on AWS.
Fix: the lambda crate's event-source-mapping poller — the stage between reading a DynamoDB stream shard and invoking the function must evaluate each configured Filters[].Pattern against the record envelope and drop non-matching records (and report an unparseable pattern via FilterCriteriaError) instead of forwarding the whole batch.
Traces: dynamodb__events__lambda.ddb-stream-esm-filter-criteria-json-positive.

Cause 2 of 2: a CloudFormation-deployed EventBridge Rule whose AWS::Lambda::Permission grants events.amazonaws.com never invokes the target, so a matching PutEvents produces no side effect and the check is killed at the harness's 60 s cap. Failing check: dynamodb__events__lambda.cfn-eventbridge-rule-lambda-target-deployment.
AWS: recorded behavior only for the end-to-end path (the Smithy models describe the APIs, not the delivery). PutEvents against the default bus with an entry whose Source and DetailType match the deployed rule's EventPattern returns 200 with FailedEntryCount 0 and an EventId per entry, and EventBridge then invokes the rule's Lambda target asynchronously, passing the full event envelope — the Detail fields from the request reach the handler, which writes them through to DynamoDB. The deployment order matters: the stack's rule, target and lambda permission are all CREATE_COMPLETE before the first event, so the very first matching PutEvents is delivered.
Emulator: the only detail recorded is "Check timed out after 60.0s". The stack deploys and its TableName output is read, PutEvents is issued, and the check then polls GetItem for the marker the target handler would write; that bounded poll never observes the item, so the harness kills the check with no assertion reached. No delivery and no error are produced — PutEvents reports success while the target is inert.
Why: a client wiring the canonical CFN pattern (Rule + Target + Lambda::Permission) gets a stack that reports success and a PutEvents that reports success, yet nothing downstream ever runs and nothing reports a failure. Event-driven code paths cannot be exercised at all, and a genuinely broken rule or permission is indistinguishable from a working one.
Fix: the eventbridge crate's rule-matching and target-dispatch path (match a PutEvents entry against each rule's EventPattern on the named bus and invoke Lambda targets asynchronously with the full envelope, honouring the lambda:InvokeFunction permission) together with the CloudFormation provisioning of AWS::Events::Rule targets and AWS::Lambda::Permission, which must register the target so it is live once the stack reaches CREATE_COMPLETE.
Traces: dynamodb__events__lambda.cfn-eventbridge-rule-lambda-target-deployment.

## Evidence
Cause 1 — Expected (AWS): CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn=<table stream arn>, StartingPosition=TRIM_HORIZON, BatchSize=1, FilterCriteria={"Filters":[{"Pattern":"{\"dynamodb\":{\"Keys\":{\"id\":{\"S\":[\"id-keep\"]}}}}"}]}) then PutItem of both a matching item (id-keep) and a non-matching item (id-skip) results in the function being invoked only for id-keep; the set of keys the handler records is exactly {'id-keep'}.
Actual (emulator): the handler recorded {'id-keep', 'id-skip'}. AssertionError: "filtered-out record was delivered to Lambda anyway: {'id-keep', 'id-skip'}". (The id values are the check's own request values; the requirement is that a non-matching record is not delivered.)

Cause 2 — Expected (AWS): after the stack reaches CREATE_COMPLETE, PutEvents(Entries=[{Source: <rule's source>, DetailType: 'order-created', Detail: '{"test_marker":<marker>,"amount":42}'}]) returns FailedEntryCount 0 and the rule's Lambda target is invoked, so GetItem(TableName=<stack output TableName>, Key={'id': {'S': <marker>}}) returns the Item the handler wrote from the event detail.
Actual (emulator): "Check timed out after 60.0s" — PutEvents is accepted, the bounded GetItem poll never returns the Item, and no delivery error is reported anywhere.

## Steps to Reproduce
Cause 1:
1. CreateTable with StreamSpecification {StreamEnabled: true, StreamViewType: NEW_AND_OLD_IMAGES}; note LatestStreamArn. CreateTable a plain observation table.
2. Create an execution role trusted by lambda.amazonaws.com with the stream read grants and dynamodb:PutItem on the observation table.
3. CreateFunction whose handler writes one item per received record, keyed by the record's dynamodb.Keys.id.
4. CreateEventSourceMapping(EventSourceArn=<stream arn>, StartingPosition=TRIM_HORIZON, BatchSize=1, FilterCriteria={"Filters":[{"Pattern":"{\"dynamodb\":{\"Keys\":{\"id\":{\"S\":[\"id-keep\"]}}}}"}]}).
5. PutItem {id: 'id-keep'} and PutItem {id: 'id-skip'} into the source table.
6. Scan the observation table.
On AWS: only the id-keep record was delivered.
On the emulator: both id-keep and id-skip were delivered.

Cause 2:
1. CreateStack with a template declaring a DynamoDB table, a Lambda function that writes the event detail's test_marker into that table, an AWS::Events::Rule with an EventPattern on a custom Source plus DetailType 'order-created' and the function as a target, and an AWS::Lambda::Permission granting lambda:InvokeFunction to events.amazonaws.com for the rule ARN. Wait for CREATE_COMPLETE and read the TableName output.
2. PutEvents with one entry matching the rule's Source and DetailType and a Detail carrying a unique marker.
3. Poll GetItem on the output table for Key {'id': {'S': <marker>}}.
On AWS: step 2 returns FailedEntryCount 0 and step 3 returns the Item within seconds.
On the emulator: step 2 reports success, step 3 never returns an Item, and the check is killed at the harness's 60 s cap with no verdict.

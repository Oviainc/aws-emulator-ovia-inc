Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__iot.topic-rule-dynamodbv2-action-writes-item-from-published-message dynamodb__iot.topic-rule-dynamodbv2-action-missing-put-item-permission-rejected iot__s3.rule-s3-action-substitution-template-computes-the-object-key iot__lambda.iot-rule-disabled-rule-no-invocation-negative iot__lambda.lambda-iot-data-publish-positive
Issue-Key: dynamodb__iot.topic-rule-dynamodbv2-action-writes-item-from-published-message|run#30|Timeout|time.sleep(2)
Issue-Key: dynamodb__iot.topic-rule-dynamodbv2-action-missing-put-item-permission-rejected|run#25|Timeout|time.sleep(3)
Issue-Key: iot__s3.rule-s3-action-substitution-template-computes-the-object-key|run#35|Timeout|time.sleep(2)
Issue-Key: iot__lambda.iot-rule-disabled-rule-no-invocation-negative|_receive_all#17|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=max_messages, WaitTimeSeconds=5)
Issue-Key: iot__lambda|TagResource|NotImplemented
Issue-Commit: ba63228bbc1032132c1fbdbb0b54a8c844ce19f7

# IoT topic rules never execute their actions when a matching message is published, and iot TagResource answers 501 NotImplemented (2 causes)

> _Note: this issue was identified against the code base at commit `ba63228bbc1032132c1fbdbb0b54a8c844ce19f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish, TagResource

## Description
Two distinct net-new root causes in the iot emulator, both grounded in failing behavioral checks of the iot rules-engine surface. BOTH must be closed before this issue is done.

=== Cause 1 of 2: the topic-rule engine never evaluates a rule or runs its actions, so a published message produces no downstream effect ===

AWS: `CreateTopicRule` (Smithy: com.amazonaws.iot#CreateTopicRule, POST /rules/{ruleName}, input `topicRulePayload` (TopicRulePayload) required) records a rule whose `sql` selects a topic and whose `actions` (Smithy: com.amazonaws.iot#Action, members include `dynamoDBv2` -> DynamoDBv2Action{roleArn required, putItem: PutItemInput{tableName required}}, `s3` -> S3Action, `lambda` -> LambdaAction, `kinesis`, `sqs`, `republish`, ...) are then EXECUTED by the rules engine for every message published to the matching topic. The documented contract of `dynamoDBv2` is explicit: "Each attribute in the message payload will be written to a separate column in the DynamoDB database". An enabled rule delivers; a rule created with `ruleDisabled=true` does not, and `EnableTopicRule` makes the same rule start delivering. Delivery is asynchronous and is observed on the DOWNSTREAM service (a DynamoDB GetItem, an S3 object, an SQS message), never in the `Publish` response. Execution is also gated on the action role: an action whose `roleArn` grants no permission on the target is dropped silently by the rules engine, not surfaced to the publisher. recorded behavior only for the end-to-end timing (the checks poll the downstream service for up to 180s).

Emulator: the rule is accepted and persisted (services/service-com-amazonaws-iot-awsiotservice/src/ops/create_topic_rule.rs is implemented and `GetTopicRule` reads it back, including `ruleDisabled`), and `iot-data` `Publish` answers 200 with an `OK`/`traceId` acknowledgement, but NOTHING ever runs the rule: no action of any kind fires. Every check that publishes to a rule's topic and then polls the downstream service observes no effect at all and is killed by the harness deadline ("Check timed out after 60.0s") while still polling. This is seen across three independent action types — `dynamoDBv2` (no item ever appears in the table), `s3` (no object ever appears under the substitution-template key), `lambda` (the positive control after `EnableTopicRule` never delivers). The two permission/disabled negatives fail on their own POSITIVE CONTROL (the correctly-permissioned sibling rule / the re-enabled rule), i.e. they cannot even reach the over-permissiveness assertion they exist to make, so the emulator currently cannot be shown to enforce rule-action roles either.

Why: a client that wires IoT eventing against this emulator sees every control-plane call succeed — rule created, rule readable, publish acknowledged — and no message ever arrives anywhere. Rule SQL, substitution templates, per-action role enforcement and the enabled/disabled distinction are all unobservable, so any application whose logic depends on a rule firing silently does nothing, and a test suite that asserts a message reached DynamoDB/S3/SQS/Lambda hangs until its own deadline rather than failing with a clear refusal.

Fix: the rules-engine dispatch that the data-plane publish path must drive — the iot service's topic-rule state/evaluation component plus the hook from the iot-data `Publish` handler (services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops/publish.rs) into it: match the published topic against each stored rule's `sql` FROM clause, skip rules with `ruleDisabled=true`, resolve the action's `roleArn` and drop the action silently when it grants nothing for the target, and otherwise apply the action to the target service's state (one DynamoDB column per top-level message attribute for `dynamoDBv2`, the rendered key for `s3`, the invocation for `lambda`).

Note: `iot__lambda.iot-rule-disabled-rule-no-invocation-negative` additionally depends on a Lambda deployment package actually running, which open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md owns; it is listed here because its failure as replayed is the rules engine never dispatching at all, and the report did not mark it owned. Open issue 20261023T024645Z-iot-createtopicrule-stub.md lists some of these same check ids but is about `CreateTopicRule` answering 501 as a scaffold stub; that handler is now implemented, so it does NOT cover this cause.

Traces: iot__dynamodb.topic-rule-dynamodbv2-action-writes-item-from-published-message, iot__dynamodb.topic-rule-dynamodbv2-action-missing-put-item-permission-rejected, iot__s3.rule-s3-action-substitution-template-computes-the-object-key, iot__lambda.iot-rule-disabled-rule-no-invocation-negative.

=== Cause 2 of 2: iot TagResource is an unimplemented scaffold stub and answers 501 NotImplemented ===

AWS: `TagResource` (Smithy: com.amazonaws.iot#TagResource, POST /tags, input `resourceArn` (ResourceArn) required + `tags` (TagList) required, output an empty TagResourceResponse with HTTP 200; modeled errors InternalFailureException 500, InvalidRequestException 400, LimitExceededException 410, ResourceNotFoundException 404, ThrottlingException 400). Tagging an existing IoT resource — here a topic rule ARN — succeeds with 200 and an empty body.

Emulator: the call is refused with HTTP 501 and error code `NotImplemented`, message e.g. "operation TagResource is scaffolded but not implemented in this emulator" (message quoted as an example). The handler still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker at services/service-com-amazonaws-iot-awsiotservice/src/ops/tag_resource.rs line 4, so nothing is persisted and `ListTagsForResource` can never report the tag.

Why: any client or test that tags an IoT resource after creating it — an extremely common ownership/cleanup convention — fails outright with an unmodeled 501 instead of a 200, aborting the whole workflow before it reaches the behavior it was testing. There is no open issue for this operation (the filed TagResource stub issue, 20261023T122758Z-iot-managed-integrations-tagresource-stub.md, is for iot-managed-integrations, a different service).

Fix: implement the handler in services/service-com-amazonaws-iot-awsiotservice/src/ops/tag_resource.rs against the iot service's tag state (keyed by resourceArn), returning the empty 200 response, `ResourceNotFoundException` for an ARN the store does not hold, and remove the scaffold marker line.

Traces: iot__lambda.lambda-iot-data-publish-positive (the check fails on `iot.tag_resource(resourceArn=<rule arn>, ...)` immediately after creating its topic rule).

## Evidence
Cause 1 — expected: after `CreateTopicRule` with actions=[{"dynamoDBv2": {"roleArn": <role>, "putItem": {"tableName": <table>}}}] and `iot-data` Publish of {"id": <marker>, "temp": 42, "status": "ok"} to the rule's topic, a DynamoDB GetItem on {"id": <marker>} returns an Item with exactly columns id (S), temp (N="42"), status (S="ok"). Actual: no Item ever appears; the check keeps publishing and polling and is killed — suite verdict "[mismatch] Check timed out after 60.0s". Same shape for the S3 action (no object at the substitution-template key) and for the lambda action after `EnableTopicRule` (nothing forwarded to the control queue). The two negatives (`...-missing-put-item-permission-rejected`, `iot-rule-disabled-rule-no-invocation-negative`) fail on their positive control, whose assertion text is "positive control: no item for marker_good=... appeared after repeated publishes — the correctly-permissioned sibling rule must deliver, or this check cannot distinguish 'enforced' from 'broken'" / "expected the positive control (rule enabled) to succeed".

Cause 2 — expected: HTTP 200 with an empty body from POST /tags. Actual: HTTP 501, error code `NotImplemented`: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TagResource operation: operation TagResource is scaffolded but not implemented in this emulator (message is an example; the 501/NotImplemented pair is the divergence). Confirmed in the checkout: services/service-com-amazonaws-iot-awsiotservice/src/ops/tag_resource.rs:4 still carries `//! SCAFFOLDED, NOT IMPLEMENTED:`.

## Steps to Reproduce
Cause 1:
1. Boot the emulator and create a DynamoDB table with hash key `id` (S); create an IAM role trusted by iot.amazonaws.com granting dynamodb:PutItem on that table.
2. iot CreateTopicRule ruleName=<r> topicRulePayload={sql: "SELECT * FROM '<topic>'", awsIotSqlVersion: "2016-03-23", ruleDisabled: false, actions: [{dynamoDBv2: {roleArn: <role arn>, putItem: {tableName: <table>}}}]}.
3. iot-data Publish topic=<topic> qos=1 payload={"id":"m1","temp":42,"status":"ok"} — observe 200 OK.
4. DynamoDB GetItem TableName=<table> Key={"id":{"S":"m1"}} ConsistentRead=true, repeatedly: no Item ever appears. Repeat with an `s3` action (object never written) and with a `lambda` action on a rule created with ruleDisabled=true then EnableTopicRule (nothing ever delivered).
5. Replay: iot__dynamodb.topic-rule-dynamodbv2-action-writes-item-from-published-message, iot__dynamodb.topic-rule-dynamodbv2-action-missing-put-item-permission-rejected, iot__s3.rule-s3-action-substitution-template-computes-the-object-key, iot__lambda.iot-rule-disabled-rule-no-invocation-negative.

Cause 2:
1. iot CreateTopicRule ruleName=<r> (succeeds, 200).
2. iot TagResource resourceArn="arn:aws:iot:<region>:<account>:rule/<r>" tags=[{Key: "k", Value: "v"}].
3. Observe HTTP 501 / NotImplemented instead of an empty 200; replay iot__lambda.lambda-iot-data-publish-positive.

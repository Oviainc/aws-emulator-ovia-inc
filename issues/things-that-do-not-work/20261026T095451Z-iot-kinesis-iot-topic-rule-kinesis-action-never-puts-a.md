Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot__kinesis.topic-rule-kinesis-action-puts-record-from-published-message iot__kinesis.topic-rule-kinesis-action-missing-put-record-permission-rejected
Issue-Key: iot__kinesis.topic-rule-kinesis-action-puts-record-from-published-message|run#32|Timeout|time.sleep(5)
Issue-Key: iot__kinesis.topic-rule-kinesis-action-missing-put-record-permission-rejected|run#27|Timeout|time.sleep(5)
Issue-Commit: fccfb1a16fe5640e548871e12a9114fba6894959

# IoT topic rule `kinesis` action never puts a record on the stream when a matching message is published

> _Note: this issue was identified against the code base at commit `fccfb1a16fe5640e548871e12a9114fba6894959`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
One net-new root cause in the iot rules-engine surface, grounded in the two failing iot__kinesis behavioral checks.

=== Cause 1 of 1: the `kinesis` topic-rule action is never executed, so a published message produces no Kinesis record (and the role-permission boundary on that action is therefore unobservable) ===

AWS: a topic rule recorded by `CreateTopicRule` (Smithy: com.amazonaws.iot#CreateTopicRule, POST /rules/{ruleName}, required input `topicRulePayload` of TopicRulePayload) whose `sql` selects a topic and whose `actions` contain `kinesis` (Smithy: com.amazonaws.iot#Action member `kinesis` -> com.amazonaws.iot#KinesisAction { roleArn (AwsArn) required, streamName (StreamName) required, partitionKey (PartitionKey) optional }; documented as "Describes an action to write data to an Amazon Kinesis stream" / "Write data to an Amazon Kinesis stream") is EXECUTED by the rules engine for every message published to the matching topic: the message body is written as one Kinesis record on the named stream under the given partition key, and is read back with a plain Kinesis GetShardIterator(TRIM_HORIZON)+GetRecords on the stream's shard — the record `Data` decodes to exactly the published payload. Delivery is asynchronous and observed only on the downstream service, never in the `Publish` response (iot-data Publish answers 200 regardless). Execution is gated on the action's `roleArn`: an action whose role grants no `kinesis:PutRecord` on that stream ARN is dropped silently by the rules engine — no record is written and no error is surfaced to the publisher. A rule created with `ruleDisabled=true` does not deliver. recorded behavior only for the end-to-end timing: the checks publish and poll for up to 180s.

Emulator: the control plane all succeeds — Kinesis CreateStream/DescribeStream/AddTagsToStream answer, IAM CreateRole/PutRolePolicy answer, `CreateTopicRule` accepts the rule with the `kinesis` action, and iot-data `Publish` answers 200 — but nothing ever runs the rule's `kinesis` action. No record ever appears on the stream: `GetRecords` from TRIM_HORIZON returns an empty `Records` list on every poll, so both checks keep publishing and re-polling until the harness kills them (suite verdict "[mismatch] Check timed out after 60.0s" for both). The permission-boundary negative never even reaches the over-permissiveness assertion it exists to make, because it fails on its own POSITIVE control (the correctly-permissioned sibling rule on the same stream also delivers nothing), so the emulator cannot currently be shown to enforce `kinesis:PutRecord` on a rule-action role either.

Why: a client wiring IoT -> Kinesis ingestion against this emulator sees every call succeed — stream created, rule created, publish acknowledged — and no telemetry ever lands on the stream. Rule SQL matching, the `partitionKey`, the enabled/disabled distinction and per-action role enforcement for this action type are all unobservable, so an application whose logic depends on the rule firing silently does nothing, and a test that asserts a record reached Kinesis hangs until its own deadline instead of failing with a clear refusal.

Fix: the rules-engine dispatch driven by the data-plane publish path, extended to the `kinesis` action — the iot service's topic-rule evaluation/state component plus the hook from the iot-data `Publish` handler into it: match the published topic against each stored rule's `sql` FROM clause, skip rules with `ruleDisabled=true`, resolve the action's `roleArn` and drop the action silently when it grants no `kinesis:PutRecord` for the target stream ARN, and otherwise append the message body as a record to the named Kinesis stream's shard under the action's `partitionKey`, so a subsequent GetRecords returns it.

Related (does NOT cover this): open issue 20261024T035602Z-iot-topic-rules-never-execute-their-actions-when.md describes the same non-dispatch for the `dynamoDBv2`, `s3` and `lambda` action types and carries only those check ids (iot__dynamodb.*, iot__s3.*, iot__lambda.*); the report did not mark either iot__kinesis check as owned, and the `kinesis` action target is not in its replay key set, so the Kinesis action path is filed here. Fixing the shared dispatch may close both; this issue is closed only when the two iot__kinesis checks below replay green.

Traces: iot__kinesis.topic-rule-kinesis-action-puts-record-from-published-message, iot__kinesis.topic-rule-kinesis-action-missing-put-record-permission-rejected.

## Evidence
Expected: after kinesis CreateStream(StreamName=<stream>, ShardCount=1), an IAM role trusted by iot.amazonaws.com with an inline Allow of kinesis:PutRecord on <stream ARN>, and iot CreateTopicRule(ruleName=<r>, topicRulePayload={sql: "SELECT * FROM '<topic>'", awsIotSqlVersion: "2016-03-23", ruleDisabled: false, actions: [{kinesis: {roleArn: <role arn>, streamName: <stream>, partitionKey: "sim2real-partition"}}]}), an iot-data Publish(topic=<topic>, qos=1, payload={"marker": <marker>, "temp": 42}) is followed within the delivery window by one Kinesis record on the stream's shard whose Data decodes to exactly that published object. Actual: Publish answers 200, and GetShardIterator(TRIM_HORIZON)+GetRecords on that shard returns no records at all, on every poll for the full window; the check is killed by the harness — suite verdict "[mismatch] Check timed out after 60.0s". The assertion the check would otherwise raise is "no Kinesis record carrying marker=<marker> appeared on stream <stream> after <n> publishes over 180.0s" (fixed text; marker, stream name and attempt count come from the request/run).

For iot__kinesis.topic-rule-kinesis-action-missing-put-record-permission-rejected the same non-delivery breaks the POSITIVE control: two sibling rules on the same stream (one role granting kinesis:PutRecord, one granting nothing) both deliver nothing, so the check times out on "positive control: no record for marker_good=<marker> appeared after repeated publishes — the correctly-permissioned sibling rule must deliver, or this check cannot distinguish 'enforced' from 'broken'" (fixed text; marker_good is a per-run value) and never evaluates its real assertion, that the unpermissioned rule delivered nothing. Both checks are [mismatch] / "Check timed out after 60.0s"; neither is marked ALREADY FILED or OWNED in the suite report.

## Steps to Reproduce
1. Boot the emulator. kinesis CreateStream StreamName=<stream> ShardCount=1; wait for stream_exists; DescribeStream to get StreamARN and Shards[0].ShardId.
2. iam CreateRole with a trust policy for Service=iot.amazonaws.com, then PutRolePolicy allowing kinesis:PutRecord on <StreamARN>.
3. iot CreateTopicRule ruleName=<r> topicRulePayload={sql: "SELECT * FROM '<topic>'", awsIotSqlVersion: "2016-03-23", ruleDisabled: false, actions: [{kinesis: {roleArn: <role arn>, streamName: <stream>, partitionKey: "sim2real-partition"}}]} — observe 200.
4. iot-data Publish topic=<topic> qos=1 payload={"marker":"m1","temp":42} — observe 200.
5. kinesis GetShardIterator(StreamName=<stream>, ShardId=<shard>, ShardIteratorType=TRIM_HORIZON) then GetRecords(Limit=100), repeatedly: Records stays empty, no record carrying marker "m1" ever appears.
6. Repeat step 3 with a second rule on a second topic whose role has NO kinesis:PutRecord grant: nothing is delivered for either rule, so 'enforced' is indistinguishable from 'broken'.
7. Replay: iot__kinesis.topic-rule-kinesis-action-puts-record-from-published-message, iot__kinesis.topic-rule-kinesis-action-missing-put-record-permission-rejected.

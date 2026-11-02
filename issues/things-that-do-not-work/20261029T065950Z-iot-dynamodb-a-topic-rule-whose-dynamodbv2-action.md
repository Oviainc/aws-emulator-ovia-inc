Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__iot.topic-rule-dynamodbv2-action-missing-put-item-permission-rejected
Issue-Key: dynamodb__iot.topic-rule-dynamodbv2-action-missing-put-item-permission-rejected|_publish_until_written#24|Timeout|time.sleep(2)
Issue-Commit: c17c6e54c038586d9d6328bb68b9d1b3d6bcf360

# iot__dynamodb: a topic rule whose dynamoDBv2 action lacks dynamodb:PutItem never executes, so the permission-denied contract is unobservable and the check hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `c17c6e54c038586d9d6328bb68b9d1b3d6bcf360`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 1: publishing to a topic that matches a topic rule with a dynamoDBv2 action produces no rule execution at all, so the negative case — the action's role lacking dynamodb:PutItem — cannot be distinguished from a rule that simply never ran.

AWS: an IoT topic rule created with CreateTopicRule and a SQL statement selecting from a topic filter is evaluated on every matching Publish to the message broker. A dynamoDBv2 action assumes the rule's roleArn and calls DynamoDB PutItem with the message payload split into attributes. When that role does not grant dynamodb:PutItem on the target table, the action is refused: DynamoDB answers AccessDeniedException (HTTP 400) with the message "User: <assumed-role principal> is not authorized to perform: dynamodb:PutItem on resource: <table arn> because no identity-based policy allows the dynamodb:PutItem action" (fixed text; the principal and table ARN come from the rule's role and the action's target), the rule's error action / rule-failure path is taken, and critically NO item is written to the table — the positive control (a role that does grant PutItem) writes, the negative one does not. Recorded behavior: the check expects this differential to settle within its wait budget.

Emulator: nothing happens on either leg. The check hangs until the harness's 60 s cap and is reported as "Check timed out after 60.0s". No rule execution is observable: the denied action produces no refusal and no error-action signal, and the control that must write produces no item, so there is no evidence the topic rule engine evaluated the Publish at all.

Why: a client that wires device telemetry straight into DynamoDB through a topic rule gets a rule the control plane accepts and an ingestion path that silently delivers nothing. Worse for the negative case: a mis-scoped rule role is indistinguishable from a correctly scoped one, so an IAM regression in a device-ingestion pipeline cannot be detected, and the "denied action writes nothing" guarantee the design depends on is vacuous.

Fix: the IoT rules-engine execution component — the path that must evaluate a topic rule's SQL against a matching Publish and run its actions, the dynamoDBv2 action's role assumption and authorization against DynamoDB, and the resulting DynamoDB AccessDeniedException/error-action handling. A sibling open issue, 20261024T035602Z-iot-topic-rules-never-execute-their-actions-when.md, covers the positive dynamoDBv2 leg but does not carry this check's replay key or the permission-denied contract.

Traces: iot__dynamodb.topic-rule-dynamodbv2-action-missing-put-item-permission-rejected

## Evidence
Expected (AWS): after Publish to the rule's topic,
  - with a role granting dynamodb:PutItem: the item appears in the table;
  - with a role that does NOT grant it: the dynamoDBv2 action is refused with DynamoDB's AccessDeniedException, HTTP 400, message "User: <assumed-role principal> is not authorized to perform: dynamodb:PutItem on resource: <table arn> because no identity-based policy allows the dynamodb:PutItem action" (fixed text; principal and table ARN from the rule role and action target), and the table stays empty.

Actual (emulator): neither leg produces anything; the rule is never executed and the check never reaches a verdict.
  #### [mismatch] iot__dynamodb.topic-rule-dynamodbv2-action-missing-put-item-permission-rejected
  Error: Check timed out after 60.0s

## Steps to Reproduce
1. CreateTable with a hash key; wait for ACTIVE.
2. Create an IAM role trusted by iot.amazonaws.com WITHOUT dynamodb:PutItem on that table (and a sibling role WITH it, as the positive control).
3. CreateTopicRule(ruleName, topicRulePayload={sql: "SELECT * FROM '<topic>'", actions: [{dynamoDBv2: {roleArn: <role>, putItem: {tableName: <table>}}}]}).
4. Publish a JSON payload to <topic> via the IoT data plane.
5. Poll the table and the rule's failure signal.
Observed: nothing is written and no refusal is observable on either the denied or the permitted role; the flow never settles. Expected: the permitted role writes the item; the denied role yields DynamoDB AccessDeniedException and leaves the table empty.

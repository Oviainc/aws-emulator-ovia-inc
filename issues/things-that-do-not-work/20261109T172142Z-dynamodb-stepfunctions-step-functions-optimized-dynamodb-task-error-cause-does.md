Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__stepfunctions.delete-item-task-removes-the-item-and-a-failed-condition-leaves-it-untouched
Issue-Key: dynamodb__stepfunctions.delete-item-task-removes-the-item-and-a-failed-condition-leaves-it-untouched|run#17|AssertionError|assert masked == (
Issue-Commit: 545841f07feb57b92e392d55db412e521f655a7b

# Step Functions optimized DynamoDB task error Cause does not use AWS's SDK-exception format

> _Note: this issue was identified against the code base at commit `545841f07feb57b92e392d55db412e521f655a7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: the Cause string a failed optimized DynamoDB task puts on the execution history diverges from AWS's.

AWS: when an optimized `arn:aws:states:::dynamodb:deleteItem` task is refused by DynamoDB, Step Functions fails the task with `Error` = `DynamoDB.ConditionalCheckFailedException` and a `Cause` rendered from the Java SDK exception, whose fixed shape is `<message> (Service: AmazonDynamoDBv2; Status Code: <code>; Error Code: <ErrorCode>; Request ID: <request-id>; Proxy: null)` — the message, status code, error code and request id are taken from the DynamoDB response, and the field separators, the `Service: AmazonDynamoDBv2` token and the `Proxy: null` suffix are fixed text. The task failure must also leave the item untouched.

Emulator: the Cause the execution carries is `The conditional request failed (Service: AmazonDynamoDBv2; Status Code: 400; Error Code: ConditionalCheckFailedException; Request ID: <id>; Proxy: null)` but it is not the string the execution history is expected to carry for this task — the check's assertion on the caught cause fails, i.e. the relayed cause does not match AWS's rendering for the optimized integration (field set/order/placement differs from the recorded one).

Why: state machines branch on `Error` and parse `Cause` in Catch/Retry blocks and in error-reporting steps. A cause string in a different shape breaks those parsers and the operator's diagnosis.

Fix: the optimized DynamoDB service-integration error relay in the Step Functions task handler — it must render the DynamoDB refusal into the SDK-exception cause format, with the request id and status taken from the DynamoDB response.

Traces: dynamodb__stepfunctions.delete-item-task-removes-the-item-and-a-failed-condition-leaves-it-untouched. A sibling cause for the request-id relayed in a Step Functions task is already filed under another grouping as 20261109T143515Z-dynamodb-logs-stepfunctions-the-dynamodb-request-id-relayed-in-a-step.md; that issue does not carry this check's replay key.

## Evidence
Expected: the task's Cause matches AWS's optimized-integration rendering of the DynamoDB refusal (fixed text "(Service: AmazonDynamoDBv2; Status Code: <code>; Error Code: <ErrorCode>; Request ID: <request-id>; Proxy: null)" with the variable parts taken from the DynamoDB response), and the item is left untouched.
Actual: AssertionError: the caught cause was 'The conditional request failed (Service: AmazonDynamoDBv2; Status Code: 400; Error Code: ConditionalCheckFailedException; Request ID: <id>; Proxy: null)' (the request id is a per-run value and is not itself the defect).

## Steps to Reproduce
1. Create a table and an item.
2. Create a state machine with an optimized arn:aws:states:::dynamodb:deleteItem task carrying a ConditionExpression that cannot be satisfied, wrapped in a Catch.
3. StartExecution and read the caught Error/Cause from the execution history.
4. Compare the Cause with AWS's SDK-exception rendering, and verify the item is unchanged.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__events__lambda.ddb-stream-esm-destination-missing-permission-rejected
Issue-Key: dynamodb__events__lambda.ddb-stream-esm-destination-missing-permission-rejected|run#9|Timeout|time.sleep(4.0)
Issue-Commit: ea825c3b5f0a7c3e6fb8963cf0e8152afa9f60b5

# dynamodb__events__lambda: a DynamoDB-Streams ESM with an OnFailure SQS destination neither refuses a role without sqs:SendMessage nor ever delivers a RetryAttemptsExhausted failure record (1 cause)

> _Note: this issue was identified against the code base at commit `ea825c3b5f0a7c3e6fb8963cf0e8152afa9f60b5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One net-new root cause in this service group; no open issue carries this check's replay key.

Cause 1 of 1: the OnFailure failure-destination contract of a DynamoDB-Streams event source mapping is not served, so the check never reaches a verdict and is killed by the harness at 60 s. Failing check: dynamodb__events__lambda.ddb-stream-esm-destination-missing-permission-rejected.

AWS: Smithy (lambda): CreateEventSourceMapping (POST /2015-03-31/event-source-mappings, success 202) takes DestinationConfig and MaximumRetryAttempts (range -1..10000) and models InvalidParameterValueException as a 400 client error. AWS either refuses the create when the function's execution role cannot write to the declared OnFailure destination — InvalidParameterValueException with the fixed message "The function execution role does not have permissions to call SendMessage on SQS" (fixed text; it carries no request-derived values) — or accepts the mapping and then never writes anything to that destination. With the permission granted and MaximumRetryAttempts=0, a handler that raises makes the poller give up immediately and write ONE failure record to the OnFailure SQS queue; that record's requestContext.condition is the fixed literal "RetryAttemptsExhausted", requestContext.functionArn is the ARN of the failing function (value from state), and the record carries a DDBStreamBatchInfo object whose streamArn equals the table's stream ARN from the request (recorded behavior for the record's member set; the Smithy model does not describe the failure-record document).
Emulator: the check records only "Check timed out after 60.0s" — no verdict. Both of its legs are bounded waits on the emulator: the negative leg polls the OnFailure queue for 60 s asserting that no delivery happens (entered only when the create was NOT refused), and the positive leg polls for up to 120 s for the failure record. Hypothesis: the create-time permission refusal does not fire (so the negative leg's 60 s no-delivery watch is entered and alone exhausts the harness budget) and the failure destination is never written either, consistent with the already-filed finding that DDB-stream ESMs in this emulator never deliver records at all.
Why: a client that wires an OnFailure SQS destination to a stream mapping gets neither of AWS's two answers: it is not told at create time that its execution role lacks sqs:SendMessage (so a mis-scoped role ships silently), and it never receives the RetryAttemptsExhausted record it relies on to reprocess or alert, so poison-batch handling is inert and indistinguishable from "nothing failed".
Fix: the lambda crate's CreateEventSourceMapping validation (evaluate the execution role's access to the declared DestinationConfig.OnFailure destination and refuse with 400 InvalidParameterValueException and AWS's message) and the event-source-mapping poller/state that must, on exhausting MaximumRetryAttempts, emit the failure record to the configured SQS destination with requestContext.condition="RetryAttemptsExhausted", requestContext.functionArn and DDBStreamBatchInfo.streamArn.
Traces: dynamodb__events__lambda.ddb-stream-esm-destination-missing-permission-rejected.
Related open issues that describe neighbouring symptoms but do NOT carry this check's replay key: 20261027T061147Z-dynamodb-events-lambda-a-dynamodb-streams-event-source.md and 20261029T180026Z-dynamodb-events-lambda-eventbridge-rules-never-invoke-their-lambda-targets-ddb.md.

## Evidence
Cause 1 — Expected (AWS): CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn=<table stream arn>, BatchSize=1, StartingPosition=TRIM_HORIZON, MaximumRetryAttempts=0, DestinationConfig={"OnFailure": {"Destination": <queue arn>}}) with an execution role that has the dynamodb:GetRecords/GetShardIterator/DescribeStream/ListStreams grants but NO sqs:SendMessage is either refused 400 InvalidParameterValueException "The function execution role does not have permissions to call SendMessage on SQS", or accepted and then nothing is ever delivered to that queue. The same mapping with sqs:SendMessage granted delivers exactly one failure record whose requestContext.condition == "RetryAttemptsExhausted", whose requestContext.functionArn names the failing function, and which contains DDBStreamBatchInfo.streamArn == the table's stream ARN.
Actual (emulator): "Check timed out after 60.0s" — the check never reaches either assertion. No create-time refusal is observed (the check proceeds into its 60 s no-delivery watch) and no failure record is observed on the permitted leg.

## Steps to Reproduce
1. CreateTable with StreamSpecification enabled and note its LatestStreamArn; CreateQueue and note its ARN.
2. Create an execution role trusted by lambda.amazonaws.com with logs:* and dynamodb:GetRecords/GetShardIterator/DescribeStream/ListStreams, and deliberately NO sqs:SendMessage.
3. CreateFunction whose handler raises RuntimeError('boom').
4. CreateEventSourceMapping(BatchSize=1, StartingPosition=TRIM_HORIZON, MaximumRetryAttempts=0, DestinationConfig={"OnFailure": {"Destination": <queue arn>}}).
5. If step 4 was accepted, PutItem into the table and poll ReceiveMessage on the queue — nothing must arrive.
6. Repeat 1-4 with sqs:SendMessage granted on the queue, PutItem, then poll ReceiveMessage.
On AWS: step 4 is refused 400 InvalidParameterValueException "The function execution role does not have permissions to call SendMessage on SQS" (or accepted with no delivery in step 5), and step 6 yields one record with requestContext.condition="RetryAttemptsExhausted", requestContext.functionArn and DDBStreamBatchInfo.streamArn.
On the emulator: no refusal and no failure record; the check is killed at the harness's 60 s cap without a verdict.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns__sqs.dead-letter-topic-relays-the-original-event-with-lambda-error-attributes lambda__sns__sqs.dead-letter-topic-the-execution-role-cannot-publish-to-is-rejected lambda__sns__sqs.on-success-destination-topic-publishes-the-invocation-record-as-the-message lambda__sns__sqs.sns-lambda-destination-onfailure-to-sqs lambda__sns__sqs.snstrig-execution-role-without-sqs-grant-rejects-onfailure-destination lambda__sns__sqs.snstrig-lambda-async-retry-exhausted-onfailure-to-sqs
Issue-Key: lambda__sns__sqs.dead-letter-topic-relays-the-original-event-with-lambda-error-attributes|_receive#11|Timeout|for m in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=wait_s,
Issue-Key: lambda__sns__sqs.dead-letter-topic-the-execution-role-cannot-publish-to-is-rejected|_error#8|AssertionError|raise AssertionError(f"call was expected to fail but succeeded: {result!r}")
Issue-Key: lambda__sns__sqs.on-success-destination-topic-publishes-the-invocation-record-as-the-message|_receive#10|Timeout|for m in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=wait_s,
Issue-Key: lambda__sns__sqs.sns-lambda-destination-onfailure-to-sqs|_poll_for_message#14|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=5)
Issue-Key: lambda__sns__sqs.snstrig-execution-role-without-sqs-grant-rejects-onfailure-destination|run#9|AssertionError|assert message == f"The function execution role does not have permissions to call SendMessage on {queue_arn}", (
Issue-Key: lambda__sns__sqs.snstrig-lambda-async-retry-exhausted-onfailure-to-sqs|_publish_until_delivered#14|Timeout|sent[marker] = sns.publish(TopicArn=topic_arn, Message=marker)["MessageId"]
Issue-Commit: 486a0852ab6c107011e8dec7ea6574940ef62e53

# lambda__sns__sqs: async destinations and dead-letter targets on SNS/SQS never deliver, an ungrantable SNS dead-letter topic is accepted, and the OnFailure refusal message is rewritten (3 causes)

> _Note: this issue was identified against the code base at commit `486a0852ab6c107011e8dec7ea6574940ef62e53`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFunctionConfiguration

## Description
Three net-new root causes on Lambda's asynchronous-invocation plane as seen through SNS and SQS. None of these failing checks is marked ALREADY FILED or OWNED in the replay report. The open issue 20261105T022225Z-lambda-sns-sqs-2-net-new-causes-the.md already describes the same delivery gap for a CloudFormation-declared AWS::Lambda::EventInvokeConfig (its Cause 2); the checks below exercise the direct API doors (PutFunctionEventInvokeConfig and UpdateFunctionConfiguration) and the dead-letter path, and are not carried by that issue's replay keys.

Cause 1 of 3: an asynchronous invocation's OnSuccess/OnFailure destination and its DeadLetterConfig target never receive anything when the target is an SNS topic or an SQS queue.
AWS: Smithy (lambda PutFunctionEventInvokeConfig -> DestinationConfig{OnSuccess,OnFailure}, MaximumRetryAttempts @range 0..2; UpdateFunctionConfiguration -> DeadLetterConfig.TargetArn) models the configuration; the delivery is recorded behavior only. On an async Invoke (InvocationType=Event, accepted 202): a successful invocation publishes the invocation record (version/timestamp/requestContext/requestPayload/responsePayload) to an OnSuccess topic as the SNS Message; a handler that exhausts its retries publishes the record with requestContext.condition 'RetriesExhausted' to the OnFailure destination; and a DeadLetterConfig SNS topic receives the ORIGINAL event with the Lambda error message attributes (RequestID, ErrorCode, ErrorMessage).
Emulator: nothing is ever published or sent. All four checks are abandoned with 'Error: Check timed out after 60.0s' while polling the SQS queue subscribed to the topic (or the destination queue directly).
Why: a client that fans async successes or failures out through SNS for alerting and replay loses every record silently; nothing distinguishes 'no failure happened' from 'the failure was dropped'.
Fix: the Lambda asynchronous-invocation pipeline's destination and dead-letter delivery (SNS Publish for a topic ARN, SendMessage for a queue ARN), including the record envelope and the dead-letter message attributes.
Traces: lambda__sns__sqs.dead-letter-topic-relays-the-original-event-with-lambda-error-attributes, lambda__sns__sqs.on-success-destination-topic-publishes-the-invocation-record-as-the-message, lambda__sns__sqs.sns-lambda-destination-onfailure-to-sqs, lambda__sns__sqs.snstrig-lambda-async-retry-exhausted-onfailure-to-sqs.

Cause 2 of 3: UpdateFunctionConfiguration accepts a DeadLetterConfig SNS topic the execution role cannot publish to.
AWS: Smithy (lambda UpdateFunctionConfiguration -> DeadLetterConfig; error InvalidParameterValueException, httpError 400). A DeadLetterConfig.TargetArn naming an SNS topic the execution role has no sns:Publish on is refused with HTTP 400 InvalidParameterValueException and the message 'The provided execution role does not have permissions to call Publish on SNS' — that sentence is entirely fixed text (it names the API and the service, never the topic) — and no DeadLetterConfig is stored; once the grant exists the identical call is accepted.
Emulator: the call is accepted with HTTP 200 and the response echoes DeadLetterConfig.TargetArn = the ungranted topic ('AssertionError: call was expected to fail but succeeded: {... HTTPStatusCode: 200 ... DeadLetterConfig: {TargetArn: arn:aws:sns:...}}').
Why: a misconfiguration AWS rejects at configure time becomes a silent runtime hole — dead-lettered events are dropped with no refusal anywhere. This is the same missing check as the open issue 20261102T090835Z-lambda-logs-sqs-createfunction-accepts-a-deadletterconfig-the-execution-role-may.md reports for CreateFunction with an SQS target; the SNS target on UpdateFunctionConfiguration is a separate door.
Fix: the UpdateFunctionConfiguration handler's DeadLetterConfig validation (resolve the target service from the ARN, authorize the execution role for sns:Publish / sqs:SendMessage before storing).
Traces: lambda__sns__sqs.dead-letter-topic-the-execution-role-cannot-publish-to-is-rejected.

Cause 3 of 3: the OnFailure-destination refusal message is rewritten and drops the queue ARN.
AWS: Smithy (lambda PutFunctionEventInvokeConfig -> DestinationConfig; error InvalidParameterValueException, httpError 400). An OnFailure destination the execution role cannot sqs:SendMessage to is refused 400 InvalidParameterValueException with 'The function execution role does not have permissions to call SendMessage on <queue ARN>' — fixed text 'The function execution role does not have permissions to call ', the action implied by the destination type, ' on ', then the destination ARN from the request.
Emulator: the message is 'The provided execution role does not have permissions to call SendMessage on SQS' — wrong subject and the ARN replaced by the service name. (The resolved issue 20261030T121631Z-events-lambda-sqs-createeventsourcemapping-s-under-privileged-execution.md corrected this wording for CreateEventSourceMapping only.)
Why: operators and error handlers read that sentence to learn WHICH destination was rejected; with 'SQS' in place of the ARN the offending destination cannot be identified, and exact-message comparisons fail.
Fix: the destination-validation refusal in the PutFunctionEventInvokeConfig handler (message template and ARN interpolation).
Traces: lambda__sns__sqs.snstrig-execution-role-without-sqs-grant-rejects-onfailure-destination.

## Evidence
Cause 1 — Expected: the invocation record published to the OnSuccess/OnFailure SNS topic (visible on the subscribed queue) and the original event with RequestID/ErrorCode/ErrorMessage attributes on the dead-letter topic. Actual: 'Error: Check timed out after 60.0s' on all four listed checks; the subscribed queue stays empty.
Cause 2 — Expected: HTTP 400 InvalidParameterValueException 'The provided execution role does not have permissions to call Publish on SNS' and no stored DeadLetterConfig. Actual: 'AssertionError: call was expected to fail but succeeded: {ResponseMetadata: {HTTPStatusCode: 200, ...}, ... DeadLetterConfig: {TargetArn: arn:aws:sns:<region>:<account>:<topic>} ...}' (the ARNs and ids there are per-run values).
Cause 3 — Expected: 'The function execution role does not have permissions to call SendMessage on <queue ARN>'. Actual: 'AssertionError: unexpected rejection message: \'The provided execution role does not have permissions to call SendMessage on SQS\''.

## Steps to Reproduce
Cause 1: create a topic, subscribe an SQS queue, create a function whose role may sns:Publish; PutFunctionEventInvokeConfig MaximumRetryAttempts=0 with DestinationConfig.OnSuccess.Destination = the topic ARN; Invoke InvocationType=Event and poll the queue. Repeat with OnFailure plus a raising handler, and with UpdateFunctionConfiguration DeadLetterConfig.TargetArn = the topic ARN. Cause 2: UpdateFunctionConfiguration DeadLetterConfig.TargetArn naming a topic the role has no sns:Publish on — the call answers 200. Cause 3: PutFunctionEventInvokeConfig with DestinationConfig.OnFailure.Destination naming a queue the role has no sqs:SendMessage on — read Error.Message. Replay the check ids in trace_ids.

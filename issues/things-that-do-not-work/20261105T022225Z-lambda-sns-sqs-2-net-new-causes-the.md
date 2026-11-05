Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns__sqs.cfn-deploy-iam-role-trust-mismatch-negative lambda__sns__sqs.cfn-deploy-lambda-destination-sns-on-failure
Issue-Key: lambda__sns__sqs.cfn-deploy-iam-role-trust-mismatch-negative|run#15|AssertionError|assert reason_text == (
Issue-Key: lambda__sns__sqs.cfn-deploy-lambda-destination-sns-on-failure|_probe#14|Timeout|r = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=5, WaitTimeSeconds=5)
Issue-Commit: 3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713

# lambda__sns__sqs: 2 net-new causes — the unassumable-role CREATE_FAILED reason truncates the quoted handler message, and an async OnFailure destination on an SNS topic never publishes the invocation record

> _Note: this issue was identified against the code base at commit `3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
Two distinct root causes. Neither failing check is marked OWNED or ALREADY FILED in the replay report.

Cause 1 of 2: the CREATE_FAILED ResourceStatusReason for an AWS::Lambda::Function whose role cannot be assumed drops the service-diagnostic clause AWS embeds inside the quoted handler message.
AWS: recorded behavior only (CloudFormation resource-handler error wrapping). When the declared execution role's trust policy does not admit lambda.amazonaws.com, the Function resource goes CREATE_FAILED and its ResourceStatusReason is `Resource handler returned message: "The role defined for the function cannot be assumed by Lambda. (Service: Lambda, Status Code: 400, Request ID: <request id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)`. The fixed text is `Resource handler returned message: "The role defined for the function cannot be assumed by Lambda. (Service: Lambda, Status Code: 400, Request ID: ` ... `) (SDK Attempt Count: ` ... `)" (RequestToken: ` ... `, HandlerErrorCode: InvalidRequest)`; the variable parts are the Lambda request id, the SDK attempt count and the CloudFormation request token.
Emulator: the quote is closed immediately after the sentence — `Resource handler returned message: "The role defined for the function cannot be assumed by Lambda." (RequestToken: <token>, HandlerErrorCode: InvalidRequest)` — so the embedded `(Service: Lambda, Status Code: 400, Request ID: ...) (SDK Attempt Count: ...)` clause is absent and the closing quote lands in the wrong place.
Why: tooling that parses stack-event reasons to attribute a failure (splitting on ` (Service: ` to recover the handler sentence, or reading the Status Code / Service to decide whether the failure is a client error from Lambda) gets a reason it cannot parse, and loses the service attribution and status code AWS supplies.
Fix: the CloudFormation resource-handler error wrapping for AWS::Lambda::Function — the underlying Lambda client error's service/status/request-id clause must be carried inside the quoted handler message before the RequestToken/HandlerErrorCode suffix.
Traces: lambda__sns__sqs.cfn-deploy-iam-role-trust-mismatch-negative.

Cause 2 of 2: an asynchronous invocation's OnFailure destination pointing at an SNS topic never delivers the invocation record.
AWS: recorded behavior only. With an AWS::Lambda::EventInvokeConfig declaring MaximumRetryAttempts 0 and DestinationConfig.OnFailure.Destination set to an SNS topic ARN, an async Invoke (InvocationType=Event, accepted with 202) whose handler always raises exhausts its retries immediately and Lambda publishes an invocation record to that topic. The record carries requestPayload (the exact payload submitted), requestContext.condition == "RetriesExhausted", and responsePayload.errorMessage containing the handler's error text.
Emulator: nothing is ever published. The check subscribes an SQS queue to the topic, forces the failing async invocation and polls for 90 s, then is abandoned with "Check timed out after 60.0s" — no verdict is reached.
Why: a client that routes async failures to a topic for alerting or replay loses every failed invocation silently; neither the payload that failed nor the reason it failed is recoverable, and the "RetriesExhausted" condition that distinguishes an exhausted retry from a throttle is never emitted.
Fix: the Lambda asynchronous-invocation pipeline — after retries are exhausted it must build the invocation record and deliver it to the configured OnFailure destination (SNS Publish for a topic ARN), and the CloudFormation provider for AWS::Lambda::EventInvokeConfig must register that destination.
Traces: lambda__sns__sqs.cfn-deploy-lambda-destination-sns-on-failure.

## Evidence
Cause 1 — expected ResourceStatusReason text up to ` (Service: `: `Resource handler returned message: "The role defined for the function cannot be assumed by Lambda.`
Actual (replay): `AssertionError: expected the Function resource's CREATE_FAILED reason to name the unassumable role, got 'Resource handler returned message: "The role defined for the function cannot be assumed by Lambda." (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'` — the quote closes after the sentence and the `(Service: Lambda, Status Code: 400, Request ID: ...) (SDK Attempt Count: ...)` clause is missing. The request token is a per-run value.

Cause 2 — expected: an invocation record on the SNS-fed queue with requestPayload.marker == the submitted marker, requestContext.condition == "RetriesExhausted", and "forced failure" in responsePayload.errorMessage.
Actual (replay): `Error: Check timed out after 60.0s` — the async Invoke is accepted with 202 but no record is ever published to the topic.

## Steps to Reproduce
Cause 1: CreateStack with a template declaring an IAM role whose trust policy names a principal other than lambda.amazonaws.com and an AWS::Lambda::Function using that role; wait for the terminal status and call DescribeStackEvents — the Function's CREATE_FAILED ResourceStatusReason omits the `(Service: Lambda, Status Code: 400, Request ID: ...) (SDK Attempt Count: ...)` clause inside the quoted handler message.
Cause 2: CreateStack with a template declaring an SNS topic, a Lambda function whose handler always raises, and an AWS::Lambda::EventInvokeConfig with MaximumRetryAttempts 0 and DestinationConfig.OnFailure.Destination = {"Ref": "Topic"}; subscribe an SQS queue to the topic; call Invoke with InvocationType=Event (202 is returned); poll the queue — no invocation record ever arrives.

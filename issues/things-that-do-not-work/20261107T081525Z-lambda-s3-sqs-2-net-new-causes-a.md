Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__sqs.deploy-lambda-destination-onfailure-to-sqs-positive lambda__s3__sqs.deploy-lambda-destination-missing-permission-negative
Issue-Key: lambda__s3__sqs.deploy-lambda-destination-onfailure-to-sqs-positive|_receive_one#9|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=5)
Issue-Key: lambda__s3__sqs.deploy-lambda-destination-missing-permission-negative|_receive_one#10|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=wait_s)
Issue-Commit: 145c20a94e0bd5c99471752c6f247be888f6ff73

# lambda__s3__sqs: 2 net-new causes — a CFN-declared Lambda OnFailure destination never delivers the invocation record to SQS, and the destination's sqs:SendMessage permission is never checked at EventInvokeConfig creation

> _Note: this issue was identified against the code base at commit `145c20a94e0bd5c99471752c6f247be888f6ff73`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutFunctionEventInvokeConfig

## Description
Two root causes in the asynchronous-destination path of this scenario service.

Cause 1 of 2: a CloudFormation-declared async OnFailure destination to SQS never receives the failure invocation record.
AWS: recorded behavior only for the delivery (Lambda's Smithy model declares `PutFunctionEventInvokeConfig` / `DestinationConfig.OnFailure.Destination` but states no runtime rule). The recording shows that after the function raises on an asynchronous invoke and exhausts its retries, Lambda writes an invocation record to the OnFailure destination queue — a JSON document carrying the request context (requestId, function ARN, condition `RetriesExhausted`), the original request payload and the error response.
Emulator: no message ever reaches the destination queue; the check polls until its budget expires and reports `Check timed out after 60.0s`.
Why: the documented failure-capture mechanism for async Lambda is silent, so a client loses every failed event instead of receiving its record — dead-letter/retry tooling built on OnFailure destinations never sees anything.
Fix: the Lambda asynchronous invocation path — the async invoke queue/retry driver and the destination dispatch it owes (the lambda service's async invocation component), reached here through CloudFormation's AWS::Lambda::EventInvokeConfig provisioning. Related but not covering: 20261107T014900Z-lambda-sqs-the-asynchronous-invocation-plane-is-inert.md is filed against the `lambda__sqs` scenario service; these checks are the `lambda__s3__sqs` CloudFormation-declared path and are not owned by it in this run.
Traces: lambda__s3__sqs.deploy-lambda-destination-onfailure-to-sqs-positive.

Cause 2 of 2: the OnFailure destination's `sqs:SendMessage` permission is not validated when the EventInvokeConfig is created, and the matching positive control never delivers.
AWS: recorded behavior only. Lambda checks the execution role's `sqs:SendMessage` on the OnFailure destination at the moment the EventInvokeConfig is created: a stack whose role lacks it fails to deploy (the EventInvokeConfig resource is refused, so the stack does not reach CREATE_COMPLETE), while an identical stack whose role grants it deploys and then delivers the failed invocation's record to the queue.
Emulator: neither half is observable — the check exhausts its 60s budget, so the under-permissioned stack is not refused at create time and the permitted control stack never delivers its record either.
Why: a client gets a stack that deploys clean and then silently drops every failure record, instead of AWS's fail-fast refusal at deploy time; the misconfiguration is discovered only when failures are already lost.
Fix: the PutFunctionEventInvokeConfig handler's destination-permission validation (the lambda service's event-invoke-config validation), plus the same async destination dispatch as Cause 1.
Traces: lambda__s3__sqs.deploy-lambda-destination-missing-permission-negative.

## Evidence
Cause 1 — deploy-lambda-destination-onfailure-to-sqs-positive: 'Check timed out after 60.0s'; the check deploys a CFN stack wiring a Lambda async OnFailure destination to an SQS queue, invokes the function asynchronously so it raises, then polls DestQueueUrl for the invocation record — none arrives.
Cause 2 — deploy-lambda-destination-missing-permission-negative: 'Check timed out after 60.0s'; the check deploys a control stack (role grants sqs:SendMessage) and a broken stack (role omits it) and expects the broken deploy to fail and the control to deliver; neither is observed.

## Steps to Reproduce
1. CreateStack with a Lambda function, an execution role granting sqs:SendMessage on a destination queue, and an AWS::Lambda::EventInvokeConfig with DestinationConfig.OnFailure pointing at that queue.
2. Invoke the function with InvocationType=Event so it raises; poll the destination queue — no invocation record ever arrives.
3. Deploy an identical stack whose role omits sqs:SendMessage on the destination: the stack is not refused at EventInvokeConfig creation as AWS refuses it.

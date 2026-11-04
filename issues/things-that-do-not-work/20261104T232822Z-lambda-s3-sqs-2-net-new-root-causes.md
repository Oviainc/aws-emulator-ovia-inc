Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__sqs.deploy-lambda-destination-onfailure-to-sqs-positive lambda__s3__sqs.deploy-lambda-destination-missing-permission-negative lambda__s3__sqs.deploy-s3-batch-notification-multiple-lambda-targets-positive
Issue-Key: lambda__s3__sqs.deploy-lambda-destination-onfailure-to-sqs-positive|_poll_until#9|Timeout|last = probe()
Issue-Key: lambda__s3__sqs.deploy-lambda-destination-missing-permission-negative|_poll_until#10|Timeout|last = probe()
Issue-Key: lambda__s3__sqs.deploy-s3-batch-notification-multiple-lambda-targets-positive|_poll_until#13|Timeout|last = probe()
Issue-Commit: 6ff397d47b7f23b010f8590f84afedb6b986e178

# lambda__s3__sqs: 2 net-new root causes — async OnFailure destinations never deliver a failure record to SQS, and CFN-deployed prefix-scoped S3 notifications never invoke their Lambda targets

> _Note: this issue was identified against the code base at commit `6ff397d47b7f23b010f8590f84afedb6b986e178`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutFunctionEventInvokeConfig

## Description
Two distinct net-new root causes in this service; both must be closed before this issue is submitted.

Cause 1 of 2: a Lambda asynchronous OnFailure destination wired to an SQS queue never receives the failure invocation record.
AWS: recorded behavior only for the delivery envelope (the configuration itself is lambda#PutFunctionEventInvokeConfig / AWS::Lambda::EventInvokeConfig with DestinationConfig.OnFailure.Destination = the queue ARN). After an Event-type Invoke of a handler that always raises, Lambda exhausts its retries and sends one message to the destination queue whose JSON body carries `requestContext.condition == "RetriesExhausted"` and a `responsePayload.errorMessage` echoing the invocation payload (here the id "c1" supplied in the request).
Emulator: ReceiveMessage on the stack's destination queue never returns a message — the poll for the control's OnFailure delivery runs out and the check is cut off by the harness at 60.0 s. No failure record is produced for either the plain positive stack or the control half of the permission check.
AWS additionally refuses the negative half of deploy-lambda-destination-missing-permission-negative at deploy time: a stack whose function role lacks sqs:SendMessage on its own destination queue never reaches CREATE_COMPLETE — it ends ROLLBACK_COMPLETE/CREATE_FAILED with a failure reason naming the SendMessage permission. That assertion was never reached because the control delivery above never happened, so it is covered by this same cause and must be verified when it is fixed.
Why: OnFailure destinations are how clients capture asynchronous invocation failures. With no record ever delivered, a failed async invocation disappears silently, and the deploy-time permission gate that AWS applies to the destination is untestable.
Fix: the Lambda asynchronous-invocation path and its EventInvokeConfig destination dispatch (emit the RetriesExhausted record to the OnFailure destination), plus the permission validation PutFunctionEventInvokeConfig applies to the destination.
Traces: lambda__s3__sqs.deploy-lambda-destination-onfailure-to-sqs-positive, lambda__s3__sqs.deploy-lambda-destination-missing-permission-negative.

Cause 2 of 2: two prefix-scoped S3 notification rules on one CFN-deployed bucket invoke neither of their Lambda targets.
AWS: recorded behavior only. A bucket with two LambdaFunctionConfigurations, each filtered to a different key prefix and pointing at a different function, invokes exactly the function wired to the matching prefix for each uploaded object, and only that one.
Emulator: neither function is invoked — the check never observes a per-prefix effect and is cut off by the harness at 60.0 s. The notification configuration is accepted by the stack but nothing is ever delivered, so prefix scoping cannot even be distinguished from total non-delivery.
Why: prefix-scoped fan-out is the standard way one bucket feeds several consumers. With no delivery at all, both the positive routing and the "only that one" isolation guarantee are lost, while the control plane still reports the rules as configured.
Fix: the S3 object-write notification dispatch (prefix/suffix filter matching and the asynchronous Lambda invoke per matching configuration) and the CloudFormation AWS::S3::Bucket NotificationConfiguration handler that installs the two rules.
The open issue 20261104T044826Z-lambda-s3-sqs-s3-bucket-notifications-never-invoke-their-lambda-target.md reports the same non-delivery for other checks in this service but does not carry this check's replay key.
Traces: lambda__s3__sqs.deploy-s3-batch-notification-multiple-lambda-targets-positive.

## Evidence
Cause 1 — expected: after `lam.invoke(FunctionName=<stack function>, InvocationType="Event", Payload={"id":"c1"})` on an always-failing handler, ReceiveMessage on the stack's DestQueueUrl yields one message whose body has requestContext.condition == "RetriesExhausted" and responsePayload.errorMessage containing "c1"; and the permission-less twin stack ends ROLLBACK_COMPLETE/CREATE_FAILED with a reason mentioning SendMessage/permission. Actual: ReceiveMessage returns no messages for the whole poll ("control OnFailure delivery" never satisfied); both checks end "Error: Check timed out after 60.0s".
Cause 2 — expected: an upload under prefix A invokes function A only and an upload under prefix B invokes function B only. Actual: no invocation or side effect is observed for either prefix; "Error: Check timed out after 60.0s".

## Steps to Reproduce
1) Replay lambda__s3__sqs.deploy-lambda-destination-onfailure-to-sqs-positive: deploy the stack (function with an EventInvokeConfig whose OnFailure destination is the stack's SQS queue), Invoke it with InvocationType=Event, then poll sqs ReceiveMessage on the DestQueueUrl output.
2) Replay lambda__s3__sqs.deploy-lambda-destination-missing-permission-negative: deploy the same shape with grant_send=True (control) and then with grant_send=False, polling DescribeStacks for the second one's terminal status and failure reason.
3) Replay lambda__s3__sqs.deploy-s3-batch-notification-multiple-lambda-targets-positive: deploy the stack with two prefix-filtered notification rules pointing at two functions, upload one object under each prefix and observe which function ran.

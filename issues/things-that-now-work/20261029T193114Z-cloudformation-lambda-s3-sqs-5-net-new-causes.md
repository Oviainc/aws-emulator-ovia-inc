Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__s3__sqs.sqs-esm-batch-attributes-preserved cloudformation__lambda__s3__sqs.sqs-esm-lambda-partial-batch-response-positive cloudformation__lambda__s3__sqs.sqs-esm-lambda-trigger-positive cloudformation__lambda__s3__sqs.s3-to-sqs-to-lambda-pipeline-positive cloudformation__lambda__s3__sqs.sqs-to-lambda-to-s3-pipeline-positive cloudformation__lambda__s3__sqs.s3-notification-lambda-missing-resource-policy-negative cloudformation__lambda__s3__sqs.s3-notification-to-lambda-event-format-positive cloudformation__lambda__s3__sqs.s3-notification-to-lambda-positive cloudformation__lambda__s3__sqs.s3-notification-sqs-missing-policy-negative cloudformation__lambda__s3__sqs.sqs-esm-missing-receive-permission-negative cloudformation__lambda__s3__sqs.cfn-invalid-lambda-code-s3-key-negative
Issue-Key: cloudformation__lambda__s3__sqs.sqs-esm-batch-attributes-preserved|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__lambda__s3__sqs.sqs-esm-lambda-partial-batch-response-positive|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__lambda__s3__sqs.sqs-esm-lambda-trigger-positive|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__lambda__s3__sqs.s3-to-sqs-to-lambda-pipeline-positive|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__lambda__s3__sqs.sqs-to-lambda-to-s3-pipeline-positive|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__lambda__s3__sqs.s3-notification-lambda-missing-resource-policy-negative|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__lambda__s3__sqs.s3-notification-to-lambda-event-format-positive|_wait_for_messages#10|Timeout|resp = sqs.receive_message(
Issue-Key: cloudformation__lambda__s3__sqs.s3-notification-to-lambda-positive|_wait_for_messages#10|Timeout|resp = sqs.receive_message(
Issue-Key: cloudformation__lambda__s3__sqs.s3-notification-sqs-missing-policy-negative|run#15|AssertionError|assert err_code == "InvalidArgument", (
Issue-Key: cloudformation__lambda__s3__sqs.sqs-esm-missing-receive-permission-negative|run#13|AssertionError|raise AssertionError(
Issue-Key: cloudformation__lambda__s3__sqs|GetFunction|ResourceNotFoundException
Issue-Commit: 6712e9b9ed86b1379f14acf29b1637f3b8f9994b

# cloudformation__lambda__s3__sqs: 5 net-new causes — SQS ESM and S3->Lambda notifications never invoke, S3/ESM permission validation is missing, and a stack's AWS::Lambda::Function never materialises

> _Note: this issue was identified against the code base at commit `6712e9b9ed86b1379f14acf29b1637f3b8f9994b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Five net-new root causes in this service group. Related open issues that do NOT cover these checks: 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md (direct Invoke), 20261023T004328Z-bedrock-agent-runtime-lambda-s3vectors-cloudformation-stacks-declaring-aws-lambda-function-reach-create.md (CFN-declared functions).

Cause 1 of 5: an SQS event source mapping never produces an invocation.
AWS: recorded behavior only for the poller; the model fixes the surface (lambda CreateEventSourceMapping takes EventSourceArn + FunctionName and returns State; AWS moves it to Enabled and then polls the queue, invoking the function with an SQS Records envelope and deleting successfully processed messages).
Emulator: the mapping is created and reported, but no invocation ever happens and no message is ever consumed, so every check that waits for the function's side effect is killed at 60s.
Why: the single most common serverless topology (queue -> function) is inert while the control plane reports success; batching, batch windows, partial-batch responses and attribute preservation can never be observed.
Fix: the lambda event-source-mapping poller/runtime — it must dequeue from the source, build the Records envelope and invoke the target, then delete or re-expose messages per the handler result.
Traces: cloudformation__lambda__s3__sqs.sqs-esm-batch-attributes-preserved, cloudformation__lambda__s3__sqs.sqs-esm-lambda-partial-batch-response-positive, cloudformation__lambda__s3__sqs.sqs-esm-lambda-trigger-positive, cloudformation__lambda__s3__sqs.s3-to-sqs-to-lambda-pipeline-positive, cloudformation__lambda__s3__sqs.sqs-to-lambda-to-s3-pipeline-positive

Cause 2 of 5: an S3 bucket notification whose destination is a Lambda function never invokes it.
AWS: recorded behavior only; S3 PutBucketNotificationConfiguration with a LambdaFunctionConfiguration causes each matching object event to invoke the function with the S3 event envelope (Records[].eventSource=aws:s3), and refuses delivery when the function has no lambda:InvokeFunction resource policy for s3.amazonaws.com.
Emulator: PutObject produces no invocation at all, so both the positive envelope-format check and the negative missing-resource-policy check hang to the 60s timeout.
Why: S3-triggered processing never runs, and the missing-permission case is indistinguishable from the working case, so an IaC regression that drops the resource policy cannot be detected.
Fix: the S3 notification dispatcher — the Lambda destination branch must invoke the configured function and must gate that invocation on the function's resource policy.
Traces: cloudformation__lambda__s3__sqs.s3-notification-to-lambda-positive, cloudformation__lambda__s3__sqs.s3-notification-to-lambda-event-format-positive, cloudformation__lambda__s3__sqs.s3-notification-lambda-missing-resource-policy-negative

Cause 3 of 5: PutBucketNotificationConfiguration does not validate that the destination queue grants S3 permission to send.
AWS: S3 validates destination permissions at configuration time and refuses with HTTP 400, Error.Code `InvalidArgument`. The message's fixed text is "Unable to validate the following destination configurations"; its variable part is the destination ARN(s) from the request.
Emulator: the call returns success and no error at all (the check reports `Expected InvalidArgument when the destination queue grants S3 no send permission, got None`), and the unusable wiring is persisted.
Why: a misconfigured queue policy is accepted at deploy time and only shows up later as permanently lost events, the exact failure mode the AWS-side validation exists to prevent.
Fix: the S3 PutBucketNotificationConfiguration handler's destination-validation step, before the configuration is stored.
Traces: cloudformation__lambda__s3__sqs.s3-notification-sqs-missing-policy-negative

Cause 4 of 5: CreateEventSourceMapping accepts an execution role that lacks sqs:ReceiveMessage.
AWS: Smithy lambda CreateEventSourceMapping models `InvalidParameterValueException`; AWS refuses the create when the function's execution role cannot read the source queue, with HTTP 400 and Error.Code `InvalidParameterValueException` whose message names the role ARN from the request and the missing permission.
Emulator: the mapping is created successfully (`create_event_source_mapping should have been rejected without sqs:ReceiveMessage`).
Why: the role-permission precondition is never enforced, so a client that relies on create-time refusal to catch a broken role deploys a mapping that can never work.
Fix: the lambda CreateEventSourceMapping handler's validation — resolve the function's execution role and verify it allows the source-queue read actions.
Traces: cloudformation__lambda__s3__sqs.sqs-esm-missing-receive-permission-negative

Cause 5 of 5: a stack-declared AWS::Lambda::Function with a Code.S3Key that does not exist leaves no function under the declared name.
AWS: recorded behavior only. CloudFormation attempts the Lambda create, Lambda refuses the bad S3 key, and the resource reaches CREATE_FAILED with that refusal as its reason; the function the template names is resolvable for the lifetime it exists and the stack rolls back deterministically.
Emulator: lambda GetFunction for the function the stack declares answers HTTP 404 `ResourceNotFoundException` with message `Function not found: arn:aws:lambda:<region>:<account>:function:<declared-name>` (the ARN is the only variable part), and the check cannot proceed. The same "Function not found" signature appears for sibling CFN-declared functions covered by 20261023T004328Z-...-cloudformation-stacks-declaring-aws-lambda-function-reach-create.md, which does not carry this check's replay key.
Why: a negative deployment test cannot distinguish "Lambda refused the bad code location" from "CloudFormation never created the resource", so bad-code-location regressions are invisible.
Fix: the CloudFormation AWS::Lambda::Function resource provider — it must create the function under its declared/derived physical name and record the provider's refusal as the resource's CREATE_FAILED reason.
Traces: cloudformation__lambda__s3__sqs.cfn-invalid-lambda-code-s3-key-negative

## Evidence
Cause 1/2 — `Error: Check timed out after 60.0s` on all eight delivery checks; no invocation and no message consumption is ever observed.
Cause 3 — `AssertionError: Expected InvalidArgument when the destination queue grants S3 no send permission, got None`.
Cause 4 — `AssertionError: create_event_source_mapping should have been rejected without sqs:ReceiveMessage`.
Cause 5 — `botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetFunction operation: Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-xsvc-badkey-fn-badkey-stack55fb04b096eb`.

## Steps to Reproduce
Cause 1: create a queue and a function, CreateEventSourceMapping(EventSourceArn=queue, FunctionName=fn), SendMessage, then wait for any function side effect — none occurs.
Cause 2: PutBucketNotificationConfiguration with a LambdaFunctionConfiguration, PutObject, wait for the invocation — none occurs; repeat with the function's resource policy removed and observe the same silence.
Cause 3: create a queue with NO policy granting s3.amazonaws.com sqs:SendMessage, then PutBucketNotificationConfiguration with that queue as QueueArn — the call succeeds instead of returning InvalidArgument.
Cause 4: create a role without sqs:ReceiveMessage, create a function with it, then CreateEventSourceMapping against an SQS source — the call succeeds.
Cause 5: CreateStack with an AWS::Lambda::Function whose Code.S3Bucket/S3Key does not exist, then GetFunction on the declared function name — 404 ResourceNotFoundException and no CREATE_FAILED reason naming the bad key.

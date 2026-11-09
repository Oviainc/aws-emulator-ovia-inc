Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__sqs.cli-create-bucket-queue-function-wire-notification-e2e-positive lambda__s3__sqs.deploy-lambda-reserved-concurrency-limits-sqs-throughput-positive lambda__s3__sqs.deploy-sqs-dlq-redrive-policy-lambda-failure-positive lambda__s3__sqs.s3-notification-to-sqs-lambda-esm-drains-queue-positive lambda__s3__sqs.sqs-esm-direct-sdk-trigger-positive lambda__s3__sqs.sqs-esm-disabled-mapping-no-invocation-negative lambda__s3__sqs.sqs-esm-dlq-redirect-after-max-receives-positive lambda__s3__sqs.sqs-esm-filter-criteria-drops-nonmatching-negative lambda__s3__sqs.sqs-esm-lambda-error-triggers-retry-positive lambda__s3__sqs.sqs-esm-message-attributes-preserved-positive lambda__s3__sqs.sqs-esm-wrong-queue-arn-mismatch-negative lambda__s3__sqs.cli-put-bucket-notification-missing-permission-negative lambda__s3__sqs.deploy-missing-esm-source-arn-permission-negative lambda__s3__sqs.deploy-s3-batch-notification-multiple-lambda-targets-positive lambda__s3__sqs.esm-event-source-arn-type-and-account-rejected
Issue-Key: lambda__s3__sqs.cli-create-bucket-queue-function-wire-notification-e2e-positive|run#19|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=5, WaitTimeSeconds=10)
Issue-Key: lambda__s3__sqs.deploy-lambda-reserved-concurrency-limits-sqs-throughput-positive|_collect_results#9|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: lambda__s3__sqs.deploy-sqs-dlq-redrive-policy-lambda-failure-positive|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: lambda__s3__sqs.s3-notification-to-sqs-lambda-esm-drains-queue-positive|run#15|Timeout|time.sleep(3)
Issue-Key: lambda__s3__sqs.sqs-esm-direct-sdk-trigger-positive|_poll_reports#10|Timeout|time.sleep(interval_s)
Issue-Key: lambda__s3__sqs.sqs-esm-disabled-mapping-no-invocation-negative|_poll_reports#12|Timeout|time.sleep(interval_s)
Issue-Key: lambda__s3__sqs.sqs-esm-dlq-redirect-after-max-receives-positive|run#22|AssertionError|assert receive_counts, "no lambda invocation report recorded for the marker message"
Issue-Key: lambda__s3__sqs.sqs-esm-filter-criteria-drops-nonmatching-negative|_poll_reports#13|Timeout|time.sleep(interval_s)
Issue-Key: lambda__s3__sqs.sqs-esm-lambda-error-triggers-retry-positive|_poll_reports#12|Timeout|time.sleep(interval_s)
Issue-Key: lambda__s3__sqs.sqs-esm-message-attributes-preserved-positive|_poll_reports#11|Timeout|time.sleep(interval_s)
Issue-Key: lambda__s3__sqs.sqs-esm-wrong-queue-arn-mismatch-negative|_poll_reports#13|Timeout|time.sleep(interval_s)
Issue-Key: lambda__s3__sqs.cli-put-bucket-notification-missing-permission-negative|_cli_error#10|AssertionError|raise AssertionError(f"{operation} was expected to fail but succeeded: {proc.stdout[-500:]!r}")
Issue-Key: lambda__s3__sqs.deploy-missing-esm-source-arn-permission-negative|_expect_error#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: lambda__s3__sqs.deploy-s3-batch-notification-multiple-lambda-targets-positive|_deploy_cfn#5|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: lambda__s3__sqs.esm-event-source-arn-type-and-account-rejected|run#10|AssertionError|assert wrong_type == expected_wrong_type, (
Issue-Commit: 6712e9b9ed86b1379f14acf29b1637f3b8f9994b

# lambda__s3__sqs: 5 net-new causes — SQS event source mappings never invoke, S3/ESM permission validation is missing, a multi-target notification stack rolls back, and an ESM refusal message is rewritten

> _Note: this issue was identified against the code base at commit `6712e9b9ed86b1379f14acf29b1637f3b8f9994b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Five net-new root causes. Related open issues that do NOT carry these replay keys: 20261025T022656Z-lambda-s3-sqs-sqs-event-source-mappings-never.md, 20261029T072224Z-lambda-s3-sqs-an-sqs-event-source-mapping.md, 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md.

Cause 1 of 5: an SQS event source mapping never produces an invocation.
AWS: recorded behavior only for the poller; the model fixes the surface (lambda CreateEventSourceMapping with EventSourceArn, FunctionName, BatchSize, FilterCriteria, Enabled, FunctionResponseTypes; State is driven to Enabled). AWS polls the source queue and invokes the function with an SQS Records envelope, deletes processed messages, re-exposes failed ones until maxReceiveCount sends them to the configured DLQ, and preserves message attributes in the record.
Emulator: nothing is ever invoked and nothing is ever consumed; eleven checks either hang to the 60s harness budget or fail with "no lambda invocation report recorded for the marker message".
Why: the queue -> function topology is completely inert while the control plane reports a healthy mapping, so retries, DLQ redirection, filter criteria, reserved concurrency and attribute preservation cannot be relied on or tested.
Fix: the lambda event-source-mapping poller/runtime.
Traces: lambda__s3__sqs.cli-create-bucket-queue-function-wire-notification-e2e-positive, lambda__s3__sqs.deploy-lambda-reserved-concurrency-limits-sqs-throughput-positive, lambda__s3__sqs.deploy-sqs-dlq-redrive-policy-lambda-failure-positive, lambda__s3__sqs.s3-notification-to-sqs-lambda-esm-drains-queue-positive, lambda__s3__sqs.sqs-esm-direct-sdk-trigger-positive, lambda__s3__sqs.sqs-esm-disabled-mapping-no-invocation-negative, lambda__s3__sqs.sqs-esm-dlq-redirect-after-max-receives-positive, lambda__s3__sqs.sqs-esm-filter-criteria-drops-nonmatching-negative, lambda__s3__sqs.sqs-esm-lambda-error-triggers-retry-positive, lambda__s3__sqs.sqs-esm-message-attributes-preserved-positive, lambda__s3__sqs.sqs-esm-wrong-queue-arn-mismatch-negative

Cause 2 of 5: PutBucketNotificationConfiguration accepts a destination the caller is not permitted to deliver to.
AWS: S3 validates destination permissions at configuration time, refusing with HTTP 400 and Error.Code `InvalidArgument`, message fixed text "Unable to validate the following destination configurations" plus the offending destination ARN from the request.
Emulator: the call succeeds and returns an empty response body.
Why: an unusable notification wiring is accepted and only manifests later as permanently lost events; the negative test that is supposed to catch a missing grant passes vacuously.
Fix: the S3 PutBucketNotificationConfiguration handler's destination-validation step.
Traces: lambda__s3__sqs.cli-put-bucket-notification-missing-permission-negative

Cause 3 of 5: CreateEventSourceMapping accepts a function whose execution role has no permission on the source ARN.
AWS: refuses with HTTP 400 and Error.Code `InvalidParameterValueException` (modelled on lambda CreateEventSourceMapping), the message naming the role ARN from the request and the permission it lacks on the given EventSourceArn.
Emulator: answers HTTP 202 with a full mapping body (State='Creating', a fresh UUID and EventSourceMappingArn).
Why: a mapping that can never poll is created and reported healthy, so the create-time guardrail that catches a broken execution role is gone.
Fix: the lambda CreateEventSourceMapping handler's validation — resolve the function's execution role and check it against the source ARN.
Traces: lambda__s3__sqs.deploy-missing-esm-source-arn-permission-negative

Cause 4 of 5: a stack declaring one bucket notification with multiple Lambda targets ends ROLLBACK_COMPLETE.
AWS: recorded behavior only. A NotificationConfiguration with several LambdaFunctionConfigurations on one bucket is valid and the stack reaches CREATE_COMPLETE with every target wired.
Emulator: `Stack <name> ended in ROLLBACK_COMPLETE` — the multi-target notification resource is refused during provisioning.
Why: a legitimate fan-out template cannot be deployed at all, so the deployment path for the common "one bucket, several processors" pattern is unavailable.
Fix: the CloudFormation provider for the bucket-notification resource (and the S3 PutBucketNotificationConfiguration it calls) — accept multiple Lambda destinations in one configuration.
Traces: lambda__s3__sqs.deploy-s3-batch-notification-multiple-lambda-targets-positive

Cause 5 of 5: the refusal message for an unsupported EventSourceArn is emulator-authored.
AWS: refuses an S3 bucket ARN as an EventSourceArn with Error.Code `InvalidParameterValueException` and the message "Unrecognized event source. Unsupported source arn : <arn>" — the fixed text is "Unrecognized event source. Unsupported source arn : " and the only variable part is the rejected ARN from the request.
Emulator: returns the right code but the message "Unrecognized event source '<arn>'. An event source mapping polls an Amazon Kinesis data stream or stream consumer, an Amazon DynamoDB stream, an Amazon Simple Queue Service queue, an Amazon Managed Streaming for Apache Kafka cluster or VPC connection, an Amazon MQ broker or an Amazon DocumentDB change stream" — a different sentence with a different shape.
Why: clients and IaC tools match this refusal text to distinguish "wrong ARN type" from other InvalidParameterValueException causes; a rewritten sentence breaks that discrimination.
Fix: the lambda CreateEventSourceMapping validation's message template.
Traces: lambda__s3__sqs.esm-event-source-arn-type-and-account-rejected

## Evidence
Cause 1 — `Error: Check timed out after 60.0s` (10 checks) and `AssertionError: no lambda invocation report recorded for the marker message`.
Cause 2 — `AssertionError: PutBucketNotificationConfiguration was expected to fail but succeeded: ''`.
Cause 3 — `AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 202, ...}, 'UUID': '...', 'State': 'Creating', 'EventSourceMappingArn': 'arn:aws:lambda:us-west-2:000000000000:event-source-mapping:...'}`.
Cause 4 — `AssertionError: Stack sim2real-xsvc-stk-775ee288685d ended in ROLLBACK_COMPLETE`.
Cause 5 — expected ('InvalidParameterValueException', 'Unrecognized event source. Unsupported source arn : arn:aws:s3:::<bucket>'); got ('InvalidParameterValueException', "Unrecognized event source 'arn:aws:s3:::<bucket>'. An event source mapping polls an Amazon Kinesis data stream or stream consumer, ...").

## Steps to Reproduce
Cause 1: CreateEventSourceMapping(EventSourceArn=<queue>, FunctionName=<fn>), SendMessage, wait for an invocation — none occurs.
Cause 2: create a function/queue with no resource policy permitting s3.amazonaws.com, then PutBucketNotificationConfiguration naming it — the call succeeds.
Cause 3: create a function whose execution role has no sqs permission on the source queue ARN, then CreateEventSourceMapping — HTTP 202.
Cause 4: CreateStack with one AWS::S3::Bucket NotificationConfiguration containing two LambdaFunctionConfigurations — the stack rolls back.
Cause 5: CreateEventSourceMapping(EventSourceArn='arn:aws:s3:::<bucket>') — the message differs from AWS's.

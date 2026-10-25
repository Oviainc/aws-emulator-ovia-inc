Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3.s3-put-triggers-lambda-via-notification dynamodb__lambda__s3.saga-s3-upload-processing-with-compensation
Issue-Key: dynamodb__lambda__s3.s3-put-triggers-lambda-via-notification|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda__s3.saga-s3-upload-processing-with-compensation|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# dynamodb__lambda__s3: an S3 PutObject never invokes the bucket's Lambda notification target, so both object-driven flows hang to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Cause 1 of 1: a bucket with a LambdaFunctionConfiguration never delivers an s3:ObjectCreated:* event to the function, so neither the simple put-triggers-lambda flow nor the saga with compensation ever observes an effect; both checks are killed at the harness 60 s cap.

AWS: once PutBucketNotificationConfiguration has installed a LambdaFunctionConfiguration (and lambda:InvokeFunction is granted to s3.amazonaws.com), every matching PutObject asynchronously invokes the function with an s3 event record naming the bucket and key; the side effect the function performs becomes observable within the delivery delay. Recorded behavior only for the delay.

Emulator: the notification configuration is accepted but no invocation ever happens — the checks poll for the function's side effect (a DynamoDB item / a compensating write) and never see it, reporting 'Error: Check timed out after 60.0s'.

Why: event-driven designs built on S3 notifications silently do nothing. A client sees a successful PutObject and a configured notification, yet no downstream work, and nothing in any API response says the event was dropped.

Fix: the S3 event-notification dispatcher (the PutObject path that must match configured notification filters and invoke the configured Lambda target) in the s3 service, plus the lambda async invoke path it calls. Related open issue 20261024T190039Z-events-lambda-s3-sfn-s3-event-notifications-never-fire-a-putobject-publishes.md records this for another service group; neither of these two checks is listed there.

Traces: dynamodb__lambda__s3.s3-put-triggers-lambda-via-notification, dynamodb__lambda__s3.saga-s3-upload-processing-with-compensation.

## Evidence
Expected: PutObject to the configured prefix invokes the function once and its DynamoDB write appears.
Actual: Error: Check timed out after 60.0s (both checks) — no invocation and no side effect.

## Steps to Reproduce
1. Create a function and grant s3.amazonaws.com lambda:InvokeFunction with the bucket as SourceArn.
2. PutBucketNotificationConfiguration with a LambdaFunctionConfiguration for s3:ObjectCreated:*.
3. PutObject a matching key and poll for the function's side effect.
4. Observe no invocation.

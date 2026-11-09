Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.sqs-success-feedback-role-writes-the-delivery-record-to-cloudwatch-logs sns.lambda-feedback-roles-log-the-invocation-delivery-record-and-the-uninvokable-function-failure sns.feedback-role-without-logs-permission-still-delivers-and-writes-no-log-record
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.sqs-success-feedback-role-writes-the-delivery-record-to-cloudwatch-logs|run#34|KeyError|assert record["delivery"]["statusCode"] == 200, (
Issue-Key: sns.lambda-feedback-roles-log-the-invocation-delivery-record-and-the-uninvokable-function-failure|run#25|KeyError|assert matching_success[0]["status"] == "SUCCESS" and matching_success[0]["delivery"]["statusCode"] == 202, (
Issue-Key: sns.feedback-role-without-logs-permission-still-delivers-and-writes-no-log-record|run#18|AssertionError|assert stuck == [], (
Issue-Commit: 9ae19e0eca83af3a9e4c3dd8e20d1f4043458c83

# sns: delivery-status log records omit delivery.statusCode/providerResponse, and a feedback role with no logs permissions still writes a record (2 root causes)

> _Note: this issue was identified against the code base at commit `9ae19e0eca83af3a9e4c3dd8e20d1f4043458c83`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Two net-new root causes in SNS delivery-status logging (the `sns/<region>/<account-id>/<topic-name>` log group written when a topic carries a <Protocol>SuccessFeedbackRoleArn / <Protocol>FailureFeedbackRoleArn and a non-zero sample rate). NOTE: three open issues claim this path is wholly inert — 20261104T075451Z-sns-delivery-status-logging-is-entirely-inert-no.md, 20261029T065537Z-sns-delivery-status-logging-is-inert-for-the.md and 20261025T064350Z-lambda-sns-sqs-a-cfn-deployed-sns-deliverystatuslogging.md. They do NOT cover these: records ARE now being written and the group IS created; what diverges is the record's CONTENT and the fact that the role's permissions are never consulted. Neither check below is marked OWNED by any of them.

Cause 1 of 2: the delivery-status record's `delivery` object omits `statusCode` and `providerResponse`.
  AWS: recorded/documented behavior only — the delivery-status record shape is not in the Smithy model (SNS Smithy exposes only the topic attributes `SQSSuccessFeedbackRoleArn`, `SQSSuccessFeedbackSampleRate`, `LambdaSuccessFeedbackRoleArn`, `LambdaFailureFeedbackRoleArn` on CreateTopic/SetTopicAttributes). AWS writes one JSON record per delivery attempt whose `delivery` object carries, besides `deliveryId`/`destination`/`dwellTimeMs`/`attempts`, a `statusCode` that is the endpoint's HTTP status for that protocol (200 for a successful SQS SendMessage, 202 for a successful asynchronous Lambda invoke, 403 for an invoke SNS is not authorized to perform) and a `providerResponse` string carrying the endpoint's response — for a successful Lambda delivery a JSON object with a `lambdaRequestId` member, and for a denied Lambda delivery a JSON object whose `ErrorCode` is `AccessDeniedException` and whose `ErrorMessage` is the fixed sentence `User: sns.amazonaws.com is not authorized to perform: lambda:InvokeFunction on resource: <function-arn> because no resource-based policy allows the lambda:InvokeFunction action` (fixed text, with only `<function-arn>` taken from the subscription's endpoint). The record also carries `notification.messageMD5Sum` alongside `messageId`/`topicArn`/`timestamp`.
  Emulator: the record it writes is exactly `{"notification": {"messageId", "topicArn", "timestamp"}, "delivery": {"deliveryId", "destination", "dwellTimeMs", "attempts"}, "status": "SUCCESS"}` — `delivery.statusCode` and `delivery.providerResponse` are absent entirely (and `notification.messageMD5Sum` with them), for both the sqs and the lambda protocol. Both checks reach the record, find exactly one for their MessageId, confirm `status == "SUCCESS"` and the right `delivery.destination`, then die with `KeyError: 'statusCode'`.
  Why: a client reading delivery-status logs to tell a 200 SQS enqueue from a 403 refusal, or to correlate an invoke through `providerResponse.lambdaRequestId`, cannot: every record looks the same and the per-attempt outcome detail AWS guarantees is simply missing. Failure diagnosis built on `delivery.statusCode` / `providerResponse.ErrorCode` silently reads nothing.
  Fix: the delivery-status record serializer in the SNS delivery/feedback path — it must populate `delivery.statusCode` from the protocol endpoint's actual response status and `delivery.providerResponse` from that endpoint's response body, per protocol, and add `notification.messageMD5Sum`.
  Traces: sns.sqs-success-feedback-role-writes-the-delivery-record-to-cloudwatch-logs (KeyError: 'statusCode' at the `record["delivery"]["statusCode"] == 200` assertion), sns.lambda-feedback-roles-log-the-invocation-delivery-record-and-the-uninvokable-function-failure (KeyError: 'statusCode' at the `statusCode == 202` assertion).

Cause 2 of 2: the feedback role's own IAM permissions are never evaluated — a role that cannot write logs still produces a log record.
  AWS: recorded/documented behavior only. SNS assumes the configured feedback role and writes the record AS that role, so when the role's identity policy does not allow `logs:CreateLogGroup`/`logs:CreateLogStream`/`logs:PutLogEvents`, the write is refused: no `sns/<region>/<account-id>/<topic-name>` log group is created and no record appears. Message delivery itself is unaffected (Publish still succeeds and the subscriber still receives the message) — the logging is best-effort and is dropped. A twin topic whose feedback role DOES carry those logs permissions writes the record, proving the path is live.
  Emulator: the denied topic's log group is created and a full SUCCESS record is written anyway. The check's negative control fails with `AssertionError: a feedback role with no logs permissions must produce no log record; got [{'notification': {...}, 'delivery': {'deliveryId': ..., 'destination': <queue-arn>, 'dwellTimeMs': 10, 'attempts': 1}, 'status': 'SUCCESS'}]` (the ids/timestamps there are per-run values; the defect is that any record exists).
  Why: the emulator writes delivery-status logs unconditionally, so a client cannot reproduce or diagnose the single most common real-world failure of this feature — "my feedback role is missing logs permissions, that is why I see no records". Any test that asserts the absence of records for an under-permissioned role passes against AWS and fails here, and IAM least-privilege work on feedback roles cannot be validated at all.
  Fix: the SNS delivery-status logging path — it must assume the configured feedback role and authorize the logs:CreateLogGroup/CreateLogStream/PutLogEvents write against that role's policies, dropping the record (and not creating the group) when the write is not allowed, while leaving message delivery untouched.
  Traces: sns.feedback-role-without-logs-permission-still-delivers-and-writes-no-log-record.

## Evidence
Check sns.sqs-success-feedback-role-writes-the-delivery-record-to-cloudwatch-logs:
  expected: record["delivery"]["statusCode"] == 200 for a successful SQS delivery
  actual:   File "check_sns_sqs_success_feedback_role_writes_the_delivery_record_to_cloudwatch_logs.py", line 91, in run
            assert record["delivery"]["statusCode"] == 200
            KeyError: 'statusCode'

Check sns.lambda-feedback-roles-log-the-invocation-delivery-record-and-the-uninvokable-function-failure:
  expected: status SUCCESS with delivery.statusCode == 202 and delivery.providerResponse carrying lambdaRequestId; then a FAILURE record with statusCode 403 and providerResponse.ErrorCode == "AccessDeniedException" and ErrorMessage "User: sns.amazonaws.com is not authorized to perform: lambda:InvokeFunction on resource: <denied-fn-arn> because no resource-based policy allows the lambda:InvokeFunction action"
  actual:   File "check_sns_lambda_feedback_roles_log_the_invocation_delivery_record_and_the_uninvokable_function_failure.py", line 62, in run
            assert matching_success[0]["status"] == "SUCCESS" and matching_success[0]["delivery"]["statusCode"] == 202
            KeyError: 'statusCode'
  (the record was found and had the right status/destination — only statusCode/providerResponse are missing)

Check sns.feedback-role-without-logs-permission-still-delivers-and-writes-no-log-record:
  expected: no log group and no record for the topic whose feedback role has no logs permissions ([] records)
  actual:   AssertionError: a feedback role with no logs permissions must produce no log record; got [{'notification': {'messageId': ..., 'topicArn': 'arn:aws:sns:<region>:<account>:<topic>', 'timestamp': ...}, 'delivery': {'deliveryId': ..., 'destination': 'arn:aws:sqs:<region>:<account>:<queue>', 'dwellTimeMs': 10, 'attempts': 1}, 'status': 'SUCCESS'}]
  (note the same record also lacks delivery.statusCode — Cause 1)

## Steps to Reproduce
Cause 1:
1. iam:CreateRole a role trusting {"Service": "sns.amazonaws.com"} and put an inline policy allowing logs:CreateLogGroup/CreateLogStream/PutLogEvents on *.
2. sns:CreateTopic with Attributes {SQSSuccessFeedbackRoleArn: <role-arn>, SQSSuccessFeedbackSampleRate: "100"}.
3. sqs:CreateQueue, set a queue Policy allowing sns.amazonaws.com sqs:SendMessage with ArnEquals aws:SourceArn = <topic>, and sns:Subscribe protocol "sqs" to the queue ARN.
4. sns:Publish, drain the queue to confirm delivery, then logs:FilterLogEvents on log group sns/<region>/<account>/<topic-name>.
5. Parse the single record: delivery.statusCode is absent (AWS: 200); delivery.providerResponse is absent.
6. Repeat with Protocol="lambda" and LambdaSuccessFeedbackRoleArn/SampleRate=100 plus lambda:AddPermission for sns.amazonaws.com: delivery.statusCode is absent (AWS: 202) and providerResponse.lambdaRequestId is absent. Subscribe a second function WITHOUT AddPermission and read sns/<region>/<account>/<topic-name>/Failure: AWS records status FAILURE, statusCode 403 and providerResponse.ErrorCode AccessDeniedException.

Cause 2:
1. Create two identical roles trusting sns.amazonaws.com; give only one an inline policy allowing logs:CreateLogGroup/CreateLogStream/PutLogEvents.
2. Create two topics, each with SQSSuccessFeedbackRoleArn set to one of the roles and SQSSuccessFeedbackSampleRate "100"; subscribe a queue to each.
3. Publish to both.
4. logs:FilterLogEvents on the permitted topic's group: a record appears (positive control). On the DENIED topic's group: AWS has no group and no record; the emulator creates the group and writes a full SUCCESS record.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3.eventbridge-rule-invokes-lambda-target events__lambda__s3.eventbridge-event-payload-format-to-lambda events__lambda__s3.eventbridge-pattern-mismatch-suppressed events__lambda__s3.eventbridge-target-missing-lambda-permission-not-invoked events__lambda__s3.eventbridge-target-qualified-arn-resolution events__lambda__s3.s3-to-eventbridge-to-lambda-pipeline events__lambda__s3.s3-object-created-invokes-lambda events__lambda__s3.s3-event-payload-format-to-lambda events__lambda__s3.s3-notification-suffix-filter events__lambda__s3.s3-notification-missing-permission-rejected events__lambda__s3.cfn-deploy-s3-lambda-notification
Issue-Key: events__lambda__s3.eventbridge-rule-invokes-lambda-target|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__s3.eventbridge-event-payload-format-to-lambda|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__s3.eventbridge-pattern-mismatch-suppressed|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__s3.eventbridge-target-missing-lambda-permission-not-invoked|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__s3.eventbridge-target-qualified-arn-resolution|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__s3.s3-to-eventbridge-to-lambda-pipeline|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__s3.s3-object-created-invokes-lambda|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__s3.s3-event-payload-format-to-lambda|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__s3.s3-notification-suffix-filter|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__s3.s3-notification-missing-permission-rejected|_error#17|AssertionError|raise AssertionError(
Issue-Key: events__lambda__s3.cfn-deploy-s3-lambda-notification|_deploy_cfn#3|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# events/lambda/s3: EventBridge and S3 notifications never invoke Lambda, S3 accepts a notification config it has no invoke permission for, and a CFN S3+Lambda stack rolls back (4 causes)

> _Note: this issue was identified against the code base at commit `b1cd28f1ca277d6a032df605f83af5017e934043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Four root causes. 20261025T002357Z-events-lambda-s3-an-eventbridge-rule-s-lambda-target-is-never.md and 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md own other checks in this service; the checks below were not bound to them.

Cause 1 of 4: an EventBridge rule never invokes its Lambda target, so neither the envelope format nor the pattern-mismatch suppression can be observed.
AWS: a matching PutEvents invokes each Lambda target asynchronously once with the full envelope as the event: version, id, detail-type, source, account, time, region, resources, detail. A qualified function ARN (with :alias or :version) resolves to that qualifier. A non-matching source is suppressed by the rule, so the handler is never entered.
Emulator: the function is never invoked for any pattern; every check ends "Check timed out after 60.0s" and the negative legs are ungraded.
Why: the EventBridge→Lambda pattern is unusable and a suppressed event looks identical to a broken bus.
Fix: the EventBridge Lambda-target invoker, including qualified-ARN resolution and the envelope it passes as the payload.
Traces: events__lambda__s3.eventbridge-rule-invokes-lambda-target, events__lambda__s3.eventbridge-event-payload-format-to-lambda, events__lambda__s3.eventbridge-pattern-mismatch-suppressed, events__lambda__s3.eventbridge-target-missing-lambda-permission-not-invoked, events__lambda__s3.eventbridge-target-qualified-arn-resolution, events__lambda__s3.s3-to-eventbridge-to-lambda-pipeline

Cause 2 of 4: an S3 bucket notification configuration with a Lambda destination never invokes the function.
AWS: PutObject into a bucket with a LambdaFunctionConfiguration for s3:ObjectCreated:* invokes the function once with the S3 event payload — Records[0] carrying eventSource "aws:s3", eventName, s3.bucket.name, s3.object.key and s3.object.size — honouring any Filter prefix/suffix rules.
Emulator: the function is never invoked; every check ends "Check timed out after 60.0s", so the payload shape and the suffix filter cannot be validated.
Why: the most common S3 trigger pattern does nothing.
Fix: the S3 bucket-notification dispatcher's Lambda destination (payload construction and Filter rule evaluation).
Traces: events__lambda__s3.s3-object-created-invokes-lambda, events__lambda__s3.s3-event-payload-format-to-lambda, events__lambda__s3.s3-notification-suffix-filter

Cause 3 of 4: PutBucketNotificationConfiguration accepts a Lambda destination the bucket has no permission to invoke.
AWS: S3 validates the destination at configure time by test-invoking it; if the function's resource policy has no lambda:InvokeFunction statement for s3.amazonaws.com with the bucket as SourceArn, the call is refused with HTTP 400 and error code InvalidArgument, message of the fixed form "Unable to validate the following destination configurations" (the destination ARN in the detail comes from the request).
Emulator: the call succeeds. "AssertionError: the call was expected to be refused but it SUCCEEDED — S3 accepted a notification config pointing at a Lambda function it has no invoke permission for".
Why: the misconfiguration is accepted and then silently never fires in AWS; the caller gets no signal at the only point where AWS gives one.
Fix: destination validation in the S3 PutBucketNotificationConfiguration handler.
Traces: events__lambda__s3.s3-notification-missing-permission-rejected

Cause 4 of 4: a CloudFormation stack declaring an S3 bucket with a Lambda notification rolls back.
AWS: the stack reaches CREATE_COMPLETE with the bucket, the function, the AWS::Lambda::Permission and the NotificationConfiguration all created.
Emulator: the stack ends ROLLBACK_COMPLETE. "AssertionError: Stack <name> ended in ROLLBACK_COMPLETE".
Why: the IaC form of the S3→Lambda trigger cannot be deployed at all, and the bare rollback does not name the failing resource.
Fix: the CloudFormation providers for AWS::S3::Bucket NotificationConfiguration and AWS::Lambda::Permission, plus the stack-event surface that should identify the failing resource.
Traces: events__lambda__s3.cfn-deploy-s3-lambda-notification

## Evidence
Causes 1 and 2: 9 checks end "Error: Check timed out after 60.0s" with the Lambda never invoked after a matching PutEvents or a PutObject.
Cause 3: "AssertionError: the call was expected to be refused but it SUCCEEDED — S3 accepted a notification config pointing at a Lambda function it has no invoke permission for" — expected HTTP 400 InvalidArgument naming the unvalidatable destination.
Cause 4: "AssertionError: Stack sim2real-xsvc-stk-493ff4118f77 ended in ROLLBACK_COMPLETE".

## Steps to Reproduce
(1) PutRule + PutTargets with a Lambda target (plain and :alias-qualified ARN), AddPermission, PutEvents — the function never runs. (2) PutBucketNotificationConfiguration with a LambdaFunctionConfiguration for s3:ObjectCreated:* (with and without a suffix Filter) and a permissive AddPermission, then PutObject — the function never runs. (3) PutBucketNotificationConfiguration with a Lambda destination and no AddPermission — accepted instead of refused. (4) CreateStack with AWS::S3::Bucket + NotificationConfiguration + AWS::Lambda::Function + AWS::Lambda::Permission — ROLLBACK_COMPLETE.

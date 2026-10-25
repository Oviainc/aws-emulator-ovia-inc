Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3.health-ev-a-matching-event-reaches-the-lambda-and-lands-in-s3 events__lambda__s3.health-ev-a-non-matching-source-is-suppressed-by-the-rule-not-the-handler events__lambda__s3.health-ev-aws-cli-created-rule-delivers-through-the-sdk-wired-target
Issue-Key: events__lambda__s3.health-ev-a-matching-event-reaches-the-lambda-and-lands-in-s3|_poll_keys#7|Timeout|time.sleep(3.0)
Issue-Key: events__lambda__s3.health-ev-a-non-matching-source-is-suppressed-by-the-rule-not-the-handler|_poll_keys#7|Timeout|time.sleep(3.0)
Issue-Key: events__lambda__s3.health-ev-aws-cli-created-rule-delivers-through-the-sdk-wired-target|_poll_keys#22|Timeout|time.sleep(3.0)
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# An EventBridge rule's Lambda target is never invoked, so the AWS Health event pipeline produces no S3 object (1 cause)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Cause 1 of 1 — a matched EventBridge rule does not invoke its Lambda target, so the function's S3 write never happens.
AWS: a rule on the default bus whose target is `arn:aws:lambda:...:function:<fn>`, with a lambda:InvokeFunction resource policy for events.amazonaws.com and SourceArn the rule ARN, invokes the function asynchronously with the event envelope as the payload. A matching event therefore produces the function's side effect (here, an object in S3); a non-matching source must be suppressed by the RULE, so the function is not invoked at all and no object appears; and a rule created with the AWS CLI behaves identically to one created through the SDK.
Emulator: the function is never run. Each check first succeeds at CreateFunction, AddPermission, PutRule, PutTargets (FailedEntryCount 0) and PutEvents (FailedEntryCount 0, an EventId returned), then polls S3 for the object the function would write; the object never appears and the replay cuts each check off at 60 s. Because the positive control never fires, the negative check (a non-matching source must be suppressed by the rule rather than by the handler) cannot be graded either — an emulator that delivers nothing passes a suppression test for the wrong reason.
Why: the EventBridge -> Lambda -> S3 shape is the standard event-driven integration. Every control-plane call reports success, so a client sees a healthy rule, a healthy function and a permission in place, and silently gets no processing at all. It also cannot tell rule-level filtering from total non-delivery.
Fix: the Lambda target of the EventBridge target dispatcher (services/service-com-amazonaws-eventbridge-awsevents) and the asynchronous invocation path it calls. The already-open 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md covers the synchronous Invoke API answering NotImplemented; it does not carry these checks' replay keys and does not cover the EventBridge-side dispatch, which here never reaches Invoke at all.
Traces: events__lambda__s3.health-ev-a-matching-event-reaches-the-lambda-and-lands-in-s3, events__lambda__s3.health-ev-a-non-matching-source-is-suppressed-by-the-rule-not-the-handler, events__lambda__s3.health-ev-aws-cli-created-rule-delivers-through-the-sdk-wired-target.

## Evidence
All three checks report "Check timed out after 60.0s". Each had already completed, with 2xx responses, the full wiring: CreateBucket, CreateFunction, lambda AddPermission(Principal='events.amazonaws.com', SourceArn=<rule arn>), PutRule, PutTargets (FailedEntryCount 0) and PutEvents (FailedEntryCount 0 plus an EventId). The terminal read — ListObjectsV2/GetObject for the key the function writes — never returns the object. Expected: the object present within the check's delivery budget for the matching event, and absent for the non-matching source *after* the positive control has been established.

## Steps to Reproduce
1. CreateBucket(b); CreateFunction(fn) whose handler writes the received event to b.
2. lambda AddPermission(FunctionName=fn, Principal='events.amazonaws.com', Action='lambda:InvokeFunction', SourceArn=<rule arn>).
3. PutRule(Name=r, EventPattern='{"source":["aws.health"]}'); PutTargets(Rule=r, Targets=[{Id:'t1', Arn:<fn arn>}]).
4. PutEvents a matching event -> poll b for the object: it never appears.
5. Repeat with the rule created via `aws events put-rule` on the CLI -> same result.

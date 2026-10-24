Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__events.eventbridge-cloudfront-putevents-custom-detail-invokes-lambda cloudfront__events.eventbridge-cloudfront-tag-change-event-routes-to-sns cloudfront__events.eventbridge-rule-invokes-lambda-to-create-cloudfront-invalidation
Issue-Key: cloudfront__events.eventbridge-cloudfront-putevents-custom-detail-invokes-lambda|_wait_for_matching_message#23|Timeout|resp = sqs.receive_message(
Issue-Key: cloudfront__events.eventbridge-cloudfront-tag-change-event-routes-to-sns|_wait_for_sns_notification#20|Timeout|resp = sqs.receive_message(
Issue-Key: cloudfront__events.eventbridge-rule-invokes-lambda-to-create-cloudfront-invalidation|_find_matching_invalidation#18|Timeout|time.sleep(interval_s)
Issue-Commit: 0de7718ade8f368be0e47aca52d0dd18a585e082

# cloudfront__events: a matched EventBridge rule never produces its target effect, so all three CloudFront eventing checks hang to the 60 s cap

> _Note: this issue was identified against the code base at commit `0de7718ade8f368be0e47aca52d0dd18a585e082`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
ONE root cause, covering all three failing checks of this service.

Cause 1 of 1: an event that matches a rule never reaches the rule's target, so the target's effect never happens.
AWS: recorded behavior only (EventBridge's model describes the API, not delivery). A PutEvents call whose detail-type matches a rule invokes the rule's Lambda target, and that Lambda's own cloudfront:CreateInvalidation becomes visible through ListInvalidations/GetInvalidation with the exact paths carried in the event detail; a CloudTrail management event for a CloudFront TagResource, matched by a rule, is delivered to an SNS topic target and observed through an SQS subscription; an S3 PutObject event matched by a rule invokes a Lambda that creates an invalidation. A non-matching detail-type invokes nothing.
Emulator: nothing arrives at any target. Each check polls for the target's effect (an invalidation with the event's paths via `_find_matching_invalidation`, or an SQS message) and is killed by the suite's 60 s cap. Two contributing reasons are visible elsewhere in this same run: matched events are not delivered to targets at all, and a Lambda target cannot run even if it were invoked (Invoke answers 501 NotImplemented: "running its python3.12 deployment package is not implemented in this emulator").
Why: every event-driven CloudFront workflow (cache purge on object change, tag-change notification, scheduled/event-triggered invalidation) is unreachable, and the check cannot distinguish "no match" from "matched but undelivered" — both look like silence.
Fix: EventBridge rule matching -> target delivery in the eventbridge/cloudwatchevents service (the PutEvents dispatch path and the target invokers for Lambda and SNS), plus the Lambda execution path it depends on. The open issues 20261023T140640Z-cloudfront-events-eventbridge-never-delivers-a-matched-event-to-any.md and 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md describe the same two components but do not carry these three checks' replay keys — close them together.
Traces: cloudfront__events.eventbridge-cloudfront-putevents-custom-detail-invokes-lambda, cloudfront__events.eventbridge-cloudfront-tag-change-event-routes-to-sns, cloudfront__events.eventbridge-rule-invokes-lambda-to-create-cloudfront-invalidation.

## Evidence
Replay output, verbatim tails:
- cloudfront__events.eventbridge-cloudfront-putevents-custom-detail-invokes-lambda: Error: Check timed out after 60.0s. Check doc: "A custom PutEvents call with detail-type 'CloudFront Cache Purge Requested' matched by an EventBridge rule invokes a Lambda that performs a real cloudfront:CreateInvalidation with the exact paths carried in the event detail; a non-matching detail-type never invokes it."
- cloudfront__events.eventbridge-cloudfront-tag-change-event-routes-to-sns: Error: Check timed out after 60.0s. Check doc: "A CloudTrail TagResource event for a CloudFront distribution, matched by an EventBridge rule, is delivered to an SNS topic target (observed via an SQS subscription)."
- cloudfront__events.eventbridge-rule-invokes-lambda-to-create-cloudfront-invalidation: Error: Check timed out after 60.0s. Check doc: "S3 PutObject -> EventBridge rule -> Lambda -> cloudfront.create_invalidation, visible via GetInvalidation"; it polls `_find_matching_invalidation(cf, dist_id, path, timeout_s=90)` which lists invalidations until one carries the path.
Corroborating evidence from the same run that a Lambda target could not run even if delivered: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the Invoke operation: ... running its python3.12 deployment package is not implemented in this emulator.

## Steps to Reproduce
1. Create a Lambda (or SNS topic + SQS subscription) and an EventBridge rule whose pattern matches detail-type 'CloudFront Cache Purge Requested' (or the CloudTrail TagResource event for a CloudFront distribution), with that target.
2. PutEvents with a matching event (or tag a distribution / put an object into an EventBridge-enabled bucket).
3. Poll for the target effect — the invalidation with the event's paths, or the SQS message. Nothing ever appears; the check is killed at the 60 s cap.
Or replay the three check ids in trace_ids.

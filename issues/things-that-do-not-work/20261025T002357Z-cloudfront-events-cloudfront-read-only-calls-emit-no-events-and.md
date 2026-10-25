Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__events.xsvc-cloudfront-1-read-only-calls-not-delivered-to-enabled-rule cloudfront__events.xsvc-cloudfront-1-s3-object-created-lambda-invalidation-refreshes-edge
Issue-Key: cloudfront__events.xsvc-cloudfront-1-read-only-calls-not-delivered-to-enabled-rule|_await_sinks_live#16|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Key: cloudfront__events.xsvc-cloudfront-1-s3-object-created-lambda-invalidation-refreshes-edge|_await_sinks_live#43|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# CloudFront read-only calls emit no events and an S3-triggered invalidation pipeline never runs, so neither the CloudFront negative nor the edge-refresh positive can be observed (2 causes)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInvalidation

## Description
Cause 1 of 2 — CloudFront read-only API calls must produce NO event on an enabled rule, but the positive control that makes that assertion meaningful never fires.
AWS: CloudFront publishes management events (via CloudTrail) for mutating calls; read-only calls such as GetDistribution/ListDistributions produce no event, so a rule enabled and matching `aws.cloudfront` receives nothing from them — while a mutating call on the same distribution does reach the rule. The check establishes that positive leg first and then asserts the absence.
Emulator: nothing reaches the rule on either leg; the terminal read never succeeds and the replay cuts the check off at 60 s. An emulator that delivers nothing at all "passes" the absence assertion for the wrong reason, which is why the check gates on the positive leg and fails here.
Why: a client cannot rely on an EventBridge rule over CloudFront activity for audit or automation — no event ever arrives — and cannot distinguish correct suppression of read-only calls from total non-delivery.
Fix: the CloudFront management-event publisher (the code that must emit `aws.cloudfront` events for mutating calls and not for read-only ones) in services/service-com-amazonaws-cloudfront-cloudfront2020-05-31, and the EventBridge default-bus ingestion of them.
Traces: cloudfront__events.xsvc-cloudfront-1-read-only-calls-not-delivered-to-enabled-rule.

Cause 2 of 2 — the S3-object-created -> Lambda -> CreateInvalidation -> refreshed edge chain produces no terminal effect.
AWS: an `aws.s3` "Object Created" event matched by a rule invokes a Lambda target, which calls CloudFront `CreateInvalidation` for the object's path; once the invalidation completes the distribution serves the new object bytes from the edge.
Emulator: the chain produces nothing — no invalidation appears and the edge read never changes — and the check is cut off at 60 s.
Why: cache invalidation on write is the standard CloudFront+S3 deployment pattern; a client gets stale content indefinitely with every call in the chain reporting success.
Fix: the links of that chain in this emulator — the S3 EventBridge notification publisher, the EventBridge Lambda target dispatch, and the CloudFront CreateInvalidation handler and the distribution read it must affect.
Traces: cloudfront__events.xsvc-cloudfront-1-s3-object-created-lambda-invalidation-refreshes-edge.

Note: two open issues already cover CloudFront eventing from the other direction — 20261024T182555Z-cloudfront-events-a-matched-eventbridge-rule-never-produces.md and 20261023T140640Z-cloudfront-events-eventbridge-never-delivers-a-matched-event-to-any.md — but neither carries these two checks' replay keys, and neither covers the read-only-suppression negative or the S3-triggered invalidation chain.

## Evidence
Both checks report "Check timed out after 60.0s". Setup calls returned 2xx beforehand (CreateDistribution, CreateQueue/CreateFunction, PutRule, PutTargets with FailedEntryCount 0, and the CloudFront or S3 call under test). The terminal reads — a message on the enabled rule's queue for the mutating CloudFront call, and the invalidation plus the refreshed edge response — never appear. Expected: the mutating call's `aws.cloudfront` event delivered to the enabled rule and the read-only calls producing none; and a completed CloudFront invalidation followed by the edge serving the new object.

## Steps to Reproduce
Cause 1:
1. CreateDistribution(d); CreateQueue(q); SetQueueAttributes(q, Policy admitting the rule ARN).
2. PutRule(Name=r, EventPattern='{"source":["aws.cloudfront"]}'); PutTargets(Rule=r, Targets=[{Id:'t1', Arn:<q arn>}]).
3. Issue a MUTATING CloudFront call on d (e.g. UpdateDistribution/TagResource) -> q never receives anything, so the read-only absence assertion has no positive control.

Cause 2:
1. CreateBucket(b) with EventBridgeConfiguration; CreateDistribution(d) with b as origin; CreateFunction(fn) that calls cloudfront CreateInvalidation for the event's object key; PutRule+PutTargets pointing the `aws.s3` "Object Created" rule at fn.
2. PutObject(b, k) with new bytes -> no invalidation is created and the distribution never serves the new bytes.

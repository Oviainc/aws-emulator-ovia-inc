Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__events.eventbridge-cron-rule-triggers-cloudfront-invalidation-daily cloudfront__events.eventbridge-scheduled-rule-invokes-lambda-for-cloudfront-invalidation cloudfront__events.eventbridge-scheduled-rule-triggers-cloudfront-invalidation-lambda cloudfront__events.xsvc-cloudfront-1-s3-object-created-lambda-invalidation-refreshes-edge
Issue-Key: cloudfront__events.eventbridge-cron-rule-triggers-cloudfront-invalidation-daily|run#16|AssertionError|assert now < deadline, (
Issue-Key: cloudfront__events.eventbridge-scheduled-rule-invokes-lambda-for-cloudfront-invalidation|_poll_until#13|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: cloudfront__events.eventbridge-scheduled-rule-triggers-cloudfront-invalidation-lambda|_poll_until#17|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: cloudfront__events.xsvc-cloudfront-1-s3-object-created-lambda-invalidation-refreshes-edge|run#45|PublishedUrlRejected|url, route_headers = ctx.endpoint_route(f"{domain}/page.txt")
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# CloudFront: 2 net-new causes — EventBridge cron/rate schedules never fire the CloudFront-invalidation Lambda, and a distribution's DomainName is published as *.cloudfront.net which is not reachable on the emulator

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInvalidation

## Description
Two independent defects grounded in four failing checks under the `cloudfront__events` heading.

Cause 1 of 3... (see numbering below). Cause 1 of 2: schedule-expression rules never fire.
AWS: an `AWS::Events::Rule`/PutRule whose ScheduleExpression is `rate(1 minute)` or `cron(<m> <h> * * ? *)` fires at every nominal instant while State=ENABLED and invokes its target; the target Lambda here calls `cloudfront:CreateInvalidation`, so `ListInvalidations` on the distribution shows an invalidation shortly after each instant.
Emulator: no invalidation ever appears. The cron rule never triggered by its deadline, the `rate(1 minute)` rule never triggered within 210s, and the generic scheduled rule never produced an entry in `ListInvalidations` within 180s. GetRule reports the rule ENABLED, so the rule exists and the EMISSION is what is missing.
Why: every scheduled workflow (cache refreshes, periodic jobs) silently never runs; nothing fails, nothing is logged, and a client cannot distinguish 'not yet' from 'never'.
Fix: the EventBridge scheduler that evaluates ScheduleExpression (rate() and cron()) and dispatches to the rule's targets.
Traces: cloudfront__events.eventbridge-cron-rule-triggers-cloudfront-invalidation-daily; cloudfront__events.eventbridge-scheduled-rule-invokes-lambda-for-cloudfront-invalidation; cloudfront__events.eventbridge-scheduled-rule-triggers-cloudfront-invalidation-lambda.

Cause 2 of 2: the distribution's published DomainName is an address the emulator does not serve.
AWS: `CreateDistribution`/`GetDistribution` return a `DomainName` that a client can fetch over HTTP(S) to read the object through the edge, and after an invalidation the edge serves the new object.
Emulator: it publishes `DomainName` as `<id>.cloudfront.net` and the check's fetch is refused before it leaves the runner: `simparity.published.PublishedUrlRejected: endpoint_url was given https://<id>.cloudfront.net/page.txt (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`. The emulator announced an address other than the one it is reached on.
Why: no client can ever fetch content through an emulated distribution, so edge behavior (cache hit/miss, invalidation taking effect) is untestable and any application that follows DomainName leaves the emulator entirely.
Fix: the CloudFront distribution serializer / state that mints DomainName — it must publish a host the emulator serves (its own host:port, or a per-resource listener) rather than the real AWS edge suffix.
Traces: cloudfront__events.xsvc-cloudfront-1-s3-object-created-lambda-invalidation-refreshes-edge.

Note: open issues 20261107T192835Z-cloudfront-events-3-net-new-causes-a-matched.md and 20261101T214154Z-cloudfront-events-two-more-root-causes-a-matched.md carry OTHER cloudfront__events checks (pattern-matched rules not invoking targets); neither carries the four replay keys above, and neither is about schedule expressions or the published DomainName.

## Evidence
Cause 1: `AssertionError: cron rule cron(55 15 * * ? *) never triggered a CloudFront invalidation by 2026-11-09T15:57:30+00:00 (target was 2026-11-09T15:55:00+00:00)`; `AssertionError: rate(1 minute) schedule triggering a CloudFront invalidation with marker '...' not met within 210.0s. Last result: None`; `AssertionError: scheduled-rule-triggered invalidation appearing in list_invalidations not met within 180.0s. Last result: False`.
Cause 2: `simparity.published.PublishedUrlRejected: endpoint_url was given https://dgzni4pnd7h2s3.cloudfront.net/page.txt (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:36035/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`. (Host/port values are per-run; the requirement is that the published host be one the emulator serves.)

## Steps to Reproduce
1. Cause 1: PutRule with `ScheduleExpression='rate(1 minute)'` (and separately a `cron(...)` expression) and State=ENABLED, PutTargets a Lambda that calls `cloudfront.create_invalidation`, then poll `cloudfront.list_invalidations(DistributionId=...)` past several nominal instants — none appear.
2. Cause 2: `cloudfront.create_distribution(...)`, read `Distribution.DomainName`, and attempt an HTTP GET of `https://<DomainName>/<key>`.
3. Replay the four checks in trace_ids.

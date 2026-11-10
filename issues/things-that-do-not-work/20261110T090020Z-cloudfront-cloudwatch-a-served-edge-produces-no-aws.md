Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-match-recorded-edge-outcomes cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-withheld-without-monitoring-subscription
Issue-Key: cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-match-recorded-edge-outcomes|_poll_until#16|AssertionError|raise AssertionError(f"timed out waiting for {description} after {timeout_s}s (last: {last!r})")
Issue-Key: cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-withheld-without-monitoring-subscription|_poll_until#39|AssertionError|raise AssertionError(f"timed out waiting for {description} after {timeout_s}s (last: {last!r})")
Issue-Commit: c7d11968b57edd6fc914922856e7958da62d91a7

# cloudfront__cloudwatch: a served edge produces no AWS/CloudFront additional metrics — CacheHitRate/OriginLatency/403ErrorRate never appear after CreateMonitoringSubscription

> _Note: this issue was identified against the code base at commit `c7d11968b57edd6fc914922856e7958da62d91a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMonitoringSubscription

## Description
Cause 1 of 1: with a monitoring subscription Enabled on a Deployed distribution and real viewer traffic driven through the edge, the emulator never publishes any subscription-gated AWS/CloudFront metric series, so both the positive and the negative side of the additional-metrics contract are unobservable. Failing checks: cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-match-recorded-edge-outcomes, cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-withheld-without-monitoring-subscription.

AWS: Smithy — CreateMonitoringSubscription is POST /2020-05-31/distributions/{DistributionId}/monitoring-subscription, input MonitoringSubscription (httpPayload, required) whose RealtimeMetricsSubscriptionConfig.RealtimeMetricsSubscriptionStatus is a required enum of {Enabled, Disabled}; it answers 200 with the stored MonitoringSubscription, and models AccessDenied (403), MonitoringSubscriptionAlreadyExists (409), NoSuchDistribution (404), UnsupportedOperation (400). The model says nothing about metric emission, so for the metric contract this is recorded behavior only: once the subscription is Enabled, the additional per-distribution metrics (CacheHitRate, OriginLatency and the per-status error rates such as 403ErrorRate) are published into the AWS/CloudFront namespace in us-east-1, dimensioned exactly on DistributionId=<the distribution's id> and Region=Global, one datapoint per one-minute bucket in which the edge answered traffic; without a subscription those same series are withheld while the always-on series (Requests, 4xxErrorRate, …) continue. The second check's whole claim is that difference: subject distribution with a subscription has the gated series, the control distribution without one does not.

Emulator: the edge is now reachable and the warm-up requests are issued, but ListMetrics/GetMetricStatistics over AWS/CloudFront return nothing at all for the distribution: both checks fail at the same first gate, AssertionError "timed out waiting for the monitoring subscription to go live (a first CacheHitRate point for warm-up traffic) after 900.0s (last: [])" — the last probe result is an empty list after 900 s of 30 s polling. Nothing is emitted for the subject, so the control's (correct) emptiness is vacuous.

Why: a client that enables CloudFront additional metrics and then builds dashboards, metric-math expressions or alarms on CacheHitRate/OriginLatency/<status>ErrorRate sees permanently empty series, and cannot distinguish "subscription not in effect" from "no traffic" from "edge healthy". Worse, the subscription-gating contract reads as satisfied because both sides are empty, so a configuration regression that drops the subscription is undetectable.

Fix: the CloudFront edge request path must record each viewer-request outcome into the CloudWatch metric store under namespace AWS/CloudFront with dimensions DistributionId and Region=Global in us-east-1, and the distribution's monitoring-subscription state (set by CreateMonitoringSubscription, read by GetMonitoringSubscription, cleared by DeleteMonitoringSubscription) must gate exactly the additional series while the always-on series are published regardless. This belongs in the edge handler's metric emission plus the monitoring-subscription state it consults, not in CloudWatch's own storage.

Traces: cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-match-recorded-edge-outcomes, cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-withheld-without-monitoring-subscription. The open issue 20261109T181319Z-cloudfront-cloudwatch-the-distribution-s-published-domainname-is.md lists the same two check ids but is bound to a different failure (PublishedUrlRejected on the published <id>.cloudfront.net DomainName); the replay no longer reaches that refusal, so it does not own this cause. 20261101T214154Z-cloudwatch-cloudfront-sns-sqs-a-deployed-distribution-serves.md describes a related serving symptom under another heading and does not own these checks either.

## Evidence
Expected: after CreateMonitoringSubscription with RealtimeMetricsSubscriptionConfig.RealtimeMetricsSubscriptionStatus=Enabled and warm-up viewer traffic through the edge, ListMetrics/GetMetricStatistics in us-east-1 return at least one CacheHitRate datapoint in namespace AWS/CloudFront with dimensions DistributionId=<the created distribution's id> and Region=Global (and likewise OriginLatency and 403ErrorRate), while an identical distribution with no subscription returns none of those three.
Actual: both checks time out at the first metric gate: AssertionError: timed out waiting for the monitoring subscription to go live (a first CacheHitRate point for warm-up traffic) after 900.0s (last: []). The fixed text is the check's own; the variable parts are the metric name probed (CacheHitRate) and the deadline. No AWS/CloudFront series exists for the distribution at all, subject or control.

## Steps to Reproduce
1. CreateDistribution over an S3 origin using the managed CachingOptimized cache policy and wait for Status=Deployed; create a second, identical distribution as a control.
2. Call CreateMonitoringSubscription on the first distribution with MonitoringSubscription={RealtimeMetricsSubscriptionConfig:{RealtimeMetricsSubscriptionStatus:'Enabled'}} and leave the control without one.
3. Send warm-up and measured viewer requests (present key repeatedly, plus requests for a never-written key) to each distribution's DomainName.
4. In us-east-1, poll ListMetrics / GetMetricStatistics for Namespace=AWS/CloudFront, MetricName=CacheHitRate, Dimensions=[DistributionId=<id>, Region=Global]. The subject returns an empty list indefinitely (checked for 900 s at 30 s intervals); so does the control, so the gating difference is unobservable.

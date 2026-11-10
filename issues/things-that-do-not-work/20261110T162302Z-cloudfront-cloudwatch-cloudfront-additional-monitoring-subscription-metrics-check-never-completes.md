Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-withheld-without-monitoring-subscription
Issue-Key: cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-withheld-without-monitoring-subscription|_poll_until#39|Timeout|time.sleep(interval_s)
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# CloudFront additional (monitoring-subscription) metrics check never completes: no edge traffic is served and no AWS/CloudFront series is published

> _Note: this issue was identified against the code base at commit `f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: the CloudFront edge serves nothing and publishes nothing, so the opt-in boundary of additional metrics cannot be observed.
AWS: a distribution always publishes its default metric set (Requests, BytesDownloaded, TotalErrorRate, 4xxErrorRate, 5xxErrorRate) in AWS/CloudFront under {DistributionId, Region=Global}. The 'additional' metrics (CacheHitRate, OriginLatency, and the per-status-code error-rate series) exist ONLY while a monitoring subscription with RealtimeMetricsSubscriptionStatus=Enabled is in place: without it those series must be absent, with it they must appear. Metric emission is not modelled — documented/recorded behavior only; the check even carries a DATAPLANE_UNIMPLEMENTED marker so that an edge that serves nothing reads as a fidelity failure rather than an environment error.
Emulator: the check requests objects through the distribution's domain and then polls for the metric series until its own 600s budget expires; no series appears in either arm, so the withheld-versus-published distinction is unobservable. The run reports only the expired budget.
Why: a client cannot distinguish 'additional metrics not subscribed' from 'CloudFront publishes nothing', and every dashboard or alarm over an emulated distribution reads empty.
Fix: the cloudfront edge/data-plane path (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31) must serve distribution requests and publish the default AWS/CloudFront series under {DistributionId, Region=Global}, adding the additional series only while a monitoring subscription is Enabled, into the cloudwatch metric store (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801).
Why this is not covered by an existing issue: 20261024T182555Z-cloudfront-cloudwatch-no-edge-traffic-is-ever-served.md owns the sibling check of this heading but not this replay key.
Traces: cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-withheld-without-monitoring-subscription.

## Evidence
Expected: with no monitoring subscription the additional series (CacheHitRate/OriginLatency/per-code error rates) are absent while the default series (Requests, BytesDownloaded, TotalErrorRate) carry datapoints for the requests served; actual: 'Error: Check timed out after 600.0s' — no AWS/CloudFront datapoint of any kind was observed within the check's budget.

## Steps to Reproduce
1. CreateDistribution over an origin the emulator serves and wait for Deployed.
2. Without creating a monitoring subscription, issue a set of requests (hits and misses, plus one that the origin answers 4xx) to the distribution domain.
3. Poll cloudwatch GetMetricStatistics for Namespace AWS/CloudFront, Dimensions=[{DistributionId},{Region=Global}] for Requests and for CacheHitRate -> neither the default nor the additional series ever returns a datapoint.

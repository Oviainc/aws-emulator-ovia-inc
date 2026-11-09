Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-match-recorded-edge-outcomes cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-withheld-without-monitoring-subscription
Issue-Key: cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-match-recorded-edge-outcomes|run#16|PublishedUrlRejected|base, route = ctx.endpoint_route(dist["DomainName"])
Issue-Key: cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-withheld-without-monitoring-subscription|run#33|PublishedUrlRejected|routes = {side: ctx.endpoint_route(d["DomainName"]) for side, d in dists.items()}
Issue-Commit: 17c4643c966c9683f16ddaee3534ae6f6497ef31

# cloudfront__cloudwatch: the distribution's published DomainName is an unserved <id>.cloudfront.net host, so no AWS/CloudFront additional metric is ever produced

> _Note: this issue was identified against the code base at commit `17c4643c966c9683f16ddaee3534ae6f6497ef31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1: the edge address CloudFront publishes is not one the emulator serves, so no viewer traffic can be generated and the additional-metrics contract is unreachable.

AWS: CreateDistribution returns Distribution.DomainName, and viewer requests to https://<DomainName>/<path> are answered by the CloudFront edge. Those requests are what produce the AWS/CloudFront metrics a monitoring subscription enables (CacheHitRate, OriginLatency, the per-status-code error rates) in us-east-1 with dimensions DistributionId and Region=Global, and what makes the no-subscription control correctly withhold them (recorded behavior only; the Smithy model types DomainName as a plain string).

Emulator: DomainName is published as "<lowercased-distribution-id>.cloudfront.net", an address the emulator does not serve. The first viewer request is refused before it leaves the client: "endpoint_url was given https://<id>.cloudfront.net (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". The emulator announced an address other than the one it is reached on.

Why: a client that enables a CloudFront monitoring subscription and then drives traffic to measure it gets no datapoints at all — not because CloudWatch is wrong, but because nothing can reach the edge. Both the positive (metrics present after a subscription) and negative (metrics withheld without one) contracts are indistinguishable from each other.

Fix: the CloudFront distribution serializer must publish a DomainName on a host:port the emulator serves, and the edge request router must answer viewer requests on it and emit the AWS/CloudFront metric datapoints for each request outcome.

Traces: cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-match-recorded-edge-outcomes, cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-withheld-without-monitoring-subscription. Related open issues describe the same publication defect under other service headings (20261109T143515Z-cloudfront-logs-..., 20261109T152222Z-apigateway-cloudfront-s3-...); neither owns these checks.

## Evidence
Expected: GET https://<DomainName>/<path> is served by the emulator's edge, and GetMetricStatistics/GetMetricData on AWS/CloudFront returns datapoints for DistributionId=<id>, Region=Global.
Actual: simparity.published.PublishedUrlRejected: endpoint_url was given https://dgzni4pnd7h2s3.cloudfront.net (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener. (Both checks fail at this first request; the host is always the distribution id lowercased + ".cloudfront.net".)

## Steps to Reproduce
1. CreateDistribution over any origin and wait for Status=Deployed.
2. CreateMonitoringSubscription for that distribution.
3. Read Distribution.DomainName and send GET https://<DomainName>/<key> — the published host is <id>.cloudfront.net, which the emulator does not serve, so no request reaches the edge and no AWS/CloudFront datapoint is ever produced.

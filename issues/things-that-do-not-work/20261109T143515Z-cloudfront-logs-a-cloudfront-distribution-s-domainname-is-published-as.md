Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__logs.obs-cloudfront-3-vended-access-logs-match-requests-and-stop-after-delete-delivery cloudfront__logs.xsvc-cloudfront-3-standard-logging-v2-access-log-reaches-log-group
Issue-Key: cloudfront__logs.obs-cloudfront-3-vended-access-logs-match-requests-and-stop-after-delete-delivery|run#30|PublishedUrlRejected|base, route_headers = ctx.endpoint_route(domain)
Issue-Key: cloudfront__logs.xsvc-cloudfront-3-standard-logging-v2-access-log-reaches-log-group|run#38|PublishedUrlRejected|edge, route_headers = ctx.endpoint_route(dist["DomainName"])
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# A CloudFront distribution's DomainName is published as <id>.cloudfront.net, an address the emulator does not serve

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1: the distribution announces an AWS production hostname instead of the address it is reached on.
AWS: Distribution.DomainName is the host a viewer requests, and the recorded runs issue viewer requests against the domain the control plane reports, then match the vended access-log records to those requests (Smithy cloudfront Distribution.DomainName).
Emulator: the domain is published as 'https://<id>.cloudfront.net', whose host is not one of the emulator's own hosts, so no viewer request can be made — 'a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener'.
Why: the distribution is unreachable for any client that honours the published domain, so standard-logging-v2 access-log delivery and the stop-after-delete-delivery behaviour cannot be exercised at all.
Fix: the DomainName serializer/state in the cloudfront crate (publish the emulator's own host and port of the distribution's listener).
Traces: both checks in trace_ids.

## Evidence
Expected: the published distribution domain is reachable on the emulator. Actual: 'simparity.published.PublishedUrlRejected: endpoint_url was given https://dgzni4pnd7h2s3.cloudfront.net (published by the emulator or built by the check): its host is not one of the emulator's own hosts'.

## Steps to Reproduce
Replay cloudfront__logs.obs-cloudfront-3-vended-access-logs-match-requests-and-stop-after-delete-delivery and .xsvc-cloudfront-3-standard-logging-v2-access-log-reaches-log-group: both create a distribution, read its DomainName and issue viewer requests against it.

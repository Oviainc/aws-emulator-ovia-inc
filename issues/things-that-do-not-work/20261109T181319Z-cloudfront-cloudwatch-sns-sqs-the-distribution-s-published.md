Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__cloudwatch__sns__sqs.obs-cloudfront-2-4xx-error-rate-alarm-notifies-sqs-untouched-control-stays-ok
Issue-Key: cloudfront__cloudwatch__sns__sqs.obs-cloudfront-2-4xx-error-rate-alarm-notifies-sqs-untouched-control-stays-ok|run#18|PublishedUrlRejected|edge, route_headers = ctx.endpoint_route(subject["DomainName"])
Issue-Commit: 17c4643c966c9683f16ddaee3534ae6f6497ef31

# cloudfront__cloudwatch__sns__sqs: the distribution's published DomainName is an unserved <id>.cloudfront.net host, so the 4xxErrorRate alarm→SNS→SQS chain never starts

> _Note: this issue was identified against the code base at commit `17c4643c966c9683f16ddaee3534ae6f6497ef31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1: the CloudFront edge address the emulator publishes is not an address it serves, so no 4xx viewer outcome can be generated and the alarm chain under test never fires.

AWS: CreateDistribution returns Distribution.DomainName; viewer requests to https://<DomainName>/<path> are answered by the edge, a request for a missing object yields a 4xx, and that outcome feeds AWS/CloudFront 4xxErrorRate in us-east-1 (DistributionId, Region=Global). A CloudWatch alarm on that metric then goes ALARM, publishes to its SNS topic and the message lands on the subscribed SQS queue, while the untouched control distribution's alarm stays OK (recorded behavior only; DomainName is an unconstrained string in the Smithy model).

Emulator: DomainName is published as "<lowercased-distribution-id>.cloudfront.net", which the emulator does not serve. The first viewer request is refused outright: "endpoint_url was given https://<id>.cloudfront.net (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener".

Why: a client cannot exercise CloudFront-driven alerting at all. The alarm never leaves INSUFFICIENT_DATA/OK, no SNS notification is published and no SQS message arrives — and because the control also stays OK, a monitoring setup looks healthy while measuring nothing.

Fix: the CloudFront distribution serializer must publish a DomainName on a host:port the emulator serves, and the edge request router must answer viewer requests there and record the per-request 4xx outcome into AWS/CloudFront metrics.

Traces: cloudfront__cloudwatch__sns__sqs.obs-cloudfront-2-4xx-error-rate-alarm-notifies-sqs-untouched-control-stays-ok. The open issue 20261101T214154Z-cloudwatch-cloudfront-sns-sqs-a-deployed-distribution-serves.md describes a related symptom under a differently-ordered heading and does not own this check.

## Evidence
Expected: GET https://<DomainName>/<missing-key> is answered 4xx by the emulator's edge, AWS/CloudFront 4xxErrorRate rises, the alarm transitions to ALARM and an SNS notification is delivered to the SQS queue.
Actual: simparity.published.PublishedUrlRejected: endpoint_url was given https://dgzni4pnd7h2s3.cloudfront.net (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener.

## Steps to Reproduce
1. Create an SQS queue, an SNS topic subscribed to it and a CloudWatch alarm on AWS/CloudFront 4xxErrorRate for a new distribution (plus an untouched control distribution).
2. Wait for the distribution to be Deployed and read Distribution.DomainName.
3. Send GET https://<DomainName>/<missing-key> repeatedly — the published host is <id>.cloudfront.net, which the emulator does not serve, so no 4xx is ever recorded, the alarm never fires and no SQS message arrives.

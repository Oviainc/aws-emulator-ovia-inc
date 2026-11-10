Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__logs.xsvc-cloudfront-3-standard-logging-v2-access-log-reaches-log-group
Issue-Key: cloudfront__logs.xsvc-cloudfront-3-standard-logging-v2-access-log-reaches-log-group|_poll_until#43|Timeout|time.sleep(interval_s)
Issue-Commit: c52a7255ce2600c21f38f62abb43a8c83b37855a

# cloudfront__logs: CloudFront standard logging v2 delivers no access-log events to the destination CloudWatch log group, so the delivery never produces a record for a served edge request

> _Note: this issue was identified against the code base at commit `c52a7255ce2600c21f38f62abb43a8c83b37855a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDelivery

## Description
Cause 1 of 1: a CloudWatch Logs delivery wired from a CloudFront distribution (standard logging v2) never writes an access-log event for requests the edge serves.
AWS: recorded behavior only for the delivery path (the control plane is modelled by logs PutDeliverySource / PutDeliveryDestination / CreateDelivery, with logType ACCESS_LOGS for a CloudFront distribution resource ARN). Recorded behavior: once the delivery source, destination log group and delivery exist and the distribution is Deployed, each viewer request served by the distribution produces an access-log event in the destination log group, discoverable with FilterLogEvents/GetLogEvents within the delivery's propagation window and carrying the request's URI and status; deleting the delivery stops further events.
Emulator: after the edge serves the probe requests, the check polls the destination log group to its 180 s budget and no access-log event ever appears.
Why: CloudFront standard logging v2 is unobservable end to end. A client cannot validate that its delivery wiring works, cannot parse or alert on the log records, and cannot verify the "stops after DeleteDelivery" contract — an observability pipeline built against the emulator produces nothing and the gap is invisible until production.
Fix: the CloudFront edge data plane's vended-log emission — for each served request, resolve the distribution's standard-logging-v2 deliveries and write the access-log event to each delivery's destination (the edge-to-logs delivery path plus the logs delivery state that resolves source -> destination).
Traces: cloudfront__logs.xsvc-cloudfront-3-standard-logging-v2-access-log-reaches-log-group.

Note: the open 20261025T174921Z-logs-cloudfront-cloudfront-vended-access-logs-are-never.md owns a DIFFERENT check (cloudfront__logs.obs-cloudfront-3-vended-access-logs-match-requests-and-stop-after-delete-delivery) in the same family and does not carry this check's replay key, so it does not cover this failure. The open 20261109T143515Z-cloudfront-logs-a-cloudfront-distribution-s-domainname-is-published-as.md attributes the failure to an unserved <id>.cloudfront.net DomainName, which is no longer the observed behavior — the edge answers in this run.

## Evidence
"Error: Check timed out after 180.0s".
Expected: an access-log event in the delivery's destination CloudWatch log group for each request the distribution served, carrying that request's URI and status, within the delivery's propagation window.
Actual: the destination log group yields no matching event for the whole 180 s budget, although the edge served the probe request.

## Steps to Reproduce
1. Point cloudfront (us-east-1), logs and s3/iam clients at the emulator.
2. Create a distribution over a reachable origin and wait for Status Deployed.
3. logs.put_delivery_source(name=..., resourceArn=<distribution ARN>, logType="ACCESS_LOGS").
4. Create a destination CloudWatch log group and logs.put_delivery_destination(name=..., deliveryDestinationConfiguration={destinationResourceArn: <log group ARN>}).
5. logs.create_delivery(deliverySourceName=..., deliveryDestinationArn=...).
6. GET one or more objects through the distribution's DomainName and confirm they are served.
7. Poll logs.filter_log_events on the destination log group for an access-log event naming the requested URI -> none ever appears.
8. Or replay cloudfront__logs.xsvc-cloudfront-3-standard-logging-v2-access-log-reaches-log-group.

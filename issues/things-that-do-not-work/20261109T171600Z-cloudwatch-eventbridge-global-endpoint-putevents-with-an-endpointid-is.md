Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events__route53__sqs.cloudwatch-alarm-breach-fails-the-global-endpoint-over-to-secondary
Issue-Key: cloudwatch__events__route53__sqs|PutEvents|UnknownService
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# EventBridge global endpoint: PutEvents with an EndpointId is routed to an unregistered service token 'awsevents', so Route53-health-check failover is unreachable

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 1: a global-endpoint PutEvents never reaches an EventBridge handler.
AWS: `PutEvents` accepts an `EndpointId` naming an EventBridge global endpoint (Smithy: PutEventsRequest.EndpointId). The SDK resolves the global-endpoint host, EventBridge delivers the entry to the endpoint's primary Region's bus, and when the endpoint's Route 53 health check goes unhealthy (here, driven by a CloudWatch alarm breach) it fails the traffic over to the secondary Region's bus.
Emulator: the request is refused by the emulator's dispatcher before any handler runs — `botocore.exceptions.ClientError: An error occurred (UnknownService) when calling the PutEvents operation: no emulated service is registered for 'awsevents'`. The global-endpoint route resolves to the sdkId token `awsevents`, which is not registered alongside the `events`/`eventbridge` handlers.
Why: a client cannot use a global endpoint at all: it gets a non-AWS error code (`UnknownService`) instead of a 200 PutEventsResponse, so alarm-driven cross-Region failover never works and no failover test can even start.
Fix: the emulator's service/endpoint registry (register the `awsevents` sdkId token against the EventBridge handler set) and the PutEvents handler's EndpointId routing/failover.
Traces: cloudwatch__events__route53__sqs.cloudwatch-alarm-breach-fails-the-global-endpoint-over-to-secondary. The same root cause is reported for the `events` heading as part of the 7-cause EventBridge issue filed in this sweep (checks events.put-events-endpoint-id-delivers-to-primary-and-replicates-to-secondary and events.put-events-endpoint-id-with-unrelated-event-bus-name-rejected); this entry carries the replay key for the Route 53 failover check, which that one does not.

## Evidence
`botocore.exceptions.ClientError: An error occurred (UnknownService) when calling the PutEvents operation: no emulated service is registered for 'awsevents'`. Expected: HTTP 200 PutEventsResponse, with the entry landing on the primary Region's bus and, after the health check fails, on the secondary's.

## Steps to Reproduce
1. Create event buses in two Regions, a Route 53 health check backed by a CloudWatch alarm, and an EventBridge global endpoint over the two buses.
2. Call `events.put_events(Entries=[{...,'EventBusName':<bus>}], EndpointId='<endpoint-id>')`.
3. Observe `UnknownService` / `no emulated service is registered for 'awsevents'` instead of a 200 response.
4. Replay cloudwatch__events__route53__sqs.cloudwatch-alarm-breach-fails-the-global-endpoint-over-to-secondary.

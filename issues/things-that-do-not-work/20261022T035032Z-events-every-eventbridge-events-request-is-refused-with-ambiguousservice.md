Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__scheduler.eventbridge-templated-target-detail-type-and-source-reach-the-bus
Issue-Key: events__scheduler|CreateEventBus|AmbiguousService
Issue-Commit: 37e4b54b7f2f231778aa0090f22742466dbbad1b

# Every EventBridge ('events') request is refused with AmbiguousService: three emulated services all claim the 'events' signing name, so no EventBridge operation is reachable

> _Note: this issue was identified against the code base at commit `37e4b54b7f2f231778aa0090f22742466dbbad1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventBus

## Description
Cause 1 of 1: the emulator's front-door router refuses every request signed for the 'events' service because more than one registered emulated service claims that signing name, so no EventBridge operation can be invoked at all.

AWS: recorded behavior only for the routing layer (the router is emulator-internal). EventBridge is reached with sigv4 signing name / endpoint prefix 'events' and a request to it is dispatched to EventBridge; CreateEventBus (Smithy: com-amazonaws-eventbridge-awsevents, AWSEvents, awsJson1_1) answers 200 with the bus ARN, or a modeled error such as ResourceAlreadyExistsException / ValidationException. 'AmbiguousService' is not an AWS error code and appears in no AWS model.

Emulator: the router answers a non-AWS error with code AmbiguousService and the message "3 emulated services all claim requests for 'events': events, events, events; the request is rejected rather than served by an arbitrary one". Because the refusal is not a modeled throttling/retryable error, botocore exhausts its retries and surfaces it to the caller ("reached max retries: 3"). The collision is between the three registered services whose wire identity resolves to 'events' — service-com-amazonaws-eventbridge-awsevents (bootstrapped), service-com-amazonaws-cloudwatchevents-awsevents and service-com-amazonaws-eventbridgev2-awseventsv2 — so the one service that IS stood up is unreachable.

Why: a client gets an error code that exists in no AWS SDK, cannot be mapped to any modeled exception, and is retried pointlessly; every EventBridge operation (CreateEventBus, PutEvents, PutRule, ...) is 100% unavailable, and every cross-service scenario that routes an event through a bus — including EventBridge Scheduler's EventBridge target — cannot run.

Fix: the request router's service-resolution table — the signing name 'events' must resolve to exactly one registered service (disambiguate the duplicate registrations, e.g. by endpoint/target-prefix or API version, and prefer the single bootstrapped one) instead of rejecting. This is the same root defect already filed for other signing names (open issues 20261021T234042Z-osis-router-refuses-every-sigv4-iam-request-as-ambiguousservice, 20261022T031220Z-wisdom-all-wisdom-requests-rejected-with-ambiguousservice-two-emulated, 20261022T032542Z-application-autoscaling-lambda-lambda-is-unreachable-four-emulated-services-all-claim); none of them covers the 'events' collision.

Traces: events__scheduler.eventbridge-templated-target-detail-type-and-source-reach-the-bus

## Evidence
events__scheduler.eventbridge-templated-target-detail-type-and-source-reach-the-bus [error]:
  botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateEventBus operation (reached max retries: 3): 3 emulated services all claim requests for 'events': events, events, events; the request is rejected rather than served by an arbitrary one

Expected: CreateEventBus is routed to EventBridge and answers 200 with the created bus (or a modeled EventBridge error). Actual: a non-AWS 'AmbiguousService' refusal from the router before the service is ever asked.

## Steps to Reproduce
1. Boot the emulator.
2. events = boto3.client("events", endpoint_url=<emulator>)
3. events.create_event_bus(Name="any-bus")  ->  ClientError (AmbiguousService) after 3 retries; the same refusal applies to every 'events' operation (put_events, put_rule, describe_event_bus).

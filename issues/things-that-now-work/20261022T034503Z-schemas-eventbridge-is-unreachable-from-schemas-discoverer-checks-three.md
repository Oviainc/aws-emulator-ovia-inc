Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: schemas.create-discoverer-duplicate-source-rejected-conflict schemas.discoverer-lifecycle-start-stop-transitions schemas.update-discoverer-preserves-state
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|CreateEventBus|AmbiguousService
Issue-Commit: 37e4b54b7f2f231778aa0090f22742466dbbad1b

# EventBridge is unreachable from schemas discoverer checks: three emulated services all claim the 'events' signing name, so every CreateEventBus request is refused with AmbiguousService

> _Note: this issue was identified against the code base at commit `37e4b54b7f2f231778aa0090f22742466dbbad1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventBus

## Description
Every schemas check that needs an EventBridge event bus as the discoverer SourceArn fails before it reaches the schemas service at all, because the emulator's request router cannot decide which mounted service owns the 'events' endpoint/signing name.

Cause 1 of 1: the 'events' endpoint prefix is claimed by three mounted services, so the router refuses the request instead of dispatching it.
Failing checks: schemas.create-discoverer-duplicate-source-rejected-conflict, schemas.discoverer-lifecycle-start-stop-transitions, schemas.update-discoverer-preserves-state.

AWS: recorded behavior only. A botocore 'events' client calling CreateEventBus against real AWS reaches Amazon EventBridge and gets a 200 with EventBusArn; there is no such thing as an 'AmbiguousService' response on the AWS wire. The schemas Smithy model confirms the dependency is legitimate: CreateDiscoverer.SourceArn is a required @length(min 20, max 1600) string (an event-bus ARN), and CreateDiscoverer models ConflictException (409) for a duplicate source — the exact behavior these checks exercise.

Emulator: every CreateEventBus call is answered with error code 'AmbiguousService' and the message '3 emulated services all claim requests for \'events\': events, events, events; the request is rejected rather than served by an arbitrary one', retried to exhaustion by botocore ('reached max retries: 3'). Three distinct service crates in the repo expose the same 'events' wire identity (the EventBridge crate plus the cloudwatch-events and events-v2 crates), so the dispatcher's endpoint-prefix/signing-name index holds three candidates for one key and refuses rather than choosing.

Why: a client cannot create an event bus at all, and therefore cannot create, start, stop, update or describe a schemas Discoverer — the whole discoverer surface of the schemas service is unreachable even though its handlers may be correct. The failure is also mislabeled: the client sees a retryable-looking non-AWS error code instead of a modeled response, so SDK retry logic burns its budget before surfacing it.

Fix: the emulator's service-routing/registration layer (the dispatcher that maps a request's signing name / endpoint prefix to one mounted service), not a schemas handler. One service must win the 'events' key deterministically (the real EventBridge service), with the alias-only crates either not claiming the key or claiming a distinct one.

Traces: schemas.create-discoverer-duplicate-source-rejected-conflict (fails on the first CreateEventBus), schemas.discoverer-lifecycle-start-stop-transitions (same), schemas.update-discoverer-preserves-state (same). Related precedent already filed for other prefixes: 20261022T031220Z-wisdom-all-wisdom-requests-rejected-with-ambiguousservice-two-emulated.md and 20261022T032542Z-application-autoscaling-lambda-lambda-is-unreachable-four-emulated-services-all-claim.md — neither covers the 'events' key.

## Evidence
Expected (real AWS, recorded): events.create_event_bus(Name=..., Tags=[...]) -> HTTP 200 with EventBusArn, after which schemas.create_discoverer(SourceArn=<bus arn>) returns State=STARTED and a second CreateDiscoverer on the same SourceArn returns 409 ConflictException.

Actual (emulator): botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateEventBus operation (reached max retries: 3): 3 emulated services all claim requests for 'events': events, events, events; the request is rejected rather than served by an arbitrary one.

All three schemas discoverer checks abort at this call; no schemas operation is ever exercised.

## Steps to Reproduce
1. Boot the emulator and point a botocore 'events' client at it.
2. Call CreateEventBus(Name='sim2real-schemas-bus-x', Tags=[{'Key':'sim2real','Value':'true'}]).
3. Observe the AmbiguousService refusal naming three 'events' claimants instead of a 200 with EventBusArn.
4. Equivalently, replay schemas.discoverer-lifecycle-start-stop-transitions, schemas.create-discoverer-duplicate-source-rejected-conflict or schemas.update-discoverer-preserves-state.

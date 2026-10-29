Issue-Source: trace-sweep-divergence
Issue-Trace: events-connection-and-api-destination-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: events-connection-and-api-destination-lifecycle
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|CreateApiDestination|2xx->2xx|$.ApiDestinationState
Issue-Key: events|DescribeApiDestination|2xx->2xx|$.ApiDestinationState
Issue-Key: events|UpdateApiDestination|2xx->2xx|$.ApiDestinationState
Issue-Key: events|ListApiDestinations|2xx->2xx|$.ApiDestinations[Name="sim2real-probe-events-api-destination-dd290078"].ApiDestinationState
Issue-Commit: 10534c2dd099d7fcc20a17236779012152da4f21

# events api destinations never become ACTIVE: Create, Describe, Update and ListApiDestinations all report INACTIVE

> _Note: this issue was identified against the code base at commit `10534c2dd099d7fcc20a17236779012152da4f21`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApiDestination

## Description
An api destination created over a healthy connection is reported INACTIVE by every reader, so it never appears usable as a rule target. Four causes on the api-destination state and the four serializers that read it.

Cause 1 of 4 — CreateApiDestination returns ApiDestinationState=INACTIVE.
AWS: Smithy: CreateApiDestination output ApiDestinationState targets ApiDestinationState, enum = ACTIVE | INACTIVE. Recorded behavior: creating an api destination whose ConnectionArn names an existing, authorized connection answers 200 with ApiDestinationState 'ACTIVE'; INACTIVE is the state for a destination whose connection is not usable.
Emulator: answers 200 with $.ApiDestinationState = 'INACTIVE'.
Why: a client that gates PutTargets (or its own readiness check) on ACTIVE treats a correctly created destination as unusable and never proceeds. Because ApiDestinationState has only two values there is no 'still settling' reading — INACTIVE is a terminal, wrong answer.
Fix: the CreateApiDestination handler / api-destination state in the events crate — derive the state from the referenced connection's state instead of defaulting to INACTIVE. Note this is coupled to the connection state defect filed separately (connections never reach AUTHORIZED): the derivation must be fixed as well as the connection state it reads.
Traces: events-connection-and-api-destination-lifecycle#task#3.

Cause 2 of 4 — DescribeApiDestination keeps reporting INACTIVE.
AWS: Smithy: DescribeApiDestination output ApiDestinationState. Recorded behavior: 200 with 'ACTIVE'.
Emulator: 200 with 'INACTIVE'.
Why: shows the state never converges on the read path either, so polling cannot recover.
Fix: api-destination state in the events crate, read by the DescribeApiDestination handler.
Traces: events-connection-and-api-destination-lifecycle#task#4 (and task#6, the same cluster).

Cause 3 of 4 — UpdateApiDestination returns INACTIVE.
AWS: Recorded behavior: an UpdateApiDestination that changes the invocation endpoint/rate limit answers 200 with ApiDestinationState 'ACTIVE'.
Emulator: 200 with 'INACTIVE'.
Why: an update appears to deactivate the destination.
Fix: the UpdateApiDestination handler and the state it writes.
Traces: events-connection-and-api-destination-lifecycle#task#5.

Cause 4 of 4 — ListApiDestinations carries the same wrong state.
AWS: Recorded behavior: the listed entry for the destination carries ApiDestinationState 'ACTIVE'.
Emulator: the entry selected by Name carries 'INACTIVE'.
Why: inventory and CLI state filters never return a destination that is in fact usable.
Fix: the ListApiDestinations serializer, over the same stored state.
Traces: events-connection-and-api-destination-lifecycle#task#8.

Related (does not cover this): open issues 'EventBridge api-destination targets never deliver...' and 'EventBridge: an OAUTH_CLIENT_CREDENTIALS connection never obtains a token...' are about invocation/delivery, not the ApiDestinationState value on these four control-plane responses.

## Evidence
Replay of trace set simparity-wire-aws-v1 (fixture events-connection-and-api-destination-lifecycle), statuses 200->200 throughout:
- CreateApiDestination: $.ApiDestinationState expected 'ACTIVE', got 'INACTIVE'.
- DescribeApiDestination: $.ApiDestinationState expected 'ACTIVE', got 'INACTIVE' (cluster also covers task#6).
- UpdateApiDestination: $.ApiDestinationState expected 'ACTIVE', got 'INACTIVE'.
- ListApiDestinations: $.ApiDestinations[Name="..."].ApiDestinationState expected 'ACTIVE', got 'INACTIVE'.

## Steps to Reproduce
1. Replay fixture events-connection-and-api-destination-lifecycle (replay_wire_corpus).
2. task#3 CreateApiDestination with ConnectionArn naming the connection created in task#1 -> $.ApiDestinationState should be ACTIVE.
3. task#4 / task#6 DescribeApiDestination -> ACTIVE.
4. task#5 UpdateApiDestination -> ACTIVE.
5. task#8 ListApiDestinations -> $.ApiDestinations[<name>].ApiDestinationState ACTIVE.
All four report INACTIVE.

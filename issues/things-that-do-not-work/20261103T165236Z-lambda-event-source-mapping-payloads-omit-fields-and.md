Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-esm-dynamodb-stream-at-timestamp-starting-position-rejected
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-esm-dynamodb-stream-at-timestamp-starting-position-rejected lambda-onyx-traj-stream-mapping-discovery-by-function-and-by-stream lambda-onyx-traj-filter-criteria-fix-attempt-on-the-stream-mapping
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|CreateEventSourceMapping|2xx->2xx|$.DestinationConfig
Issue-Key: lambda|CreateEventSourceMapping|2xx->2xx|$.LastProcessingResult
Issue-Key: lambda|CreateEventSourceMapping|2xx->2xx|$.State
Issue-Key: lambda|CreateEventSourceMapping|2xx->2xx|$.StateTransitionReason
Issue-Key: lambda|CreateEventSourceMapping|2xx->2xx|$.TumblingWindowInSeconds
Issue-Key: lambda|GetEventSourceMapping|2xx->2xx|$.DestinationConfig
Issue-Key: lambda|GetEventSourceMapping|2xx->2xx|$.LastProcessingResult
Issue-Key: lambda|GetEventSourceMapping|2xx->2xx|$.StateTransitionReason
Issue-Key: lambda|GetEventSourceMapping|2xx->2xx|$.TumblingWindowInSeconds
Issue-Key: lambda|ListEventSourceMappings|2xx->2xx|$.EventSourceMappings[*].DestinationConfig
Issue-Key: lambda|ListEventSourceMappings|2xx->2xx|$.EventSourceMappings[*].LastProcessingResult
Issue-Key: lambda|ListEventSourceMappings|2xx->2xx|$.EventSourceMappings[*].StateTransitionReason
Issue-Key: lambda|ListEventSourceMappings|2xx->2xx|$.EventSourceMappings[*].TumblingWindowInSeconds
Issue-Key: lambda|GetEventSourceMapping|2xx->2xx|$.FilterCriteria.Filters[*].Pattern
Issue-Key: lambda|UpdateEventSourceMapping|2xx->2xx|$.DestinationConfig
Issue-Key: lambda|UpdateEventSourceMapping|2xx->2xx|$.FilterCriteria
Issue-Key: lambda|UpdateEventSourceMapping|2xx->2xx|$.LastProcessingResult
Issue-Key: lambda|UpdateEventSourceMapping|2xx->2xx|$.StateTransitionReason
Issue-Key: lambda|UpdateEventSourceMapping|2xx->2xx|$.TumblingWindowInSeconds
Issue-Key: lambda|GetEventSourceMapping|2xx->2xx|$.FilterCriteria
Issue-Commit: dd64cbf0b038e7955d699468def5671676941ec2

# Lambda event source mapping payloads omit fields and lose FilterCriteria

> _Note: this issue was identified against the code base at commit `dd64cbf0b038e7955d699468def5671676941ec2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
The EventSourceMappingConfiguration the emulator serializes omits modelled members, reports a non-AWS StateTransitionReason and a non-AWS initial State, and the FilterCriteria it stores is not the one the caller sent. 6 causes, all in the event-source-mapping projection and its FilterCriteria storage.

Cause 1 of 6 - CreateEventSourceMapping omits LastProcessingResult, DestinationConfig and TumblingWindowInSeconds, and reports the wrong State and StateTransitionReason.
AWS: Smithy: CreateEventSourceMapping output is EventSourceMappingConfiguration, whose members include LastProcessingResult (String), DestinationConfig, TumblingWindowInSeconds (range 0..900), State (String) and StateTransitionReason (String). On creation AWS reports State='Creating', StateTransitionReason='User action', LastProcessingResult='No records processed', TumblingWindowInSeconds=0 and a DestinationConfig with its OnFailure sub-structure present.
Emulator: omits LastProcessingResult, DestinationConfig and TumblingWindowInSeconds entirely, reports State='Disabled' and StateTransitionReason='USER_INITIATED' (an internal token, not AWS's sentence).
Why: a client that polls State until it leaves 'Creating' never sees the documented lifecycle; one that reads StateTransitionReason or LastProcessingResult to report mapping health gets nothing; one that reads DestinationConfig to find the on-failure destination sees no destination configured.
Fix: the event-source-mapping serializer and the create handler's initial-state assignment.
Traces: lambda-esm-dynamodb-stream-at-timestamp-starting-position-rejected#task#9.

Cause 2 of 6 - GetEventSourceMapping omits the same members and reports the same internal StateTransitionReason.
AWS: the same EventSourceMappingConfiguration members on the read path.
Emulator: the same omissions, confirming the defect is in the shared projection rather than in the create handler.
Fix: as cause 1.
Traces: lambda-esm-dynamodb-stream-at-timestamp-starting-position-rejected#task#10 (2 steps affected).

Cause 3 of 6 - ListEventSourceMappings omits the same members per entry.
AWS: Smithy: ListEventSourceMappings output EventSourceMappings -> EventSourceMappingConfiguration, with the same members as cause 1.
Emulator: each entry omits LastProcessingResult, DestinationConfig and TumblingWindowInSeconds and carries StateTransitionReason='USER_INITIATED'.
Fix: as cause 1.
Traces: lambda-onyx-traj-stream-mapping-discovery-by-function-and-by-stream#task#7 (2 steps affected).

Cause 4 of 6 - the FilterCriteria pattern read back is not the one that was configured.
AWS: Smithy: EventSourceMappingConfiguration.FilterCriteria -> Filters[].Pattern (String) - AWS stores and returns the caller's filter pattern verbatim, as a JSON document.
Emulator: returns a short non-JSON expression ('<field> = <value>') in place of the configured JSON pattern, i.e. the stored pattern is a reduced form of the input rather than the input.
Why: a client that reads the pattern back to confirm what records the mapping will accept sees a filter it did not write and cannot round-trip the mapping's configuration.
Fix: the event-source-mapping state's FilterCriteria storage (store the pattern document, not a parsed reduction) and the serializer that renders it.
Traces: lambda-onyx-traj-filter-criteria-fix-attempt-on-the-stream-mapping#task#10.

Cause 5 of 6 - clearing FilterCriteria leaves an empty Filters list instead of no FilterCriteria.
AWS: Smithy: FilterCriteria is an optional structure; when a mapping has no filter criteria AWS omits the member entirely from UpdateEventSourceMapping's response.
Emulator: returns FilterCriteria={'Filters': []}.
Why: a client that tests for the presence of FilterCriteria to decide whether filtering is active concludes a filter is configured when none is.
Fix: the event-source-mapping serializer must omit FilterCriteria when the mapping has none.
Traces: lambda-onyx-traj-filter-criteria-fix-attempt-on-the-stream-mapping#task#11.

Cause 6 of 6 - GetEventSourceMapping shows the same empty-Filters form.
AWS: as cause 5, on the read path.
Emulator: as cause 5.
Fix: as cause 5.
Traces: lambda-onyx-traj-filter-criteria-fix-attempt-on-the-stream-mapping#task#12.

Note: the open issues 20261103T022510Z-dynamodb-lambda-lambda-createeventsourcemapping-the-unrecognized-event-source-and-missing.md and 20261103T031416Z-dynamodb-lambda-3-net-new-causes-in-dynamodb.md touch the same area but carry none of these clusters' replay keys.

## Evidence
Cause 1: lambda-esm-dynamodb-stream-at-timestamp-starting-position-rejected#task#9 (202 vs 202) - $.LastProcessingResult expected 'No records processed', got absent; $.DestinationConfig expected {'OnSuccess': None, 'OnFailure': {'Destination': None}}, got absent; $.TumblingWindowInSeconds expected 0, got absent; $.State expected 'Creating', got 'Disabled'; $.StateTransitionReason expected 'User action', got 'USER_INITIATED'.
Cause 2: #task#10 (200 vs 200) - same four omissions/mismatches (2 steps affected).
Cause 3: lambda-onyx-traj-stream-mapping-discovery-by-function-and-by-stream#task#7 - the same on $.EventSourceMappings[0].* (2 steps affected).
Cause 4: lambda-onyx-traj-filter-criteria-fix-attempt-on-the-stream-mapping#task#10 - $.FilterCriteria.Filters[0].Pattern expected the configured JSON document, got 'eventName = INSERT'.
Cause 5: #task#11 (202 vs 202) - $.FilterCriteria expected absent, got {'Filters': []}.
Cause 6: #task#12 (200 vs 200) - same.

## Steps to Reproduce
1. Replay lambda-esm-dynamodb-stream-at-timestamp-starting-position-rejected task#9 and task#10 and compare every member of the EventSourceMappingConfiguration.
2. Replay lambda-onyx-traj-stream-mapping-discovery-by-function-and-by-stream task#7 and check the list entries.
3. Replay lambda-onyx-traj-filter-criteria-fix-attempt-on-the-stream-mapping task#10 (read the configured filter back), then task#11 and task#12 (clear it and read it back).

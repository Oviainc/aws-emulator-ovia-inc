Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.api-destination-lifecycle events.cancel-replay-in-flight-reaches-cancelled-and-stays-terminal events.delete-archive-not-blocked-by-in-flight-replay-and-start-replay-on-it-refused events.describe-replay-nonexistent-not-found events.e2e-bus-rule-archive-replay-redelivers-event events.events-archive-replay-filter-arn-naming-a-rule-on-another-bus-rejected events.events-archive-replay-to-a-bus-other-than-the-archive-source-rejected events.replay-completion-populates-timestamps-and-list-state-filter-discriminates events.replay-reaches-completed events.start-replay-duplicate-name-rejected events.start-replay-end-time-before-start-rejected events.start-replay-invalid-source-rejected events.delete-archive-removes-it events.delete-event-bus-blocked-while-an-archive-targets-it events.list-archives-filters-by-prefix-and-source events.list-replays-combined-filters-rejected events.update-archive-event-pattern-governs-what-is-archived-afterwards events.update-archive-pattern-is-mirrored-into-its-managed-rule-and-delete-removes-it
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|ListApiDestinations|NotImplemented
Issue-Key: events|StartReplay|NotImplemented
Issue-Key: events.events-archive-replay-filter-arn-naming-a-rule-on-another-bus-rejected|run#24|AssertionError|assert err == expected, (
Issue-Key: events.events-archive-replay-to-a-bus-other-than-the-archive-source-rejected|run#17|AssertionError|assert err == expected, (
Issue-Key: events|DeleteArchive|NotImplemented
Issue-Key: events|ListArchives|NotImplemented
Issue-Key: events|ListReplays|NotImplemented
Issue-Key: events|UpdateArchive|NotImplemented
Issue-Commit: 8b92040b06acdac381093d8b397331a442bd94ad

# EventBridge archive and replay control plane is unimplemented: six operations answer NotImplemented (6 causes)

> _Note: this issue was identified against the code base at commit `8b92040b06acdac381093d8b397331a442bd94ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListArchives

## Description
Six EventBridge control-plane operations in the archive/replay/api-destination families are still scaffold stubs: every call is answered with the emulator's own `NotImplemented` error instead of the modelled response. None of these six is covered by an existing ALREADY FILED / OWNED marker in the replay report, and none is named by the open stub issues 20261025T002357Z-events-describeendpoint-stub.md or 20261025T033539Z-events-createendpoint-stub.md.

Cause 1 of 6: ListApiDestinations is a scaffold stub.
AWS: Smithy (com-amazonaws-eventbridge-awsevents) models ListApiDestinations with input {NamePrefix, ConnectionArn, NextToken, Limit @range 1..100} and output {ApiDestinations: ApiDestinationResponseList, NextToken}; its only modelled error is InternalException (500). There is no client-error path for a well-formed call, so a well-formed call must answer HTTP 200 with the (possibly empty) list.
Emulator: `An error occurred (NotImplemented) when calling the ListApiDestinations operation: operation ListApiDestinations is scaffolded but not implemented in this emulator`.
Why: a client cannot enumerate API destinations at all, so no lifecycle or reconciliation flow over API destinations can run.
Fix: the ListApiDestinations handler in the eventbridge service crate (services/service-com-amazonaws-eventbridge-awsevents/src/ops/list_api_destinations.rs) plus the api-destination state it must read.
Traces: events.api-destination-lifecycle.

Cause 2 of 6: StartReplay is a scaffold stub.
AWS: Smithy models StartReplay with required {ReplayName, EventSourceArn (archive ARN), EventStartTime, EventEndTime, Destination} and output {ReplayArn, State, StateReason, ReplayStartTime}; modelled errors are InternalException, InvalidEventPatternException, LimitExceededException, ResourceAlreadyExistsException, ResourceNotFoundException. A valid request must answer 200 with a replay in a non-terminal State.
Emulator: `An error occurred (NotImplemented) when calling the StartReplay operation: operation StartReplay is scaffolded but not implemented in this emulator`.
Why: replay is unreachable, so neither the replay state machine (CancelReplay -> CANCELLING -> CANCELLED, terminal) nor replay redelivery can be used.
Fix: the StartReplay handler and the replay state machine in the eventbridge crate.
Traces: events.cancel-replay-in-flight-reaches-cancelled-and-stays-terminal.

Cause 3 of 6: DeleteArchive is a scaffold stub.
AWS: Smithy models DeleteArchive with required ArchiveName and an empty output; errors ConcurrentModificationException, InternalException, ResourceNotFoundException. Deleting an existing archive answers 200 and frees the name.
Emulator: `An error occurred (NotImplemented) when calling the DeleteArchive operation: operation DeleteArchive is scaffolded but not implemented in this emulator`.
Why: archives can be created but never removed, so the name is never reusable and cleanup leaks resources.
Fix: the DeleteArchive handler in the eventbridge crate and the archive registry it mutates.
Traces: events.delete-archive-removes-it.

Cause 4 of 6: ListArchives is a scaffold stub.
AWS: Smithy models ListArchives with optional filters {NamePrefix, EventSourceArn, State, NextToken, Limit @range 1..100} and output {Archives, NextToken}; errors InternalException, ResourceNotFoundException. AWS additionally refuses more than one filter per request (recorded behavior only; the check's recorded refusal message is `At most one filter is allowed for ListArchives. Use either : State, EventSourceArn, or NamePrefix.` and is cited only as an example).
Emulator: `An error occurred (NotImplemented) when calling the ListArchives operation: operation ListArchives is scaffolded but not implemented in this emulator`.
Why: archives cannot be enumerated or filtered, and the combined-filter refusal rule can never be exercised.
Fix: the ListArchives handler in the eventbridge crate, including its one-filter-only validation.
Traces: events.list-archives-filters-by-prefix-and-source.

Cause 5 of 6: ListReplays is a scaffold stub.
AWS: Smithy models ListReplays with optional filters {NamePrefix, State, EventSourceArn, NextToken, Limit @range 1..100} and output {Replays, NextToken}; only modelled error is InternalException. AWS refuses a request combining filters with ValidationException (recorded behavior only; recorded example message `At most one filter is allowed for ListReplays. Use either : State, EventSourceArn, or NamePrefix.`).
Emulator: `An error occurred (NotImplemented) when calling the ListReplays operation: operation ListReplays is scaffolded but not implemented in this emulator`.
Why: replays cannot be enumerated, and the combined-filter ValidationException is unreachable.
Fix: the ListReplays handler in the eventbridge crate, including its one-filter-only validation.
Traces: events.list-replays-combined-filters-rejected.

Cause 6 of 6: UpdateArchive is a scaffold stub.
AWS: Smithy models UpdateArchive with required ArchiveName plus optional {Description, EventPattern @length 0..4096, RetentionDays, KmsKeyIdentifier} and output {ArchiveArn, State, StateReason, CreationTime}; errors ConcurrentModificationException, InternalException, InvalidEventPatternException, LimitExceededException, ResourceNotFoundException. A valid update answers 200 and the NEW EventPattern governs what the archive ingests from then on.
Emulator: `An error occurred (NotImplemented) when calling the UpdateArchive operation: operation UpdateArchive is scaffolded but not implemented in this emulator`.
Why: an archive's pattern and retention can never be changed, so a consumer that narrows or widens an archive silently keeps the original filter.
Fix: the UpdateArchive handler in the eventbridge crate and the archive-ingest filter it must re-read.
Traces: events.update-archive-event-pattern-governs-what-is-archived-afterwards.

## Evidence
All six checks fail with the emulator's scaffold error on the first call to the named operation.

- events.api-destination-lifecycle [error]: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListApiDestinations operation: operation ListApiDestinations is scaffolded but not implemented in this emulator. Expected: HTTP 200 with {ApiDestinations: [...], NextToken?}.
- events.cancel-replay-in-flight-reaches-cancelled-and-stays-terminal [error]: ... (NotImplemented) when calling the StartReplay operation ... Expected: HTTP 200 with {ReplayArn, State, ReplayStartTime}.
- events.delete-archive-removes-it [error]: ... (NotImplemented) when calling the DeleteArchive operation ... Expected: HTTP 200, empty body; a later DescribeArchive on the deleted name -> ResourceNotFoundException.
- events.list-archives-filters-by-prefix-and-source [error]: ... (NotImplemented) when calling the ListArchives operation ... Expected: HTTP 200 with the filtered archive list; a two-filter request -> ValidationException.
- events.list-replays-combined-filters-rejected [error]: ... (NotImplemented) when calling the ListReplays operation ... Expected: HTTP 200 with the replay list; a two-filter request -> ValidationException.
- events.update-archive-event-pattern-governs-what-is-archived-afterwards [error]: ... (NotImplemented) when calling the UpdateArchive operation ... Expected: HTTP 200 with {ArchiveArn, State, StateReason, CreationTime} and subsequent ingestion filtered by the new pattern.

## Steps to Reproduce
1. Boot the emulator and point an events client at it.
2. events.list_api_destinations() -> NotImplemented instead of 200 with a list.
3. events.create_event_bus(Name=B); events.create_archive(ArchiveName=A, EventSourceArn=<bus arn>); events.delete_archive(ArchiveName=A) -> NotImplemented.
4. events.list_archives(NamePrefix='x') -> NotImplemented; events.list_archives(NamePrefix='x', State='ENABLED') -> expected ValidationException.
5. events.update_archive(ArchiveName=A, EventPattern='{"source":["s"]}') -> NotImplemented.
6. events.start_replay(ReplayName=R, EventSourceArn=<archive arn>, EventStartTime=..., EventEndTime=..., Destination={'Arn': <bus arn>}) -> NotImplemented.
7. events.list_replays(NamePrefix='x') -> NotImplemented.
Replay the six check ids in trace_ids to confirm.

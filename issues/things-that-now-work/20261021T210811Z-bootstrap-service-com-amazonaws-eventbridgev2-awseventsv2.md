Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the EventBridgeV2 (awseventsv2) emulator

_Filed: 2026-10-21T21:08:11Z_
_Operation: service-com-amazonaws-eventbridgev2-awseventsv2 | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 25 operations in `services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/` is an unmodified scaffold returning HTTP 501 `NotImplemented`, and `src/state.rs` has no fields (`// TODO: add service-specific state fields`). No client workflow can run.

Implement a SMALL first slice: the event-bus lifecycle plus event publication, all computed from persisted state.

### Happy path (in order)

1. **CreateEventBus** — input `Name` (required, len 1..256, pattern `^[A-Za-z0-9][\.\-_A-Za-z0-9]*$`), `Description`, `EncryptionConfiguration`, `StorageConfiguration`, `Tags`, `ClientToken` (@idempotencyToken). Must mint and STORE an `EventBusArn` matching the modelled pattern `arn:aws:events:<region>:<account>:event-busv2/<name>/<id>`, and return `EventBusArn`, `Name`, `Description`, `EncryptionConfiguration`, `StorageConfiguration`, `State` = `ACTIVE` (from the `BusState` enum), `CreationTime`. A second create with the same `Name` must fail `ResourceAlreadyExistsException` (HTTP 409); a repeat with the same `ClientToken` and identical inputs must return the original bus (and `IdempotentParameterMismatchException` when inputs differ).
2. **DescribeEventBus** — input `EventBusArn` (required). Must look the bus up by ARN and return exactly what was created: `EventBusArn`, `Name`, `Description`, `EncryptionConfiguration`, `StorageConfiguration`, `CreationTime`, `LastModifiedTime`, `State`, `StateReason`. Unknown/deleted ARN → `ResourceNotFoundException` (HTTP 404). No echoing of input into a canned body.
3. **ListEventBuses** — input `NamePrefix`, `EventBusAccountId`, `MaxResults` (1..100, default 100), `NextToken`. Must enumerate the stored buses as `EventBuses` (list<EventBusSummary>), honour `NamePrefix` filtering, truncate at `MaxResults` and return an opaque `NextToken` that resumes exactly where the previous page stopped (operation is `@paginated(inputToken=NextToken, outputToken=NextToken, items=EventBuses)`).
4. **PutEvents** — input `EventBusArn` (required), `Entries` (list<PutEventsRequestEntry>, len 1..100, required), `DeduplicationConfiguration`. Must verify the target bus exists (else `ResourceNotFoundException` 404), append each accepted entry to that bus's event log with a generated per-entry `EventId`, and return `FailedEntryCount` (0 on full success — computed, not hardcoded) and `Entries` (list<PutEventsResultEntry>) positionally aligned with the request, each successful element carrying its `EventId` and each rejected one `ErrorCode`/`ErrorMessage`.
5. **DeleteEventBus** — input `EventBusArn` (required), `@idempotent`. Must remove the bus (and its events/tags/policies) from state and return an empty body. A subsequent DescribeEventBus on the same ARN must then return `ResourceNotFoundException` (404), and the bus must disappear from ListEventBuses.

### Rules

- No canned, echoed, or hardcoded responses, and no random-ID-without-storage: every field above must be read back out of the store that CreateEventBus wrote. `FailedEntryCount` and `State` in particular must be derived values.
- Reach the store through `crate::state::service_state(&*self.engine)` only — never construct a fresh `InMemoryEngine` in a handler.
- Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line from each handler you implement.
- All 20 remaining operations (event sources, subscribers, resource policies, tagging, RevokeResource, PutRawEvents, Update*) stay 501 for now and will be filed as individual gap issues.

## State Model

Add to `ComAmazonawsEventbridgev2Awseventsv2State`:

- `event_buses: BTreeMap<String /* EventBusArn */, EventBus>` — keyed by ARN so ARN-addressed reads are O(log n); plus a `names: BTreeMap<String /* Name */, String /* arn */>` index (or a name scan) to enforce name uniqueness and `NamePrefix` filtering/ordering.
- `struct EventBus { arn: String, name: String, account_id: String, region: String, description: Option<String>, encryption_configuration: Option<EncryptionConfiguration>, storage_configuration: Option<StorageConfigurationOutput>, state: BusState /* ACTIVE */, state_reason: Option<String>, creation_time: SystemTime, last_modified_time: SystemTime, tags: BTreeMap<String,String>, events: Vec<StoredEvent>, client_token: Option<String> }`
- `struct StoredEvent { event_id: String /* uuid */, source: Option<String>, detail_type: Option<String>, detail: Option<String>, resources: Vec<String>, time: SystemTime, received_at: SystemTime, dedup_id: Option<String> }` — appended in publication order by PutEvents so a later reader/subscriber slice can consume the log.
- `idempotency: BTreeMap<String /* ClientToken */, String /* created arn */>` — backs ClientToken replay and `IdempotentParameterMismatchException`.

Done when an integration test runs CreateEventBus → DescribeEventBus → ListEventBuses → PutEvents → DeleteEventBus → DescribeEventBus(404) and every assertion on the response BODIES (not just 2xx) holds.

## Evidence

Spec: `services/service-com-amazonaws-eventbridgev2-awseventsv2/smithy-model/com-amazonaws-eventbridgev2-awseventsv2.json`, as denormalized in the crate's `OPERATIONS.md` — CreateEventBus (§Input `Name` required / §Output `EventBusArn`,`State`(BusState),`CreationTime`; errors ResourceAlreadyExistsException 409, IdempotentParameterMismatchException 409), DescribeEventBus (@readonly, error ResourceNotFoundException 404), ListEventBuses (@paginated inputToken=NextToken outputToken=NextToken items=EventBuses, `MaxResults` 1..100 default 100, `NamePrefix`), PutEvents (`Entries` 1..100 required, output `FailedEntryCount` required + `Entries` list<PutEventsResultEntry>, error ResourceNotFoundException 404), DeleteEventBus (@idempotent, errors ResourceInUseException 409 / ResourceNotFoundException 404).

Implementation: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/` matches all 25 handler files, each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501. `src/state.rs` line 8 is `// TODO: add service-specific state fields`, so the service holds zero state. No test asserts any of this behaviour (only the generated `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. Start the emulator and point an EventBridgeV2 (`eventsv2` endpoint prefix, sigv4 name `events`) client at it.
2. CreateEventBus with Name="demo-bus", Description="bootstrap demo" — expect 200 with an EventBusArn matching ^arn:aws:events:...:event-busv2/demo-bus/..., State=ACTIVE and a CreationTime; actual: HTTP 501 NotImplemented.
3. DescribeEventBus with the returned EventBusArn — expect the same Name/Description/CreationTime/State read back from state.
4. ListEventBuses with NamePrefix="demo" and MaxResults=1 — expect the bus in EventBuses, plus a NextToken only when more buses remain.
5. PutEvents on that EventBusArn with 2 entries (Source/DetailType/Detail) — expect FailedEntryCount=0 and 2 result entries each with a distinct EventId.
6. PutEvents on a well-formed but nonexistent event-busv2 ARN — expect ResourceNotFoundException (HTTP 404), not 501.
7. DeleteEventBus with the ARN — expect 200 empty body; then DescribeEventBus on the same ARN must return ResourceNotFoundException (404) and ListEventBuses must no longer include it.

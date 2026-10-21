Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 9fd9433686ca60725b6b64638320351edab7ca3b

# Bootstrap the AppIntegrations emulator

_Filed: 2026-10-21T21:53:00Z_
_Operation: service-com-amazonaws-appintegrations-amazonappintegrationservice | Class: stub_

> _Note: this issue was identified against the code base at commit `9fd9433686ca60725b6b64638320351edab7ca3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-appintegrations-amazonappintegrationservice/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no AppIntegrations client workflow can run. Implement a FIRST, SMALL stateful slice: the event-integration lifecycle, which is AppIntegrations' primary resource (restJson1, sigv4 signing name `app-integrations`).

Happy path to implement, in order (fields from OPERATIONS.md):

1. **CreateEventIntegration** — `POST /eventIntegrations`. Input: `Name` (required, 1..255, `^[a-zA-Z0-9\/\._\-]+$`), `EventFilter` (required, carries `Source`), `EventBridgeBus` (required, 1..255), `Description`, `ClientToken` (@idempotencyToken), `Tags`. Must STORE the record keyed by `Name` and return the generated `EventIntegrationArn` (`arn:aws:app-integrations:<region>:<account>:event-integration/<Name>`). A second create with the same `Name` must fail `DuplicateResourceException` (HTTP 409).
2. **GetEventIntegration** — `GET /eventIntegrations/{Name}`. Must return the PERSISTED `Name`, `Description`, `EventIntegrationArn`, `EventBridgeBus`, `EventFilter`, `Tags` exactly as created — not echoed from the request and not canned. Unknown name -> `ResourceNotFoundException` (HTTP 404).
3. **ListEventIntegrations** — `GET /eventIntegrations`. Must enumerate all stored integrations as `EventIntegrations` (list<EventIntegration>: `Name`, `Description`, `EventIntegrationArn`, `EventBridgeBus`, `EventFilter`, `Tags`), honouring `MaxResults` (1..50) and round-tripping `NextToken` per the `@paginated(inputToken=NextToken, outputToken=NextToken, items=EventIntegrations)` trait (an opaque offset cursor is fine).
4. **UpdateEventIntegration** — `PATCH /eventIntegrations/{Name}`. Input: `Name` (@httpLabel), `Description`. Must mutate the stored record so a following GetEventIntegration returns the NEW description; unknown name -> `ResourceNotFoundException` (404). Empty output body.
5. **DeleteEventIntegration** — `DELETE /eventIntegrations/{Name}`. Must remove the stored record; a following GetEventIntegration must return `ResourceNotFoundException` (404), and a second delete must also 404. Empty output body.

Rules for this slice: every response must be computed from persisted state held in the shared engine store reached via `crate::state::service_state(&*self.engine)` — no hardcoded ARNs that are not also stored, no echoing the request back, no generating an id without recording it. Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line from each of the five files as it is implemented.

## State Model

In `src/state.rs`, on the service state struct:

- `event_integrations: BTreeMap<String /* Name */, EventIntegrationRecord>` — ordered by name so list/pagination is deterministic.
- `EventIntegrationRecord { name: String, arn: String, description: Option<String>, event_bridge_bus: String, event_filter: EventFilterRecord, tags: BTreeMap<String, String>, client_token: Option<String>, created_time: SystemTime, last_modified_time: SystemTime }`
- `EventFilterRecord { source: String }`
- `idempotency: BTreeMap<String /* ClientToken */, String /* Name */>` — so a repeated CreateEventIntegration with the same `ClientToken` returns the first ARN instead of a duplicate error.

ARN construction: `arn:aws:app-integrations:{region}:{account}:event-integration/{Name}` (region/account from request context or emulator defaults), stored on the record so Get/List return the identical string Create returned.

Out of scope for this first slice (stay 501, filed separately later): applications (Create/Get/Update/Delete/ListApplications/ListApplicationAssociations), data integrations and their associations, ListEventIntegrationAssociations, and the tagging operations (TagResource/UntagResource/ListTagsForResource).

## Evidence

OPERATIONS.md for this crate documents all 23 operations, including CreateEventIntegration (`POST /eventIntegrations`, required `Name`/`EventFilter`/`EventBridgeBus`, output `EventIntegrationArn`, error `DuplicateResourceException` 409), GetEventIntegration (`GET /eventIntegrations/{Name}` returning Name/Description/EventIntegrationArn/EventBridgeBus/EventFilter/Tags, error `ResourceNotFoundException` 404), ListEventIntegrations (`GET /eventIntegrations`, `@paginated(inputToken=NextToken, outputToken=NextToken, items=EventIntegrations)`, `MaxResults` range 1..50), UpdateEventIntegration (`PATCH /eventIntegrations/{Name}`, `Description`, 404) and DeleteEventIntegration (`DELETE /eventIntegrations/{Name}`, 404). `src/ops/create_event_integration.rs` lines 4-6 carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and line 180 returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateEventIntegration"))`; every other file in `src/ops/` is the same generated stub, and the crate's README.md documents that these answer HTTP 501 until written. No test asserts any AppIntegrations behaviour (only `tests/state_roundtrip.rs`, a state-API example).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `app-integrations` at it.
2. `aws appintegrations create-event-integration --name demo-integration --event-bridge-bus default --event-filter Source=aws.partner/demo --description "first"` -> expect 200 with an `EventIntegrationArn` ending in `event-integration/demo-integration`; today returns HTTP 501 NotImplemented.
3. `aws appintegrations get-event-integration --name demo-integration` -> expect the persisted Name, Description "first", the SAME ARN returned by create, EventBridgeBus `default` and EventFilter Source `aws.partner/demo`.
4. `aws appintegrations list-event-integrations --max-results 1` -> expect `demo-integration` in `EventIntegrations` and, if more exist, a `NextToken` that returns the next page when passed back.
5. `aws appintegrations create-event-integration --name demo-integration ...` again -> expect HTTP 409 `DuplicateResourceException`.
6. `aws appintegrations update-event-integration --name demo-integration --description "second"` then get-event-integration -> expect Description "second".
7. `aws appintegrations delete-event-integration --name demo-integration` then get-event-integration -> expect HTTP 404 `ResourceNotFoundException`, and list-event-integrations no longer contains it.

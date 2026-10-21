Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 0761ec9a912f1ad457725ac2bfa14369ca92ad04

# Bootstrap the Personalize Events emulator

_Filed: 2026-10-21T23:12:52Z_
_Operation: service-com-amazonaws-personalizeevents-amazonpersonalizeevents | Class: missing_

> _Note: this issue was identified against the code base at commit `0761ec9a912f1ad457725ac2bfa14369ca92ad04`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 5 operations bound by this service (`PutEvents`, `PutItems`, `PutUsers`, `PutActions`, `PutActionInteractions`) is an unmodified scaffold: the handler files under `src/ops/` all carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and `src/state.rs` holds no fields (`// TODO: add service-specific state fields`). Nothing can be ingested or later observed.

This is a pure ingestion service (restJson1, endpoint prefix `personalize-events`, signing name `personalize`): every operation takes a batch of records and returns an EMPTY 200 body. A happy path therefore means: the request is validated as the real service validates it, the records are PERSISTED in the engine state, and a repeat/overlapping call behaves consistently (event de-duplication by `eventId`, item/user/action metadata upserted by id). The response body stays empty — but the stored state must be real, because the emulator's value is that a later test (or a Personalize-side emulator) can read back exactly what was ingested.

## Happy path to implement first (all 5 operations, in this order)

1. **PutEvents** — `POST /events` -> 200, empty body. Input: `trackingId` (required, 1..256), `sessionId` (required, 1..256), `userId` (optional), `eventList` (required, 1..10 `Event`). Each `Event`: `eventType` (required), `sentAt` (required timestamp), optional `eventId`, `eventValue` (float), `itemId`, `properties` (JSON string, 1..1024), `recommendationId` (1..40), `impression` (list, 1..25), `metricAttribution`. Behaviour: append one stored interaction per event to the event stream for `trackingId`, stamping the session/user; generate an `eventId` when the caller omits one; treat a repeated `eventId` within the same tracker as the SAME event (upsert, not a second record) as Amazon Personalize documents. Reject malformed `properties` JSON, or `properties` keys in the reserved set (`userId`, `sessionId`, `eventType`, `timestamp`, `recommendationId`, `impression`, case-insensitive), with `InvalidInputException` (HTTP 400) — the only error this operation models.
2. **PutItems** — `POST /items` -> 200. Input: `datasetArn` (required, must match `^arn:([a-z\d-]+):personalize:.*:.*:.+$`), `items` (1..10 `Item`, each `itemId` required + optional `properties` JSON 1..32000). Behaviour: upsert each item into the items map for that dataset ARN, merging/replacing properties for a repeated `itemId`.
3. **PutUsers** — `POST /users` -> 200. Input: `datasetArn` (required, same pattern), `users` (1..10 `User`, each `userId` required + optional `properties` JSON 1..24000). Behaviour: upsert by `userId` into the users map for that dataset ARN.
4. **PutActions** — `POST /actions` -> 200. Input: `datasetArn` (required), `actions` (1..10 `Action`, each `actionId` required + optional `properties` JSON 1..32000). Behaviour: upsert by `actionId` into the actions map for that dataset ARN.
5. **PutActionInteractions** — `POST /action-interactions` -> 200. Input: `trackingId` (required), `actionInteractions` (1..10 `ActionInteraction`: `actionId`, `sessionId`, `timestamp`, `eventType` all required; optional `userId`, `eventId`, `recommendationId`, `impression`, `properties` 1..1024). Behaviour: append to the action-interaction stream for `trackingId`, same `eventId` generation/de-dup and same reserved-`properties`-key validation as `PutEvents`.

Error semantics that must come from state, not be hardcoded: `PutItems`/`PutUsers`/`PutActions`/`PutActionInteractions` model `ResourceNotFoundException` (404) and `ResourceInUseException` (409). For a first slice, return `ResourceNotFoundException` when the target `datasetArn` / `trackingId` has never been registered in state — i.e. the emulator keeps an explicit registry of known trackers and dataset ARNs that an integration test (or a sibling Personalize emulator) can seed, rather than accepting any string. Every one of the five handlers must compute its answer from that persisted state: no canned 200, no echoed input, no accept-and-discard.

## State Model

Add to `ComAmazonawsPersonalizeeventsAmazonpersonalizeeventsState`:

- `event_trackers: HashMap<String /* trackingId */, EventTracker>` where
  `EventTracker { tracking_id: String, dataset_group_arn: Option<String>, events: Vec<StoredEvent>, action_interactions: Vec<StoredActionInteraction>, event_ids: HashSet<String> }`
- `StoredEvent { event_id: String, event_type: String, sent_at: DateTime, received_at: DateTime, session_id: String, user_id: Option<String>, item_id: Option<String>, event_value: Option<f32>, properties: Option<serde_json::Value>, recommendation_id: Option<String>, impression: Vec<String>, metric_attribution: Option<MetricAttribution> }`
- `StoredActionInteraction { event_id: String, action_id: String, session_id: String, user_id: Option<String>, event_type: String, timestamp: DateTime, recommendation_id: Option<String>, impression: Vec<String>, properties: Option<serde_json::Value> }`
- `datasets: HashMap<String /* datasetArn */, Dataset>` where
  `Dataset { arn: String, items: HashMap<String /* itemId */, Record>, users: HashMap<String /* userId */, Record>, actions: HashMap<String /* actionId */, Record> }`
  and `Record { id: String, properties: Option<serde_json::Value>, updated_at: DateTime }`

Operations stay 501 only if they are outside this list — here that is nothing, because the service binds exactly these five; the long tail for this crate is depth (validation detail, property merge rules, impression/metric-attribution fidelity), filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-personalizeevents-amazonpersonalizeevents/OPERATIONS.md` lists all 5 operations with routes, required members and errors (e.g. `PutEvents: POST /events -> 200`, input `trackingId`/`sessionId`/`userId`/`eventList`, error `InvalidInputException (HTTP 400)`; `PutItems`/`PutUsers`/`PutActions` require a `datasetArn` matching `^arn:([a-z\d-]+):personalize:.*:.*:.+$` and model `ResourceInUseException (409)` / `ResourceNotFoundException (404)`). `src/ops/put_events.rs` lines 4-6 carry the `SCAFFOLDED, NOT IMPLEMENTED` marker and line 178 returns `Scaffolded::not_implemented("PutEvents")`; the sibling files `put_items.rs`, `put_users.rs`, `put_actions.rs`, `put_action_interactions.rs` are the same generated stub. `src/state.rs` line 8 is still `// TODO: add service-specific state fields`, so no state exists to compute any response from. Member-level requirements (`Event.eventType`/`sentAt` required, `eventId` optional and server-generated, reserved `properties` keys, `Item.itemId`/`User.userId`/`Action.actionId` required, `ActionInteraction.actionId`/`sessionId`/`timestamp`/`eventType` required, length bounds on `properties`) come from the Smithy shapes `Event`, `Item`, `User`, `Action`, `ActionInteraction` in the model.

## Steps to Reproduce

1. Register (seed) an event tracker with trackingId `tracker-1` and a dataset ARN `arn:aws:personalize:us-east-1:123456789012:dataset/demo/ITEMS` in the emulator state.
2. `PutItems` with datasetArn = that ARN and items `[{itemId: "item-1", properties: "{\"genre\":\"Horror|Action\"}"}, {itemId: "item-2"}]` -> expect HTTP 200 with an empty body, and both items stored under that dataset.
3. `PutUsers` with the same ARN and users `[{userId: "user-1", properties: "{\"numberOfVideosWatched\":\"45\"}"}]` -> 200; `PutActions` with actions `[{actionId: "action-1", properties: "{\"value\":\"100\"}"}]` -> 200.
4. `PutEvents` with trackingId `tracker-1`, sessionId `session-1`, userId `user-1`, eventList `[{eventId: "e-1", eventType: "click", sentAt: <now>, itemId: "item-1", eventValue: 1.0}, {eventType: "watch", sentAt: <now>, itemId: "item-2"}]` -> 200 empty body; the tracker's event stream now holds 2 events, the second with a server-generated eventId.
5. Replay the identical `PutEvents` call -> 200, and the stored stream still holds the `e-1` event ONCE (de-duplicated by eventId), not twice.
6. `PutActionInteractions` with trackingId `tracker-1` and actionInteractions `[{actionId: "action-1", sessionId: "session-1", userId: "user-1", eventType: "Taken", timestamp: <now>}]` -> 200, one interaction persisted.
7. `PutItems` with datasetArn `arn:aws:personalize:us-east-1:123456789012:dataset/unknown/ITEMS` -> expect `ResourceNotFoundException` (HTTP 404), not 200 and not 501.
8. `PutEvents` with an event whose `properties` is `"{\"sessionId\":\"x\"}"` (reserved key) or is not valid JSON -> expect `InvalidInputException` (HTTP 400).
9. Re-`PutItems` `item-1` with `properties: "{\"genre\":\"Comedy\"}"` -> 200, and the stored record for `item-1` reflects the new properties (upsert, not a duplicate record).

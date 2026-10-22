Issue-Severity: HIGH
Issue-Gap-Class: missing

# Wire the Personalize control plane to the Personalize Events registry

_Operation: service-com-amazonaws-personalize-amazonpersonalize | Class: missing_

## Description

`service-com-amazonaws-personalizeevents-amazonpersonalizeevents` ingests only into a
target it knows: all five of its operations (`PutEvents`, `PutItems`, `PutUsers`,
`PutActions`, `PutActionInteractions`) refuse an unknown `trackingId` / `datasetArn` —
`ResourceNotFoundException` (404) where the operation models it, `InvalidInputException`
(400) for `PutEvents`, which models no not-found error. That is faithful: a `trackingId`
is minted by `CreateEventTracker` and a `datasetArn` by `CreateDataset`, and BOTH live in
the Amazon Personalize control plane — a different service
(`service-com-amazonaws-personalize-amazonpersonalize`), not the ingestion crate.

Today the registry those refusals read is reachable only from Rust
(`ComAmazonawsPersonalizeeventsAmazonpersonalizeeventsState::register_event_tracker` /
`register_dataset`, documented in the ingestion crate's `README.md`), and this crate's own
integration tests are its only callers. So an SDK client talking to the running emulator
has no wire path to register a target, and every ingestion call it makes can only be
refused.

## What to implement

In `service-com-amazonaws-personalize-amazonpersonalize`, when `CreateEventTracker` mints a
tracking id and `CreateDataset` mints a dataset ARN, seed the ingestion crate's registry
from the SAME shared engine the router hands both services:

```rust
let events = engine.state::<
    service_com_amazonaws_personalizeevents_amazonpersonalizeevents::
        ComAmazonawsPersonalizeeventsAmazonpersonalizeeventsState,
>();
events.lock().register_event_tracker(&tracking_id, Some(dataset_group_arn));
// ... and register_dataset(&dataset_arn) from CreateDataset
```

Both seam methods are idempotent, never discard what was already ingested, and answer
`None` once the store holds its documented maximum (refuse the create, or report the
emulator's capacity, rather than ignoring the answer). The engine is shared, so the
ingestion crate needs no change: it already reads whatever the control plane registered.

A cross-service test belongs in `core/router/tests/all/`: create an event tracker and a
dataset through `personalize`, then ingest through `personalize-events` over the wire and
assert the 200 — the first end-to-end path that does not need in-process seeding.

## Evidence

`services/service-com-amazonaws-personalizeevents-amazonpersonalizeevents/src/state.rs`
(module doc, `register_event_tracker`, `register_dataset`) and that crate's `README.md`
section "Seeding the event trackers and datasets this service ingests into"; the refusals
are in `src/ops/put_events.rs`, `put_items.rs`, `put_users.rs`, `put_actions.rs` and
`put_action_interactions.rs`, and are covered by
`tests/all/ingestion.rs::an_unregistered_target_is_refused_by_every_operation_that_models_not_found`.

## Steps to Reproduce

1. Boot the emulator and call `PutItems` with any `datasetArn` -> `ResourceNotFoundException`
   (404), because nothing registered it.
2. Call `CreateDataset` on `personalize` with that ARN's dataset, then repeat step 1 ->
   still 404 today; after this issue is fixed it is a 200 and the items are stored.

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GenerateQuery` — a Lake query generated from a prompt.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * `EventDataStores` carries "the ARN (or ID suffix of the ARN) of the event data
//!   store": a value that can be read as neither is refused with
//!   `EventDataStoreARNInvalidException` naming the ARN it addressed, and a readable one
//!   naming no store in the Region it addresses with `EventDataStoreNotFoundException`.
//!   Written WITHOUT an ARN the value has to BE an id this service could have minted (the
//!   shared `is_event_data_store_id` rule, a 36-character lower-case UUID), so a bare
//!   value of any other shape is unreadable rather than absent; an ARN's resource suffix
//!   is whatever store the ARN names, so for that form existence is the next question;
//! * a store is reachable only from the Region it lives in, whether it is named by a bare
//!   id (the caller's Region) or by an ARN (the ARN's Region);
//! * the stored `EventDataStoreStatus` decides whether the store can serve a query: a
//!   store awaiting deletion is `InactiveEventDataStoreException`, every other value of
//!   the catalog still serves queries;
//! * a request naming a store this emulator can serve is answered with the modelled
//!   `GenerateResponseException` — "thrown when a valid query could not be generated for
//!   the provided prompt" — because no generative model runs in this process; no reply
//!   ever carries a `QueryStatement` or a `QueryAlias`, generated or otherwise;
//! * `GenerateQuery` reports and writes nothing: however often it is called, the answer
//!   is the same and the registries are untouched.
//!
//! The not-found refusal answers HTTP 400 — the status CloudTrail answers with, which is
//! not the `@httpError(404)` its model annotates.
//!
//! [`a_store_created_over_the_wire_is_the_store_generate_query_reads`] makes its store
//! with `CreateEventDataStore` and retires it with `DeleteEventDataStore`, so the handler
//! is shown reading the same registry the real mutations write. The status table needs a
//! store in EVERY value of the model's `EventDataStoreStatus` catalog, and no operation
//! can put one into all six, so those rows seed that same registry through the service's
//! public `state` module — every assertion still goes over the wire.

use serde_json::json;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::EVENT_DATA_STORE_STATUS_VALUES;
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    event_data_store_arn, EventDataStoreRecord, EventDataStoreState,
    EVENT_DATA_STORE_NOT_FOUND_MESSAGE,
};

use super::emulator::{Emulator, Reply, HOME_REGION};

/// Another Region, for the cases that pin a store to the Region it lives in.
const OTHER_REGION: &str = "us-west-2";

/// The HTTP status this service answers a not-found refusal with: the model annotates
/// `EventDataStoreNotFoundException` `@httpError(404)`, and the recorded CloudTrail
/// response is 400.
const NOT_FOUND_STATUS: u16 = 400;

/// Every state an event data store can be in, paired with the wire value the model
/// publishes for it and the error code `GenerateQuery` answers a store in that state
/// with. The expected code is stated here, not derived from the production predicate, so
/// a change to `is_queryable` fails these rows instead of agreeing with them.
/// [`event_data_store_statuses_cover_the_model`] pins the pairing against the model's
/// own catalog.
const STORE_STATES: [(EventDataStoreState, &str, &str); 6] = [
    (
        EventDataStoreState::Created,
        "CREATED",
        "GenerateResponseException",
    ),
    (
        EventDataStoreState::Enabled,
        "ENABLED",
        "GenerateResponseException",
    ),
    // The one state that serves no query: a store waiting out its deletion period.
    (
        EventDataStoreState::PendingDeletion,
        "PENDING_DELETION",
        "InactiveEventDataStoreException",
    ),
    (
        EventDataStoreState::StartingIngestion,
        "STARTING_INGESTION",
        "GenerateResponseException",
    ),
    (
        EventDataStoreState::StoppingIngestion,
        "STOPPING_INGESTION",
        "GenerateResponseException",
    ),
    (
        EventDataStoreState::StoppedIngestion,
        "STOPPED_INGESTION",
        "GenerateResponseException",
    ),
];

/// Prompts spanning the modelled `Prompt` (len 3..500, pattern `^[ -~\n]*$`): the
/// shortest the model admits, a question, a statement, and one at the published maximum
/// length. The rules under test do not depend on the prompt, so each row takes one.
fn prompts() -> Vec<String> {
    vec![
        "Why".to_string(),
        "What are my top errors in the past month?".to_string(),
        "Give me a list of users that used SNS.".to_string(),
        "e".repeat(500),
    ]
}

/// An event data store id — the resource suffix of its ARN, in the UUID form CloudTrail
/// mints. `nth` varies the id without changing its form.
fn store_id(nth: u8) -> String {
    format!("{nth:02x}bbbbbb-5555-6666-7777-888888888888")
}

/// The ARN a refusal built from the request names for `identifier` called in `region`:
/// an ARN-shaped value as given, and a bare one completed into the ARN that id names in
/// the calling Region (the shared `event_data_store_arn_invalid_message` rule, which
/// every event data store operation refuses through).
fn addressed_arn(region: &str, identifier: &str) -> String {
    if identifier.contains(':') {
        identifier.to_string()
    } else {
        let id = identifier
            .strip_prefix("eventdatastore/")
            .unwrap_or(identifier);
        event_data_store_arn(region, id)
    }
}

/// Seed one event data store in a chosen status, the way `CreateEventDataStore` persists
/// one, and hand back its ARN.
fn seed_store(
    emulator: &Emulator,
    id: &str,
    region: &str,
    status: EventDataStoreState,
) -> String {
    emulator.state().lock().event_data_stores.insert(
        id.to_string(),
        EventDataStoreRecord {
            id: id.to_string(),
            name: format!("store-{id}"),
            region: region.to_string(),
            status,
            created_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            // The members this suite does not vary: the rules under test read the
            // identifier and the status, and `event_data_store_reads.rs` covers what a
            // read reports from the rest.
            ..EventDataStoreRecord::default()
        },
    );
    event_data_store_arn(region, id)
}

/// Every stored store as `(id, region, status)`, in registry order — what a case asserts
/// a reporting operation left alone.
fn stored_stores(emulator: &Emulator) -> Vec<(String, String, &'static str)> {
    emulator
        .state()
        .lock()
        .event_data_stores
        .values()
        .map(|record| {
            (
                record.id.clone(),
                record.region.clone(),
                record.status.as_wire(),
            )
        })
        .collect()
}

/// Call `GenerateQuery` from `region` for `identifier` with `prompt`.
async fn generate_query_in(
    emulator: &Emulator,
    region: &str,
    identifier: &str,
    prompt: &str,
) -> Reply {
    emulator
        .call_in(
            "GenerateQuery",
            region,
            &json!({"EventDataStores": [identifier], "Prompt": prompt}),
        )
        .await
}

/// Call `GenerateQuery` from [`HOME_REGION`].
async fn generate_query(emulator: &Emulator, identifier: &str, prompt: &str) -> Reply {
    generate_query_in(emulator, HOME_REGION, identifier, prompt).await
}

/// Assert `reply` carries no generated query. `GenerateQuery`'s output members are
/// `QueryStatement`, `QueryAlias` and `EventDataStoreOwnerAccountId`; the first two ARE
/// the generated query, and nothing may synthesise them.
fn carries_no_generated_query(reply: &Reply, case: &str) {
    for member in ["QueryStatement", "QueryAlias"] {
        assert!(
            reply.body.get(member).is_none(),
            "case {case}: no reply may carry a {member} this emulator did not generate, \
             got {reply:?}"
        );
    }
}

/// Assert `reply` carries a message within the bounds the modelled `ErrorMessage`
/// publishes (len 4..1000). The text is read through the shared harness, which owns the
/// `Message`/`message` casing rule.
fn carries_a_modelled_message(reply: &Reply, case: &str) {
    let length = reply.message().chars().count();
    assert!(
        (4..=1000).contains(&length),
        "case {case}: a refusal must carry an ErrorMessage of 4..1000 characters, got \
         {length} in {reply:?}"
    );
}

/// The state/wire pairing above is the model's own catalog, so a model that gains or
/// renames a status fails here rather than silently narrowing the rows below.
#[test]
fn event_data_store_statuses_cover_the_model() {
    let mut wire: Vec<&str> = STORE_STATES.iter().map(|(_, value, _)| *value).collect();
    wire.sort_unstable();
    assert_eq!(
        wire, EVENT_DATA_STORE_STATUS_VALUES,
        "case EventDataStoreStatus: expected the model's values \
         {EVENT_DATA_STORE_STATUS_VALUES:?}, got {wire:?}"
    );
    for (state, value, _) in STORE_STATES {
        assert_eq!(
            state.as_wire(),
            value,
            "case {value}: the state's wire value is wrong, got {:?}",
            state.as_wire()
        );
    }
}

/// A value that can be read as neither an event data store ARN nor an id can name no
/// store, so it is refused as an invalid ARN — whichever part of the form it breaks.
#[tokio::test]
async fn an_identifier_that_is_no_event_data_store_arn_is_refused() {
    let emulator = Emulator::new();
    let id = store_id(1);
    // Every row satisfies the modelled `EventDataStoreArn` member (len 3..256, pattern
    // `^[a-zA-Z0-9._/\-:]+$`), so each one reaches the handler rather than the boundary
    // validator.
    let rows: Vec<(&str, String)> = vec![
        // The bare-id forms: a value written without an ARN must be an id this service
        // could have minted, so one of any other shape is unreadable.
        (
            "a bare resource name",
            "my-event-data-store".to_string(),
        ),
        ("an id a character short", id[1..].to_string()),
        ("an id a character too long", format!("{id}a")),
        ("an upper-case id", id.to_uppercase()),
        ("the resource prefix alone", "eventdatastore/".to_string()),
        // The ARN forms.
        (
            "an ARN of another service",
            format!("arn:aws:logs:{HOME_REGION}:000000000000:eventdatastore/{id}"),
        ),
        (
            "an ARN naming another CloudTrail resource type",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:trail/{id}"),
        ),
        (
            "an ARN whose resource type only starts like this one's",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:eventdatastorex/{id}"),
        ),
        (
            "an ARN with no resource id",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:eventdatastore/"),
        ),
        (
            "an ARN with an empty account",
            format!("arn:aws:cloudtrail:{HOME_REGION}::eventdatastore/{id}"),
        ),
        (
            "an ARN with an empty Region",
            format!("arn:aws:cloudtrail::000000000000:eventdatastore/{id}"),
        ),
        (
            "an ARN with an empty partition",
            format!("arn::cloudtrail:{HOME_REGION}:000000000000:eventdatastore/{id}"),
        ),
        (
            "an ARN missing a field",
            format!("arn:aws:cloudtrail:{HOME_REGION}:eventdatastore/{id}"),
        ),
        (
            "an ARN carrying a field too many",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:aws:eventdatastore/{id}"),
        ),
    ];

    for (index, (case, identifier)) in rows.iter().enumerate() {
        let prompt = &prompts()[index % prompts().len()];
        let reply = generate_query(&emulator, identifier, prompt).await;
        reply.expect_error(case, 400, "EventDataStoreARNInvalidException");
        carries_no_generated_query(&reply, case);
        carries_a_modelled_message(&reply, case);
        // The refusal names the ARN the request addressed: an ARN-shaped value as given,
        // and a bare one completed into the ARN it names in the calling Region.
        let addressed = addressed_arn(HOME_REGION, identifier);
        assert!(
            reply.message().contains(&addressed),
            "case {case}: a refusal built from the request must name the ARN {addressed} \
             it addressed, got {reply:?}"
        );
    }
}

/// A readable identifier the registry does not hold is not found — whichever form it
/// takes, and whatever the prompt is.
#[tokio::test]
async fn an_identifier_naming_no_stored_store_is_not_found() {
    let emulator = Emulator::new();
    let id = store_id(2);
    let rows: Vec<(&str, String, &str)> = vec![
        ("a bare id", id.clone(), HOME_REGION),
        (
            "an ARN in the Region of the call",
            event_data_store_arn(HOME_REGION, &store_id(3)),
            HOME_REGION,
        ),
        (
            "an ARN in another Region",
            event_data_store_arn(OTHER_REGION, &store_id(4)),
            HOME_REGION,
        ),
        (
            "an ARN read from another Region",
            event_data_store_arn(HOME_REGION, &store_id(5)),
            OTHER_REGION,
        ),
        ("a bare id read from another Region", store_id(6), OTHER_REGION),
        (
            "a bare id carrying the resource prefix",
            format!("eventdatastore/{}", store_id(7)),
            HOME_REGION,
        ),
        // An ARN's resource suffix is the store the ARN names, so an ARN naming a store
        // of an id this service would never mint is still only a store nobody created.
        (
            "an ARN whose id is not a UUID",
            event_data_store_arn(HOME_REGION, "EXAMPLE_eds.1"),
            HOME_REGION,
        ),
    ];

    for (index, (case, identifier, region)) in rows.iter().enumerate() {
        let prompt = &prompts()[index % prompts().len()];
        let reply = generate_query_in(&emulator, region, identifier, prompt).await;
        reply.expect_error(case, NOT_FOUND_STATUS, "EventDataStoreNotFoundException");
        carries_no_generated_query(&reply, case);
        carries_a_modelled_message(&reply, case);
        // Every not-found refusal of this operation carries the service's own wording,
        // which names no identifier: `EventDataStores` holds exactly one store, so there
        // is only one thing the refusal can be about.
        assert_eq!(
            reply.message(),
            EVENT_DATA_STORE_NOT_FOUND_MESSAGE,
            "case {case}: the refusal text is wrong, got {reply:?}"
        );
    }

    assert!(
        stored_stores(&emulator).is_empty(),
        "case a refused GenerateQuery: nothing may be persisted, got {:?}",
        stored_stores(&emulator)
    );
}

/// The stored status decides whether a store can serve a query: every status in the
/// model's catalog serves one except the status a store awaiting deletion carries, and
/// both identifier forms address the same store.
#[tokio::test]
async fn the_stored_status_decides_whether_a_store_can_serve_a_query() {
    let emulator = Emulator::new();
    for (index, (status, wire, expected_code)) in STORE_STATES.iter().enumerate() {
        let id = store_id(10 + index as u8);
        let arn = seed_store(&emulator, &id, HOME_REGION, *status);
        let prompt = &prompts()[index % prompts().len()];

        for identifier in [id.clone(), arn.clone()] {
            let case = format!("a {wire} store addressed by {identifier}");
            let reply = generate_query(&emulator, &identifier, prompt).await;
            // Both outcomes are HTTP 400: `GenerateResponseException` says the store
            // could be queried but no statement could be generated for the prompt,
            // `InactiveEventDataStoreException` that the store serves no query at all.
            reply.expect_error(&case, 400, expected_code);
            carries_no_generated_query(&reply, &case);
            carries_a_modelled_message(&reply, &case);
        }
    }
}

/// A bare id addresses a store in the Region the call was made in; an ARN addresses the
/// store in the Region the ARN carries.
#[tokio::test]
async fn an_identifier_addresses_a_store_by_arn_region_or_by_the_callers_region() {
    let emulator = Emulator::new();
    let home_id = store_id(20);
    let other_id = store_id(21);
    let home_arn = seed_store(
        &emulator,
        &home_id,
        HOME_REGION,
        EventDataStoreState::Enabled,
    );
    let other_arn = seed_store(
        &emulator,
        &other_id,
        OTHER_REGION,
        EventDataStoreState::Enabled,
    );

    // (case, identifier, the Region the call is made in, the error code expected)
    let rows: Vec<(&str, String, &str, &str)> = vec![
        (
            "the home store by id, from its own Region",
            home_id.clone(),
            HOME_REGION,
            "GenerateResponseException",
        ),
        (
            "the home store by ARN, from its own Region",
            home_arn.clone(),
            HOME_REGION,
            "GenerateResponseException",
        ),
        (
            "the home store by ARN, from another Region",
            home_arn.clone(),
            OTHER_REGION,
            "GenerateResponseException",
        ),
        (
            "the home store by id, from another Region",
            home_id.clone(),
            OTHER_REGION,
            "EventDataStoreNotFoundException",
        ),
        (
            "the other store by ARN, from the home Region",
            other_arn.clone(),
            HOME_REGION,
            "GenerateResponseException",
        ),
        (
            "the other store by id, from its own Region",
            other_id.clone(),
            OTHER_REGION,
            "GenerateResponseException",
        ),
        (
            "the other store by id, from the home Region",
            other_id.clone(),
            HOME_REGION,
            "EventDataStoreNotFoundException",
        ),
    ];

    for (index, (case, identifier, region, code)) in rows.iter().enumerate() {
        let prompt = &prompts()[index % prompts().len()];
        let reply = generate_query_in(&emulator, region, identifier, prompt).await;
        let status = if *code == "EventDataStoreNotFoundException" {
            NOT_FOUND_STATUS
        } else {
            400
        };
        reply.expect_error(case, status, code);
        carries_no_generated_query(&reply, case);
    }
}

/// The registry `GenerateQuery` resolves against is the one the service's own mutations
/// write: a store made over the wire with `CreateEventDataStore` is addressable by either
/// identifier form, and once `DeleteEventDataStore` has moved it into its deletion period
/// the same store serves no query.
#[tokio::test]
async fn a_store_created_over_the_wire_is_the_store_generate_query_reads() {
    let emulator = Emulator::new();
    // The rows differ along the created store's name, which is what the creating call
    // varies; the identifier and the id it mints come back from the call itself.
    for (index, name) in ["lake-generate-query", "lake-generate-query-2"]
        .iter()
        .enumerate()
    {
        let created = emulator
            .call("CreateEventDataStore", &json!({"Name": name}))
            .await;
        let arn = created
            .expect_ok(&format!("CreateEventDataStore {name}"))
            .get("EventDataStoreArn")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        let id = arn.rsplit('/').next().unwrap_or_default().to_string();
        let prompt = &prompts()[index % prompts().len()];

        for identifier in [arn.clone(), id.clone()] {
            let case = format!("a created store addressed by {identifier}");
            let reply = generate_query(&emulator, &identifier, prompt).await;
            reply.expect_error(&case, 400, "GenerateResponseException");
            carries_no_generated_query(&reply, &case);
        }

        emulator
            .call("DeleteEventDataStore", &json!({"EventDataStore": arn}))
            .await
            .expect_ok(&format!("DeleteEventDataStore {name}"));

        let case = format!("a store awaiting deletion addressed by {id}");
        let reply = generate_query(&emulator, &id, prompt).await;
        reply.expect_error(&case, 400, "InactiveEventDataStoreException");
        carries_no_generated_query(&reply, &case);
    }
}

/// `GenerateQuery` reports: it answers the same way however often it is called, and it
/// leaves the registries exactly as it found them.
#[tokio::test]
async fn generate_query_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let id = store_id(30);
    let arn = seed_store(&emulator, &id, HOME_REGION, EventDataStoreState::Enabled);
    let before = stored_stores(&emulator);

    for (round, prompt) in prompts().iter().enumerate() {
        let case = format!("round {round}");
        generate_query(&emulator, &id, prompt)
            .await
            .expect_error(&case, 400, "GenerateResponseException");
        generate_query(&emulator, &arn, prompt)
            .await
            .expect_error(&format!("{case} by ARN"), 400, "GenerateResponseException");
        // A store nobody created stays uncreated, however often it is asked for.
        generate_query(&emulator, &store_id(31), prompt)
            .await
            .expect_error(
                &format!("{case}, an unknown store"),
                NOT_FOUND_STATUS,
                "EventDataStoreNotFoundException",
            );
        assert_eq!(
            stored_stores(&emulator),
            before,
            "case {case}: GenerateQuery must leave the registry untouched, got {:?}",
            stored_stores(&emulator)
        );
        assert!(
            emulator.state().lock().queries.is_empty(),
            "case {case}: GenerateQuery starts no query run, got {:?}",
            emulator.state().lock().queries.len()
        );
    }
}

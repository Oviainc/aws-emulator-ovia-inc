// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a start whose SOURCE this operation refuses leaves behind.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`],
//! the application a migration migrates into from [`super::data_source_attachments`] and
//! the two migration calls from [`super::saved_object_migrations`] and
//! [`super::migration_listing`]; this suite adds no second copy of any of them.
//!
//! What a saved-object migration exports out of is a search COLLECTION, so a
//! `migrationOptions.source.datasourceArn` outside that shape is refused —
//! [`super::constraint_refusal_messages`] owns the CLAUSE that refusal states, and this
//! suite owns its CONSEQUENCE: the refusal is per request, decided before any record is
//! minted, so each of several starts is refused on its own and no application is left
//! holding a migration a refused start would have created.

use serde_json::{json, Value};

use super::data_source_attachments::application;
use super::domain_lifecycle::{error_code, Emulator};
use super::migration_listing::list_migrations;
use super::saved_object_migrations::start_migration;

/// The sources the cases below are refused for, two forms outside the collection shape:
/// an ARN of another service naming another resource, and a value that is no ARN at all.
///
/// Two, not a table of every form — which forms the operation refuses, and the clause it
/// states for each, belong to [`super::constraint_refusal_messages`]; what is asserted
/// here needs only a source the operation refuses, and two of them so the concurrent
/// case's two answers cannot be each other's.
const REFUSED_SOURCES: [&str; 2] = [
    "arn:aws:es:us-east-1:123456789012:domain/savedobjects",
    "saved-objects-source-collection",
];

/// The request body of a start of `application_id` out of `datasource_arn`, into a
/// workspace the migration is asked to create — the target every case below shares, so
/// what differs between them is only the source.
fn start_body(application_id: &str, datasource_arn: &str) -> Value {
    json!({
        "applicationId": application_id,
        "migrationOptions": {
            "source": { "datasourceArn": datasource_arn },
            "workspace": { "createWorkspace": true, "name": "Imported dashboards" },
        },
    })
}

/// The migrations `application_id` holds, as `ListMigrations` reports them.
async fn listed(emulator: &Emulator, application_id: &str, case: &str) -> Vec<Value> {
    let answer = list_migrations(emulator, application_id, "").await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from ListMigrations, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["migrations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a migrations list, got {:?}", answer.body))
        .clone()
}

/// RULE: a start whose source is outside the shape this operation accepts is refused with
/// the modelled `ValidationException` before any record is minted, so the application it
/// named reports no migration afterwards — the observation a client makes, through
/// `ListMigrations`, rather than through the store.
#[tokio::test]
async fn a_start_whose_source_is_refused_mints_nothing() {
    let emulator = Emulator::new();
    let id = application(&emulator, "source-refused").await;

    for source in REFUSED_SOURCES {
        let answer = start_migration(&emulator, start_body(&id, source)).await;
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {source:?}: expected 400 ValidationException, got {} with {:?}",
            answer.status,
            answer.body
        );
    }

    let held = listed(&emulator, &id, "after the refusals").await;
    assert!(
        held.is_empty(),
        "case after the refusals: expected the application to hold no migration, got {held:?}"
    );
    assert!(
        emulator.store().lock().migrations.is_empty(),
        "case after the refusals: expected a refused start to persist nothing, got {:?}",
        emulator.store().lock().migrations.keys().collect::<Vec<_>>()
    );
}

/// RULE: the refusal is decided per REQUEST — two starts issued together, each naming its
/// own application, are each refused on their own, and neither application is left
/// holding a migration.
#[tokio::test]
async fn concurrent_starts_on_independent_applications_are_each_refused() {
    let emulator = Emulator::new();
    let one = application(&emulator, "source-concurrent-one").await;
    let other = application(&emulator, "source-concurrent-other").await;
    // Two different refused forms, so neither answer can be the other's.
    let (one_source, other_source) = (REFUSED_SOURCES[0], REFUSED_SOURCES[1]);

    let (first, second) = tokio::join!(
        start_migration(&emulator, start_body(&one, one_source)),
        start_migration(&emulator, start_body(&other, other_source)),
    );

    for (case, answer, application_id) in [
        ("the first of two concurrent starts", first, &one),
        ("the second of two concurrent starts", second, &other),
    ] {
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} with {:?}",
            answer.status,
            answer.body
        );
        let held = listed(&emulator, application_id, case).await;
        assert!(
            held.is_empty(),
            "case {case:?}: expected the application to hold no migration, got {held:?}"
        );
    }
}

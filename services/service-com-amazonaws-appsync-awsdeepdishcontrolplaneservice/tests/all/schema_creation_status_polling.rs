// SPDX-License-Identifier: CC0-1.0

//! `GetSchemaCreationStatus` as a REPORTING call: what it answers comes from the stored
//! creation, and reading it changes nothing a later caller sees.
//!
//! The rule asserted here is stated over a CLASS of inputs rather than over one request:
//! every read of an API's schema creation status reports the state the creation stored —
//! `NOT_APPLICABLE` where none has run, the non-terminal state while the clock still owes
//! one, and the terminal state and detail the creation settled to afterwards — so any
//! number of reads at one instant answer identically, a read neither completes nor
//! releases the creation in flight (the second modification is still refused), and the
//! types the creation installed are the same however often the status was polled.
//!
//! The table's rows differ along what the rule generalises over: whether a creation ran
//! at all, how many types the document declares, whether the document defines a type
//! system the API can hold, whether the payload is text at all, and how many times the
//! status is read at each stage.
//!
//! The harness is the one the association and schema suites already carry — the router
//! and `create_api` from `tests/all/source_api_association.rs`, and the schema calls
//! (`start_creation`, `creation_status`, `get_type`), the span slack and the
//! `SchemaStatus` catalog from `tests/all/schema_creation.rs` — so a schema request is
//! built exactly one way across this crate and every assertion goes through the router
//! the production emulator mounts.

use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;

use crate::schema_creation::{
    creation_status, get_type, start_creation, PAST_THE_CREATION, SCHEMA_STATUSES,
};
use crate::source_api_association::{create_api, router, Answer};

/// One row of the polling table.
#[derive(Debug, Clone)]
struct Row {
    label: &'static str,
    /// The document the creation carries, or `None` for an API no creation runs for.
    definition: Option<&'static [u8]>,
    /// How many further reads are taken at each stage, after the first one they are all
    /// held to.
    rereads: usize,
    /// The status every read owes once the clock has performed what it owed.
    terminal: &'static str,
    /// A type the document installs, which must still be readable however often the
    /// status was polled.
    installed: Option<&'static str>,
}

/// The cases the rule is asserted over: no creation at all, documents that define a type
/// system the API can hold (one type and several), a document GraphQL cannot parse, and a
/// payload that is not text — each read a different number of times.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "no creation has ever run",
            definition: None,
            rereads: 3,
            terminal: state::SCHEMA_NOT_APPLICABLE,
            installed: None,
        },
        Row {
            label: "a document declaring one type, read once more",
            definition: Some(b"type Query {\n  hello: String\n}\n"),
            rereads: 1,
            terminal: state::SCHEMA_SUCCESS,
            installed: Some("Query"),
        },
        Row {
            label: "a document declaring several types, read repeatedly",
            definition: Some(
                b"type Query {\n  post: Post\n}\n\ntype Post {\n  id: ID!\n  title: String\n}\n",
            ),
            rereads: 5,
            terminal: state::SCHEMA_SUCCESS,
            installed: Some("Post"),
        },
        Row {
            label: "a document GraphQL cannot parse, read repeatedly",
            definition: Some(b"type Query {\n  hello: String\n"),
            rereads: 4,
            terminal: state::SCHEMA_FAILED,
            installed: None,
        },
        Row {
            label: "a payload that is not text, read once more",
            definition: Some(&[0xff, 0xfe, 0x00, 0x01]),
            rereads: 2,
            terminal: state::SCHEMA_FAILED,
            installed: None,
        },
    ]
}

/// Assert `answer` is a 200 whose `status` is a value the model declares.
fn assert_modelled(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetSchemaCreationStatus 200, got {} {:?}",
        answer.status, answer.body
    );
    let status = answer.body["status"].as_str().unwrap_or_default();
    assert!(
        SCHEMA_STATUSES.contains(&status),
        "case {case:?}: expected a modelled SchemaStatus out of {SCHEMA_STATUSES:?}, got \
         {status:?}"
    );
}

#[tokio::test]
async fn polling_the_creation_status_reports_the_stored_state_and_changes_nothing() {
    let router = router();
    let rows = rows();
    let mut apis: Vec<String> = Vec::new();

    // Every row's creation is scheduled first, each on its own API, so the ONE wait
    // further down crosses the due instant of all of them: the transition is owed by the
    // clock rather than by a call.
    for row in &rows {
        let api = create_api(&router, row.label, state::GRAPHQL_API_TYPE).await;
        if let Some(definition) = row.definition {
            let started = start_creation(&router, &api.id, definition).await;
            assert_eq!(
                started.status, 200,
                "case {:?}: expected StartSchemaCreation 200, got {} {:?}",
                row.label, started.status, started.body
            );
        }

        // The FIRST read is what every later one is held to: a read that moved the state
        // on would show up as a difference between them.
        let first = creation_status(&router, &api.id).await;
        assert_modelled(&first, row.label);
        let awaited = if row.definition.is_some() {
            state::SCHEMA_PROCESSING
        } else {
            state::SCHEMA_NOT_APPLICABLE
        };
        assert_eq!(
            first.body["status"].as_str(),
            Some(awaited),
            "case {:?}: expected the stored state to be reported as {awaited:?}, got {:?}",
            row.label,
            first.body
        );
        for reread in 1..=row.rereads {
            let again = creation_status(&router, &api.id).await;
            assert_modelled(&again, row.label);
            assert_eq!(
                again.body, first.body,
                "case {:?} reread {reread}: expected reading the status to leave the same report \
                 for the next caller as {:?}, got {:?}",
                row.label, first.body, again.body
            );
        }

        // Reading is not what performs the creation, so a creation the clock still owes
        // is still in progress after any number of reads: the next modification of this
        // API's type system is refused rather than let through.
        if row.definition.is_some() {
            let second =
                start_creation(&router, &api.id, b"type Query {\n  other: String\n}\n").await;
            assert_eq!(
                (second.status, second.error_type.as_str()),
                (409, "ConcurrentModificationException"),
                "case {:?}: expected the creation to still be in progress after reading it, got \
                 {} {} {:?}",
                row.label,
                second.status,
                second.error_type,
                second.body
            );
        }

        apis.push(api.id);
    }

    // Nothing is called in between: the transitions below are owed by the clock, not by
    // the reads above.
    tokio::time::sleep(state::SCHEMA_CREATION_SPAN + PAST_THE_CREATION).await;

    for (row, api_id) in rows.iter().zip(&apis) {
        let settled = creation_status(&router, api_id).await;
        assert_modelled(&settled, row.label);
        assert_eq!(
            settled.body["status"].as_str(),
            Some(row.terminal),
            "case {:?}: expected the state past the due instant to be reported as {:?}, got {:?}",
            row.label,
            row.terminal,
            settled.body
        );

        // A creation that ran reports what became of it; an API none ran for has nothing
        // to detail.
        let detail = settled.body["details"].as_str();
        if row.definition.is_some() {
            assert!(
                detail.is_some_and(|text| {
                    !text.is_empty() && text != state::SCHEMA_PROCESSING_DETAIL
                }),
                "case {:?}: expected the settled creation to detail its outcome, got {:?}",
                row.label,
                settled.body
            );
        } else {
            assert!(
                detail.is_none(),
                "case {:?}: expected no detail where no creation has run, got {:?}",
                row.label,
                settled.body
            );
        }

        for reread in 1..=row.rereads {
            let again = creation_status(&router, api_id).await;
            assert_eq!(
                again.body, settled.body,
                "case {:?} reread {reread} past the due instant: expected the settled report to \
                 be unchanged by reading it, got {:?}",
                row.label, again.body
            );
        }

        // Polling left what the creation installed exactly where it was.
        if let Some(name) = row.installed {
            let reported = get_type(&router, api_id, name, state::SDL_FORMAT).await;
            assert_eq!(
                reported.status, 200,
                "case {:?} type {name:?}: expected the created type to still be readable, got {} \
                 {:?}",
                row.label, reported.status, reported.body
            );
        }

        // The clock, not a read, is what released the creation: a modification is
        // accepted again now that the one it owed has been performed.
        if row.definition.is_some() {
            let next =
                start_creation(&router, api_id, b"type Query {\n  hello: String\n}\n").await;
            assert_eq!(
                (next.status, next.body["status"].as_str()),
                (200, Some(state::SCHEMA_PROCESSING)),
                "case {:?}: expected a further creation to be accepted once the owed one was \
                 performed, got {} {:?}",
                row.label,
                next.status,
                next.body
            );
        }
    }
}

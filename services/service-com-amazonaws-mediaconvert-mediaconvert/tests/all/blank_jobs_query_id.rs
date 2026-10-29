// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the rule for the `{Id}` that addresses a JOBS QUERY.
//!
//! `GetJobsQueryResults$Id` targets the model's bare `__string`
//! (`smithy_query(query="operation",operation="GetJob,GetJobsQueryResults",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`
//! reports `{"target": "__string", "httpLabel": true, "required": true}` for both): no
//! pattern and no length, so the framework's boundary validation passes the zero-length
//! value and a value made only of whitespace alike, exactly as it does for the `{Id}` of a
//! job.
//!
//! The rule under test: a caller-sent member that names a resource this service holds must
//! carry at least one character, so a value made only of whitespace is answered with
//! `BadRequestException` (400) naming the member it arrived in, never with a lookup of a
//! value nothing could hold. The complement is pinned beside it: a value that DOES carry
//! characters is resolved by the lookup byte for byte, so an id no query is stored under —
//! a stored id padded with whitespace among them — stays the modelled `NotFoundException`
//! (404), and the minted id itself reports the query.
//!
//! `tests/all/blank_job_reference.rs` drives the same rule through the doors that resolve a
//! JOB reference and `tests/all/blank_resource_name.rs` through the doors that mint and
//! reference a NAMED resource; this suite reuses their blank forms and their refusal
//! assertion, so none of them can assert a different sentence for the same class of value.
//!
//! Each case drives the real restJson1 wire path through the crate's one test harness (the
//! router, the SigV4 scope and the reply decoding `job_lifecycle` exports), so what is
//! asserted is the bytes a client would see.

use std::time::Duration;

use serde_json::json;
use service_com_amazonaws_mediaconvert_mediaconvert::state::not_found_message;

use super::blank_resource_name::{assert_blank_refusal, blanks};
use super::job_lifecycle::{Emulator, Reply};
use super::jobs_query::pin_query_span;
use super::resource_tagging::encode_label;

/// Longer than any case here takes to run, so the fixture query stays in its non-terminal
/// `SUBMITTED` status throughout and a reply read twice is the same reply. Pinned rather
/// than raced: the span is state, so no case depends on how fast the machine is.
const HELD_IN_SUBMITTED: Duration = Duration::from_secs(3_600);

/// Read a jobs query's results through the `{Id}` label of
/// `/2017-08-29/jobsQueries/{Id}`, with `encoded` already percent-encoded for one path
/// segment.
async fn results_for_label(emulator: &Emulator, encoded: &str) -> Reply {
    emulator
        .send("GET", &format!("/2017-08-29/jobsQueries/{encoded}"), None)
        .await
}

/// A jobs query started through the real `StartJobsQuery` door. Returns the id it minted.
async fn fixture_query(emulator: &Emulator) -> String {
    let started = emulator
        .send("POST", "/2017-08-29/jobsQueries", Some(json!({})))
        .await;
    assert_eq!(
        started.status, 201,
        "the fixture query must be started, got {} with body {}",
        started.status, started.body
    );
    let id = started.string("/id");
    assert!(
        !id.is_empty(),
        "the fixture query must carry an id, got {}",
        started.body
    );
    id
}

/// The `{Id}` of a jobs query takes the same test every resource-naming member in this
/// service takes: each blank form is the modelled bad request naming the member it arrived
/// in, and the query the account does hold reads back exactly as it did before.
#[tokio::test]
async fn a_jobs_query_id_carrying_no_characters_is_refused() {
    let emulator = Emulator::new();
    pin_query_span(&emulator, HELD_IN_SUBMITTED);
    // A query the refusals must not touch: it is the one query this account holds, so a
    // door that resolved a blank id to "the query that is there" would be caught too.
    let id = fixture_query(&emulator).await;
    let before = results_for_label(&emulator, &encode_label(&id)).await;
    assert_eq!(
        before.status, 200,
        "the fixture query must be readable, got {} with body {}",
        before.status, before.body
    );

    for blank in blanks() {
        // The zero-length value is no path label at all — `/2017-08-29/jobsQueries/`
        // addresses the collection, not a query — so it is not a row of this door.
        if blank.value.is_empty() {
            continue;
        }
        let case = format!("GetJobsQueryResults with an id of {}", blank.label);

        // The encoded form each blank carries is the one the named-resource suite drives
        // its own path labels with, so neither suite encodes it its own way.
        let refused = results_for_label(&emulator, blank.path).await;
        assert_blank_refusal(&case, "Id", &refused);

        let after = results_for_label(&emulator, &encode_label(&id)).await;
        assert_eq!(
            after.body, before.body,
            "case {case:?}: a refused id must leave the stored query's report untouched, \
             got {}",
            after.body
        );
    }
}

/// The boundary of the same rule: an id that carries characters is resolved by the lookup,
/// byte for byte. So an id no query is stored under — however it is shaped, and including
/// the minted id with whitespace around it, which is a DIFFERENT value because a value
/// carrying characters is never trimmed — is the member-naming `NotFoundException`, while
/// the minted id reports the query's status.
#[tokio::test]
async fn a_jobs_query_id_that_carries_characters_is_resolved_by_the_lookup() {
    let emulator = Emulator::new();
    pin_query_span(&emulator, HELD_IN_SUBMITTED);
    let id = fixture_query(&emulator).await;

    // Rows differing along what an id can be: how long it is, whether it has the shape of
    // a minted id (epoch milliseconds, a hyphen, a base-36 suffix — `state::mint_id`), and
    // whether it is the minted id with whitespace on one or both sides.
    for (label, probe) in [
        ("a one-character id no query holds", "x".to_string()),
        ("a short label no query holds", "nosuchquery".to_string()),
        (
            "a minted-looking id no query holds",
            "1700000000000-zzzzzz".to_string(),
        ),
        (
            "an id made of the minted id and one more character",
            format!("{id}0"),
        ),
        ("the minted id with a leading space", format!(" {id}")),
        ("the minted id with a trailing newline", format!("{id}\n")),
        (
            "the minted id with whitespace on both sides",
            format!(" {id} "),
        ),
    ] {
        let case = format!("GetJobsQueryResults with {label}");
        let missed = results_for_label(&emulator, &encode_label(&probe)).await;
        assert_eq!(
            missed.status, 404,
            "case {case:?}: expected HTTP 404, got {} with body {}",
            missed.status, missed.body
        );
        assert_eq!(
            missed.error_type.as_deref(),
            Some("NotFoundException"),
            "case {case:?}: expected NotFoundException, got {:?} with body {}",
            missed.error_type,
            missed.body
        );
        assert_eq!(
            missed.string("/message"),
            not_found_message("id"),
            "case {case:?}: expected the lookup refusal to name id, got body {}",
            missed.body
        );
    }

    let case = "GetJobsQueryResults with the minted id";
    let resolved = results_for_label(&emulator, &encode_label(&id)).await;
    assert_eq!(
        resolved.status, 200,
        "case {case:?}: expected HTTP 200, got {} with body {}",
        resolved.status, resolved.body
    );
    assert_eq!(
        resolved.error_type, None,
        "case {case:?}: expected no error, got {:?} with body {}",
        resolved.error_type, resolved.body
    );
    assert_eq!(
        resolved.string("/status"),
        "SUBMITTED",
        "case {case:?}: a query held before its due instant must report SUBMITTED, got {}",
        resolved.body
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the rule for a caller-sent member that names a JOB.
//!
//! Every such member targets the model's bare `__string`
//! (`smithy_query(query="operation",operation="CancelJob,GetJob,CreateResourceShare",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`):
//! no pattern and no length, so the framework's boundary validation passes the zero-length
//! value and a value made only of whitespace alike, whether the member arrives as the
//! `{Id}` path label of `/2017-08-29/jobs/{Id}` or as the `JobId` of a request body.
//!
//! The rule under test: a value with no characters carries no job identity — it is neither
//! an id nor an ARN, so no job could ever be stored under it — so every door that resolves
//! a job reference answers it with `BadRequestException` (400) naming the member it arrived
//! in, and never with a lookup of a value nothing could hold. The complement of the rule is
//! pinned beside it: a reference that DOES carry characters is resolved by the lookup, so a
//! reference naming no stored job stays the modelled `NotFoundException` (404) and a stored
//! id padded with whitespace is a different value, not the job's.
//!
//! `tests/all/blank_resource_name.rs` drives the same rule through the doors that mint and
//! reference a NAMED resource, and this suite reuses its blank forms and its refusal
//! assertion, so the two cannot assert different sentences for the same class of value.
//!
//! Each case drives the real restJson1 wire path through the crate's one test harness (the
//! router, the SigV4 scope and the reply decoding `job_lifecycle` exports), so what is
//! asserted is the bytes a client would see.

use serde_json::json;
use service_com_amazonaws_mediaconvert_mediaconvert::state::not_found_message;

use super::blank_resource_name::{assert_blank_refusal, blanks};
use super::job_lifecycle::{settings, Emulator, Reply, ROLE};
use super::resource_tagging::encode_label;

/// Where a job reference arrives, which is what the rule generalises over: the position of
/// the member, not its value.
enum Arrival {
    /// The `{Id}` label of `/2017-08-29/jobs/{Id}`, reached with this HTTP method.
    PathLabel(&'static str),
    /// `CreateResourceShare$JobId`, the body member on the same reference.
    BodyMember,
}

/// One door that resolves a job reference.
struct JobDoor {
    /// The operation, for the case label.
    label: &'static str,
    arrival: Arrival,
    /// The member the refusal names, exactly as the model spells it.
    member: &'static str,
    /// The member a lookup refusal names, as this service words that sentence.
    looked_up: &'static str,
    /// The status a request naming a stored job answers with.
    success_status: u16,
}

/// Every door in this service that resolves a job reference: the two that read `{Id}` from
/// the path and the one that reads `JobId` from the body.
fn doors() -> Vec<JobDoor> {
    vec![
        JobDoor {
            label: "CancelJob",
            arrival: Arrival::PathLabel("DELETE"),
            member: "Id",
            looked_up: "id",
            success_status: 202,
        },
        JobDoor {
            label: "GetJob",
            arrival: Arrival::PathLabel("GET"),
            member: "Id",
            looked_up: "id",
            success_status: 200,
        },
        JobDoor {
            label: "CreateResourceShare",
            arrival: Arrival::BodyMember,
            member: "JobId",
            looked_up: "jobId",
            success_status: 202,
        },
    ]
}

/// Send a job reference to `door`: `reference` as the caller wrote it, for the door that
/// carries it in the body, and `encoded` — the same value percent-encoded for one path
/// segment — for the doors that read it from the path.
async fn send_reference(
    emulator: &Emulator,
    door: &JobDoor,
    reference: &str,
    encoded: &str,
) -> Reply {
    match door.arrival {
        Arrival::PathLabel(method) => {
            emulator
                .send(method, &format!("/2017-08-29/jobs/{encoded}"), None)
                .await
        }
        Arrival::BodyMember => {
            emulator
                .send(
                    "POST",
                    "/2017-08-29/resourceShares",
                    // The support case is valid in every row, so the only thing wrong with
                    // a refused one is the job reference itself.
                    Some(json!({ "jobId": reference, "supportCaseId": "case-4412" })),
                )
                .await
        }
    }
}

/// A stored job created through the real `CreateJob` path. Returns its `Id`.
async fn fixture_job(emulator: &Emulator, name_modifier: &str) -> String {
    let created = emulator
        .create_job(json!({
            "role": ROLE,
            "settings": settings("s3://example-bucket/in/a.mp4", name_modifier)
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture job must be created, got {} with body {}",
        created.status, created.body
    );
    let id = created.string("/job/id");
    assert!(
        !id.is_empty(),
        "the fixture job must carry an id, got {}",
        created.body
    );
    id
}

/// No door that resolves a job reference looks up a value carrying no characters: each
/// blank form is the modelled bad request naming the member it arrived in, and the job the
/// account does hold is left exactly as it was.
#[tokio::test]
async fn a_job_reference_carrying_no_characters_is_refused_by_every_job_door() {
    let emulator = Emulator::new();
    // A job the refusals must not touch: it is the one job this account holds, so a door
    // that resolved a blank reference to "the job that is there" would be caught too.
    let id = fixture_job(&emulator, "_untouched").await;
    let before = emulator.get_job(&id).await.at("/job").clone();

    for door in doors() {
        for blank in blanks() {
            // The zero-length value is no path label at all — `/2017-08-29/jobs/` addresses
            // the collection, not a job — so it is sent to the door that carries the
            // reference in the body.
            if blank.value.is_empty() && matches!(door.arrival, Arrival::PathLabel(_)) {
                continue;
            }
            let case = format!("{} with a job reference of {}", door.label, blank.label);

            // The encoded form each blank carries is the one the named-resource suite
            // drives its own path labels with, so neither suite encodes it its own way.
            let refused = send_reference(&emulator, &door, blank.value, blank.path).await;
            assert_blank_refusal(&case, door.member, &refused);

            let after = emulator.get_job(&id).await;
            assert_eq!(
                after.at("/job"),
                &before,
                "case {case:?}: a refused reference must leave the stored job untouched, \
                 got {}",
                after.body
            );
        }
    }
}

/// The boundary of the same rule: a reference that carries characters is resolved by the
/// lookup, byte for byte. So one naming no stored job is still the member-naming
/// `NotFoundException`, a stored id padded with whitespace names no job, and the stored id
/// itself answers the operation's own success status.
#[tokio::test]
async fn a_job_reference_that_carries_characters_is_resolved_by_the_lookup() {
    let emulator = Emulator::new();

    for (index, door) in doors().into_iter().enumerate() {
        // A job of this door's own, so the success row of one door cannot change what the
        // next door resolves.
        let id = fixture_job(&emulator, &format!("_resolved{index}")).await;

        // Rows differing along what a reference can be: how long it is, whether it has the
        // shape an id has, and whether it is a stored id with whitespace around it — a
        // padded id is a DIFFERENT value, because a value that carries characters is never
        // trimmed.
        for (label, reference) in [
            ("a short label no job holds", "nosuchjob".to_string()),
            (
                "a well-formed id no job holds",
                "1700000000000-zzzzzz".to_string(),
            ),
            ("a stored id with a leading space", format!(" {id}")),
            ("a stored id with a trailing newline", format!("{id}\n")),
            (
                "a stored id with whitespace on both sides",
                format!(" {id} "),
            ),
        ] {
            let case = format!("{} with {label}", door.label);
            let missed =
                send_reference(&emulator, &door, &reference, &encode_label(&reference)).await;
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
                not_found_message(door.looked_up),
                "case {case:?}: expected the lookup refusal to name {}, got body {}",
                door.looked_up,
                missed.body
            );
        }

        let case = format!("{} with the stored id", door.label);
        let resolved = send_reference(&emulator, &door, &id, &encode_label(&id)).await;
        assert_eq!(
            resolved.status, door.success_status,
            "case {case:?}: expected HTTP {}, got {} with body {}",
            door.success_status, resolved.status, resolved.body
        );
        assert_eq!(
            resolved.error_type, None,
            "case {case:?}: expected no error, got {:?} with body {}",
            resolved.error_type, resolved.body
        );
    }
}

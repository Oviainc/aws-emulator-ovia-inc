// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the rule that a caller-sent KEY is used, stored and echoed
//! byte for byte.
//!
//! `CreateJob$ClientRequestToken` targets the bare `__string` and is documented as "any
//! string that includes up to 64 ASCII characters"
//! (`smithy_query(query="doc",shape="CreateJobRequest",member="ClientRequestToken")`): the
//! model states no pattern, no length and no normalisation, so the token a caller sends is
//! the key of the idempotency window AND the value `Job$ClientRequestToken` reports. Two
//! tokens that differ only by the whitespace around them are therefore two tokens naming
//! two jobs, and the only normalisation any such member gets is the one every key in this
//! crate gets: a value carrying no characters at all names nothing.
//!
//! The same rule governs a REFERENCE to a key — `CreateJobTemplate$Queue` and
//! `UpdateJobTemplate$Queue` name a queue resource, whose own `Name` is stored verbatim
//! (`queue_name_identity` pins that), so the reference resolves the string the caller sent
//! rather than a neighbouring one.
//!
//! Every case drives the real restJson1 wire path through the router (the harness is the
//! one `job_lifecycle` builds), so what is asserted is the bytes a client would see.

use serde_json::json;

use crate::job_lifecycle::{queue_arn, settings, Emulator, ROLE};
use crate::job_template_crud::settings as template_settings;

/// One caller-sent key value: its label, and the bytes the request carries.
struct Case {
    label: &'static str,
    value: &'static str,
}

/// The rows span the forms of whitespace a bare `__string` key can carry — which is what
/// the rule generalises over — plus one value carrying none at all as the control, and one
/// at the length the token's documentation names.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "no surrounding whitespace at all",
            value: "plain",
        },
        Case {
            label: "a trailing space",
            value: "trail ",
        },
        Case {
            label: "a leading space",
            value: " lead",
        },
        Case {
            label: "spaces on both sides",
            value: "  both  ",
        },
        Case {
            label: "a trailing tab",
            value: "tabbed\t",
        },
        Case {
            label: "a leading newline",
            value: "\nlined",
        },
        Case {
            label: "whitespace inside the value",
            value: "two words",
        },
        // 64 characters, the length `ClientRequestToken` documents, with the last one a
        // space: the padding is still part of the token at the published bound.
        Case {
            label: "64 characters, the last a space",
            value: "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx ",
        },
    ]
}

/// The token a `CreateJob` was given is the token it keys the idempotency window by and the
/// token the job echoes: repeating that exact string returns the job it first created, and
/// `GetJob` reports the same bytes the request carried.
#[tokio::test]
async fn a_client_request_token_is_keyed_and_echoed_exactly_as_sent() {
    let emulator = Emulator::new();

    for Case { label, value } in cases() {
        assert_eq!(
            value.len(),
            value.chars().count(),
            "case {label:?}: the row must be ASCII, got {value:?}"
        );

        let request = json!({
            "role": ROLE,
            "clientRequestToken": value,
            "settings": settings("s3://example-bucket/in/a.mp4", "_a")
        });
        let created = emulator.create_job(request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        assert_eq!(
            created.string("/job/clientRequestToken"),
            value,
            "case {label:?}: expected the token echoed verbatim, got {}",
            created.body
        );

        // The stored job reports the same bytes, so what the create echoed came out of the
        // record rather than out of the request.
        let id = created.string("/job/id");
        let fetched = emulator.get_job(&id).await;
        assert_eq!(
            fetched.status, 200,
            "case {label:?}: expected the created job to read back, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.string("/job/clientRequestToken"),
            value,
            "case {label:?}: expected the stored token verbatim, got {}",
            fetched.body
        );

        // That exact string, inside the window, names the job it first created.
        let repeat = emulator.create_job(request).await;
        assert_eq!(
            repeat.at("/job"),
            created.at("/job"),
            "case {label:?}: the same token must return the same job, got {}",
            repeat.body
        );
    }
}

/// Because the token is never normalised, a token differing only by the whitespace around
/// it is a DIFFERENT token: it creates a second job, and each job echoes its own token.
#[tokio::test]
async fn a_token_differing_only_by_surrounding_whitespace_is_a_different_job() {
    let emulator = Emulator::new();

    for Case { label, value } in cases() {
        let neighbour = value.trim();
        if neighbour == value {
            continue;
        }

        let mut ids = Vec::new();
        for token in [value, neighbour] {
            let created = emulator
                .create_job(json!({
                    "role": ROLE,
                    "clientRequestToken": token,
                    "settings": settings("s3://example-bucket/in/a.mp4", "_a")
                }))
                .await;
            assert_eq!(
                created.status, 201,
                "case {label:?}: expected HTTP 201 for token {token:?}, got {} with body {}",
                created.status, created.body
            );
            assert_eq!(
                created.string("/job/clientRequestToken"),
                token,
                "case {label:?}: expected token {token:?} echoed verbatim, got {}",
                created.body
            );
            ids.push(created.string("/job/id"));
        }

        assert_ne!(
            ids[0], ids[1],
            "case {label:?}: tokens {value:?} and {neighbour:?} must create two jobs, got \
             {ids:?}"
        );
        // Both jobs are in the store, each still reporting the token that created it.
        for (id, token) in ids.iter().zip([value, neighbour]) {
            let fetched = emulator.get_job(id).await;
            assert_eq!(
                fetched.string("/job/clientRequestToken"),
                token,
                "case {label:?}: expected job {id} to keep token {token:?}, got {}",
                fetched.body
            );
        }
    }
}

/// The one token value that keys nothing is one carrying no characters at all: it is the
/// same as a request that sent no token, so each such create mints its own job and the job
/// reports no token.
#[tokio::test]
async fn a_token_carrying_no_characters_keys_nothing() {
    let emulator = Emulator::new();

    let mut ids = Vec::new();
    for blank in ["", " ", "\t", "\n", "   "] {
        for attempt in 1..=2 {
            let created = emulator
                .create_job(json!({
                    "role": ROLE,
                    "clientRequestToken": blank,
                    "settings": settings("s3://example-bucket/in/a.mp4", "_a")
                }))
                .await;
            assert_eq!(
                created.status, 201,
                "case {blank:?} attempt {attempt}: expected HTTP 201, got {} with body {}",
                created.status, created.body
            );
            assert!(
                created.at("/job/clientRequestToken").is_null(),
                "case {blank:?} attempt {attempt}: a token carrying no characters must not \
                 be echoed, got {}",
                created.body
            );
            ids.push(created.string("/job/id"));
        }
    }

    let distinct = {
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        sorted.len()
    };
    assert_eq!(
        distinct,
        ids.len(),
        "every create carrying no token must mint its own job, got {ids:?}"
    );
}

/// A job template's `Queue` is a REFERENCE to a queue whose `Name` is stored verbatim, so
/// it resolves the string the caller sent: the padded name is accepted and stored, and the
/// neighbouring form — a queue nobody created — is the modelled 404.
#[tokio::test]
async fn a_job_template_queue_reference_resolves_the_name_exactly_as_sent() {
    let emulator = Emulator::new();

    for (index, Case { label, value }) in cases().into_iter().enumerate() {
        let created_queue = emulator.create_queue(json!({ "name": value })).await;
        assert_eq!(
            created_queue.status, 201,
            "case {label:?}: the fixture queue must be created, got {} with body {}",
            created_queue.status, created_queue.body
        );

        let name = format!("template-{index}");
        let created = emulator
            .create_job_template(json!({
                "name": &name,
                "queue": value,
                "settings": template_settings(1, 1)
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: expected HTTP 201 from CreateJobTemplate, got {} with body {}",
            created.status, created.body
        );
        assert_eq!(
            created.string("/jobTemplate/queue"),
            queue_arn(value),
            "case {label:?}: expected the ARN of the queue the reference resolved, got {}",
            created.body
        );
        let fetched = emulator.get_job_template(&name).await;
        assert_eq!(
            fetched.string("/jobTemplate/queue"),
            queue_arn(value),
            "case {label:?}: expected the stored queue read back as its ARN, got {}",
            fetched.body
        );

        // A job created from the template is submitted to that same queue.
        let job = emulator
            .create_job(json!({
                "role": ROLE,
                "jobTemplate": &name,
                "settings": { "inputs": [{ "fileInput": "s3://example-bucket/in/a.mp4" }] }
            }))
            .await;
        assert_eq!(
            job.status, 201,
            "case {label:?}: expected the templated job to be accepted, got {} with body {}",
            job.status, job.body
        );
        assert_eq!(
            job.string("/job/queue"),
            queue_arn(value),
            "case {label:?}: expected the job on the queue the template names, got {}",
            job.body
        );

        let neighbour = value.trim();
        if neighbour == value {
            continue;
        }
        // The neighbouring reference names a queue nobody created, so it resolves nothing.
        let refused = emulator
            .create_job_template(json!({
                "name": format!("{name}-neighbour"),
                "queue": neighbour,
                "settings": template_settings(1, 1)
            }))
            .await;
        assert_eq!(
            refused.status, 404,
            "case {label:?}: expected HTTP 404 for the unknown queue {neighbour:?}, got {} \
             with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("NotFoundException"),
            "case {label:?}: expected NotFoundException for {neighbour:?}, got {:?}",
            refused.error_type
        );
        // And the same holds on the update path.
        let update_refused = emulator
            .update_job_template(&name, json!({ "queue": neighbour }))
            .await;
        assert_eq!(
            update_refused.status, 404,
            "case {label:?}: expected HTTP 404 from UpdateJobTemplate for {neighbour:?}, \
             got {} with body {}",
            update_refused.status, update_refused.body
        );
        let unchanged = emulator.get_job_template(&name).await;
        assert_eq!(
            unchanged.string("/jobTemplate/queue"),
            queue_arn(value),
            "case {label:?}: a refused update must leave the stored queue alone, got {}",
            unchanged.body
        );

        // Created in its own right, the neighbour is a second queue, and an update naming
        // it moves the template to THAT queue, not back to the padded one.
        let sibling = emulator.create_queue(json!({ "name": neighbour })).await;
        assert_eq!(
            sibling.status, 201,
            "case {label:?}: expected the neighbouring queue to be created, got {} with \
             body {}",
            sibling.status, sibling.body
        );
        let updated = emulator
            .update_job_template(&name, json!({ "queue": neighbour }))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateJobTemplate, got {} with body {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.string("/jobTemplate/queue"),
            queue_arn(neighbour),
            "case {label:?}: expected the update to store {neighbour:?}, got {}",
            updated.body
        );
    }
}

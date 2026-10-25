// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the rule that a queue's `Name` is its KEY.
//!
//! `CreateQueue$Name` targets the bare `__string`
//! (`smithy_query(query="operation",operation="CreateQueue")`): the model states no
//! pattern, no length and no normalisation, so the name a caller sends is the name that
//! is stored, echoed and addressable — byte for byte. Every queue-addressing path
//! (`GetQueue`, `UpdateQueue`, `DeleteQueue`, and a job's `Queue` reference) therefore
//! resolves the string the create was given, and a name that differs only by surrounding
//! whitespace is a DIFFERENT queue rather than the same one.
//!
//! The sibling creates of this service already follow that rule for their own keys
//! (`preset_lifecycle` pins it for presets), so these cases pin it for queues.
//!
//! Each case drives the real restJson1 wire path through the router (the harness is the
//! one `job_lifecycle` builds), so what is asserted is the bytes a client would see.

use serde_json::json;

use crate::job_lifecycle::{arn, settings, Emulator, ROLE};
use crate::queue_update::update_queue;

/// One queue name, as the request body carries it and as it goes in a `{Name}` path label.
struct Case {
    label: &'static str,
    /// The `Name` member exactly as `CreateQueue` receives it.
    name: &'static str,
    /// `name` percent-encoded for the `{Name}` path label of the queue-addressing
    /// operations: `http::Uri` refuses a raw space, and the framework percent-decodes a
    /// path label before the handler sees it. Spelled out per row so each case states the
    /// bytes it puts on the wire.
    path: &'static str,
}

/// The rows span the forms of whitespace a bare `__string` name can carry — which is what
/// the rule generalises over — plus one name carrying none at all as the control.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "no whitespace at all",
            name: "plain",
            path: "plain",
        },
        Case {
            label: "a trailing space",
            name: "trail ",
            path: "trail%20",
        },
        Case {
            label: "a leading space",
            name: " lead",
            path: "%20lead",
        },
        Case {
            label: "spaces on both sides",
            name: "  both  ",
            path: "%20%20both%20%20",
        },
        Case {
            label: "a trailing tab",
            name: "tabbed\t",
            path: "tabbed%09",
        },
        Case {
            label: "a trailing newline",
            name: "lined\n",
            path: "lined%0A",
        },
        Case {
            label: "whitespace inside the name",
            name: "two words",
            path: "two%20words",
        },
    ]
}

/// The name a `CreateQueue` was given is the name it stores, echoes and is addressed by:
/// the response names it verbatim, its ARN carries it verbatim, and `GetQueue`,
/// `UpdateQueue` and a job's `Queue` reference all resolve that same string.
#[tokio::test]
async fn a_queue_is_stored_and_addressed_by_the_name_exactly_as_sent() {
    let emulator = Emulator::new();

    for case in cases() {
        let Case { label, name, path } = case;

        let created = emulator.create_queue(json!({ "name": name })).await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        assert_eq!(
            created.string("/queue/name"),
            name,
            "case {label:?}: expected the name echoed verbatim, got {}",
            created.body
        );
        assert_eq!(
            created.string("/queue/arn"),
            arn(&format!("queues/{name}")),
            "case {label:?}: expected the ARN to carry the name verbatim, got {}",
            created.body
        );

        // The create is readable back under the very string it was given, and reports the
        // queue the create persisted.
        let fetched = emulator.get_queue(path).await;
        assert_eq!(
            fetched.status, 200,
            "case {label:?}: expected the created queue to be readable under its own \
             name, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.at("/queue"),
            created.at("/queue"),
            "case {label:?}: expected the stored queue, got {}",
            fetched.body
        );

        // The same string addresses the mutation path, and the mutation is visible to a
        // later read of that string.
        let description = format!("described: {label}");
        let updated = update_queue(&emulator, path, json!({ "description": description })).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateQueue, got {} with body {}",
            updated.status, updated.body
        );
        let reread = emulator.get_queue(path).await;
        assert_eq!(
            reread.string("/queue/description"),
            description,
            "case {label:?}: expected the update visible to a later read, got {}",
            reread.body
        );

        // And a job's `Queue` reference resolves the same string, so a queue the caller
        // just created can be submitted to with the name they used.
        let job = emulator
            .create_job(json!({
                "role": ROLE,
                "queue": name,
                "settings": settings("s3://example-bucket/in/a.mp4", "_out")
            }))
            .await;
        assert_eq!(
            job.status, 201,
            "case {label:?}: expected a job on this queue to be accepted, got {} with \
             body {}",
            job.status, job.body
        );
        assert_eq!(
            job.string("/job/queue"),
            arn(&format!("queues/{name}")),
            "case {label:?}: expected the job to name the queue verbatim, got {}",
            job.body
        );
    }
}

/// Because the name is never normalised, a name that differs only by the whitespace around
/// it is a different queue: the neighbouring form is unknown to every read until it is
/// created in its own right, and then the two are separate resources.
#[tokio::test]
async fn a_name_differing_only_by_surrounding_whitespace_is_a_different_queue() {
    let emulator = Emulator::new();

    for case in cases() {
        let Case { label, name, path } = case;
        if name.trim() == name {
            continue;
        }
        // The trimmed form of every padded row above is free of characters a path label
        // would have to encode.
        let trimmed = name.trim();

        let created = emulator.create_queue(json!({ "name": name })).await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );

        let neighbour = emulator.get_queue(trimmed).await;
        assert_eq!(
            neighbour.status, 404,
            "case {label:?}: expected {trimmed:?} to be unknown, got {} with body {}",
            neighbour.status, neighbour.body
        );
        assert_eq!(
            neighbour.error_type.as_deref(),
            Some("NotFoundException"),
            "case {label:?}: expected NotFoundException for {trimmed:?}, got {:?}",
            neighbour.error_type
        );

        // Created in its own right, it is a second, independent queue: both names read
        // back, each reporting its own name.
        let sibling = emulator.create_queue(json!({ "name": trimmed })).await;
        assert_eq!(
            sibling.status, 201,
            "case {label:?}: expected the trimmed name to create its own queue, got {} \
             with body {}",
            sibling.status, sibling.body
        );
        for (addressed, expected) in [(path, name), (trimmed, trimmed)] {
            let fetched = emulator.get_queue(addressed).await;
            assert_eq!(
                fetched.status, 200,
                "case {label:?}: expected {addressed:?} to read back, got {} with body {}",
                fetched.status, fetched.body
            );
            assert_eq!(
                fetched.string("/queue/name"),
                expected,
                "case {label:?}: expected {addressed:?} to report its own name, got {}",
                fetched.body
            );
        }
    }
}

/// The one name that cannot be stored is a name carrying no characters at all: that is the
/// modelled `BadRequestException`, and it persists nothing.
#[tokio::test]
async fn a_name_carrying_no_characters_is_a_bad_request() {
    let emulator = Emulator::new();

    // Each blank form, with the path label that would address it (empty only in the body).
    let blanks: [(&str, &str); 5] = [
        ("", ""),
        (" ", "%20"),
        ("\t", "%09"),
        ("\n", "%0A"),
        ("   ", "%20%20%20"),
    ];

    for (blank, path) in blanks {
        let refused = emulator.create_queue(json!({ "name": blank })).await;
        assert_eq!(
            refused.status, 400,
            "case {blank:?}: expected HTTP 400, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("BadRequestException"),
            "case {blank:?}: expected BadRequestException, got {:?} with body {}",
            refused.error_type,
            refused.body
        );
        if !path.is_empty() {
            let fetched = emulator.get_queue(path).await;
            assert_eq!(
                fetched.status, 404,
                "case {blank:?}: a refused create must persist nothing, got {} with body {}",
                fetched.status, fetched.body
            );
        }
    }
}

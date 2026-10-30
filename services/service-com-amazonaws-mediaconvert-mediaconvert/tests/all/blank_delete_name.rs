// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the blank-member rule on the doors that DELETE a named
//! resource: `DeleteQueue`, `DeletePreset` and `DeleteJobTemplate`.
//!
//! Each of the three carries its reference as a non-greedy path label
//! (`smithy_query(query="operation",operation="DeleteQueue,DeletePreset,DeleteJobTemplate",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`
//! reports `{Name}` as a required `@httpLabel` on the bare `__string`), so a label made
//! only of whitespace percent-decodes to a value that reaches the handler and carries no
//! resource identity at all. The rule is the one `tests/all/blank_resource_name.rs` pins
//! for the minting and reference-resolving doors, and the certificate suite pins for
//! `DisassociateCertificate`: such a value is the modelled `BadRequestException` (400)
//! naming the member it arrived in, never a probe of a key nothing could hold — and the
//! delete's idempotent empty 202 is for names a resource COULD be stored under.
//!
//! The table spans every blank form the sibling suite is driven through, crossed with all
//! three delete doors; the boundary rows at the end are names that DO carry characters,
//! which the same doors serve: a stored one (removed), a padded stored one (removed only
//! under its own spelling, never under its trimmed one) and one the store never held (the
//! idempotent 202). The system-resource refusal of these doors is pinned by
//! `tests/all/queue_deletion.rs` and `tests/all/system_presets.rs`, so it is not repeated
//! here.
//!
//! Each case drives the real restJson1 wire path through the crate's one test harness (the
//! router and the reply decoding `job_lifecycle` exports), so what is asserted is the bytes
//! a client would see.

use serde_json::json;

use super::blank_resource_name::{
    assert_blank_refusal, blanks, create_resource, doors, listed_names, read_resource, Door,
};
use super::job_lifecycle::{Emulator, Reply};
use super::resource_tagging::encode_label;

/// A stored name whose own spelling is whitespace-PADDED: it carries characters, so the
/// rule stores and addresses it byte for byte, and no refused request below may remove it.
const PADDED_NAME: &str = "  padded name  ";

/// A name that carries characters and that no row ever creates, for the idempotent repeat.
const ABSENT_NAME: &str = "never-created";

/// `DELETE <collection>/<label>` — the binding every one of the three deletes carries.
async fn delete_label(emulator: &Emulator, door: &Door, label: &str) -> Reply {
    emulator
        .send("DELETE", &format!("{}/{label}", door.collection), None)
        .await
}

/// Assert `reply` is the delete's own empty 202 success.
fn assert_deleted(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 202,
        "case {case:?}: expected HTTP 202, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.body,
        json!({}),
        "case {case:?}: a delete models no output members, got body {}",
        reply.body
    );
}

/// No door that DELETES a named resource accepts a `{Name}` carrying no characters: every
/// blank form is the modelled bad request naming the member, and the store is exactly as it
/// was afterwards — the padded resource the request could not name is still readable and
/// still listed.
#[tokio::test]
async fn a_name_carrying_no_characters_is_refused_by_every_named_delete() {
    let emulator = Emulator::new();

    for door in doors() {
        create_resource(&emulator, &door, PADDED_NAME).await;

        for blank in blanks() {
            // The zero-length value is no path segment at all, so it addresses no
            // operation: the forms a delete can receive are the ones that encode to a
            // label.
            if blank.path.is_empty() {
                continue;
            }
            let case = format!("DELETE {} with a name of {}", door.collection, blank.label);
            assert_blank_refusal(
                &case,
                "Name",
                &delete_label(&emulator, &door, blank.path).await,
            );

            // The refusal removed nothing: the one stored resource, whose name trims to
            // the same nothing the refused label does, is still there.
            let survivor = read_resource(&emulator, &door, PADDED_NAME).await;
            assert_eq!(
                survivor.status, 200,
                "case {case:?}: a refused delete must remove nothing, got {} with body {}",
                survivor.status, survivor.body
            );
            assert_eq!(
                survivor.string(&format!("/{}/name", door.view)),
                PADDED_NAME,
                "case {case:?}: the stored name must read back verbatim, got body {}",
                survivor.body
            );
        }

        let listed = listed_names(&emulator, &door).await;
        assert!(
            listed.iter().any(|name| name == PADDED_NAME),
            "case {:?}: the listing must still report the stored resource, got {listed:?}",
            door.label
        );
    }
}

/// The boundary of the same rule: a `{Name}` that carries characters is served by every
/// delete door — the stored resource is removed under its own spelling only, and a name the
/// store never held is still the delete's idempotent empty 202 rather than a refusal.
#[tokio::test]
async fn a_name_that_carries_characters_is_still_deleted() {
    let emulator = Emulator::new();

    for door in doors() {
        create_resource(&emulator, &door, PADDED_NAME).await;

        // Whitespace is part of the key, so the padded resource is NOT addressed by its
        // trimmed spelling: that names nothing, which is the idempotent success, and it
        // leaves the padded resource stored.
        let trimmed = PADDED_NAME.trim();
        assert_deleted(
            &format!("DELETE {} with the trimmed spelling", door.collection),
            &delete_label(&emulator, &door, &encode_label(trimmed)).await,
        );
        assert_deleted(
            &format!("DELETE {} with a name nothing is stored under", door.collection),
            &delete_label(&emulator, &door, &encode_label(ABSENT_NAME)).await,
        );
        let survivor = read_resource(&emulator, &door, PADDED_NAME).await;
        assert_eq!(
            survivor.status, 200,
            "case {:?}: a delete of another name must not remove the stored resource, got \
             {} with body {}",
            door.label, survivor.status, survivor.body
        );

        // Its own spelling does remove it, and the name is unreadable afterwards.
        assert_deleted(
            &format!("DELETE {} with the stored spelling", door.collection),
            &delete_label(&emulator, &door, &encode_label(PADDED_NAME)).await,
        );
        let gone = read_resource(&emulator, &door, PADDED_NAME).await;
        assert_eq!(
            gone.status, 404,
            "case {:?}: the deleted resource must be gone, got {} with body {}",
            door.label, gone.status, gone.body
        );
        let listed = listed_names(&emulator, &door).await;
        assert!(
            !listed.iter().any(|name| name == PADDED_NAME),
            "case {:?}: the listing must not report the deleted resource, got {listed:?}",
            door.label
        );

        // The repeat of the delete that succeeded is the same empty 202.
        assert_deleted(
            &format!("DELETE {} repeated after the delete", door.collection),
            &delete_label(&emulator, &door, &encode_label(PADDED_NAME)).await,
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the blank-member rule on the doors that address a stored
//! resource by its `{Name}` label WITHOUT deleting it: `GetQueue`, `GetPreset`,
//! `GetJobTemplate` and the three `Update*` doors onto the same three resources.
//!
//! Every one of the six carries its name as a non-greedy path label
//! (`smithy_query(query="operation",operation="GetQueue,GetPreset,GetJobTemplate,UpdateQueue,UpdatePreset,UpdateJobTemplate",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`
//! reports `{Name}` as a required `@httpLabel` on the bare `__string`, and
//! `BadRequestException` (400) among the errors of each), so a label made only of
//! whitespace percent-decodes to a value that reaches the handler and carries no resource
//! identity at all. The rule is the one `tests/all/blank_resource_name.rs` pins for the
//! minting and reference-resolving doors and `tests/all/blank_delete_name.rs` for the
//! deletes: such a value is the modelled bad request naming the member it arrived in,
//! never a probe of a key nothing could hold — so the status a client branches on cannot
//! depend on which door onto one resource it used.
//!
//! The table spans every blank form the sibling suites are driven through, crossed with
//! all six doors. The boundary cases are the other side of the same rule: a label that
//! DOES carry characters is resolved against the store, so a stored name is served (read
//! and modified) and a name the store never held is the modelled `NotFoundException`
//! (404) — including the trimmed spelling of a stored padded name, which is a name a
//! resource COULD have had and nothing is stored under.
//!
//! Each case drives the real restJson1 wire path through the crate's one test harness (the
//! router and the reply decoding `job_lifecycle` exports), so what is asserted is the bytes
//! a client would see.

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::state::not_found_message;

use super::blank_resource_name::{
    assert_blank_refusal, blanks, create_resource, doors, read_resource, Door,
};
use super::job_lifecycle::{Emulator, Reply};
use super::resource_tagging::encode_label;

/// A stored name whose own spelling is whitespace-PADDED: it carries characters, so the
/// rule stores and addresses it byte for byte, and no refused request below may reach it.
const PADDED_NAME: &str = "  padded name  ";

/// A name that carries characters and that no row ever creates, for the lookup that
/// resolves nothing.
const ABSENT_NAME: &str = "never-created";

/// The description an accepted update writes, so a refused one is visibly distinguishable
/// from an applied one on the resource that was left alone.
const UPDATED_DESCRIPTION: &str = "updated by the boundary case";

/// `GET <collection>/<label>` — the binding every one of the three readers carries.
async fn read_label(emulator: &Emulator, door: &Door, label: &str) -> Reply {
    emulator
        .send("GET", &format!("{}/{label}", door.collection), None)
        .await
}

/// `PUT <collection>/<label>` — the binding every one of the three `Update*` doors
/// carries. `Description` is an optional member of all three update inputs, so the only
/// thing a refused row below can be refused for is the label.
async fn update_label(emulator: &Emulator, door: &Door, label: &str) -> Reply {
    emulator
        .send(
            "PUT",
            &format!("{}/{label}", door.collection),
            Some(json!({ "description": UPDATED_DESCRIPTION })),
        )
        .await
}

/// Assert `reply` is the modelled 404 for a name that carries characters and names
/// nothing, worded as every lookup in this crate that resolves nothing words it.
fn assert_not_found(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("NotFoundException"),
        "case {case:?}: expected NotFoundException, got {:?} with body {}",
        reply.error_type,
        reply.body
    );
    assert_eq!(
        reply.string("/message"),
        not_found_message("name"),
        "case {case:?}: expected the refusal to name the member read, got body {}",
        reply.body
    );
}

/// No door addressed by a `{Name}` label accepts one carrying no characters: every blank
/// form is the modelled bad request naming the member, on the reader and on the update
/// alike, and the stored resource whose name trims to the same nothing is untouched.
#[tokio::test]
async fn a_name_carrying_no_characters_is_refused_by_every_name_addressed_door() {
    let emulator = Emulator::new();

    for door in doors() {
        create_resource(&emulator, &door, PADDED_NAME).await;

        for blank in blanks() {
            // The zero-length value is no path segment at all, so it addresses no
            // operation: the forms these doors can receive are the ones that encode to a
            // label.
            if blank.path.is_empty() {
                continue;
            }
            assert_blank_refusal(
                &format!("GET {} with a name of {}", door.collection, blank.label),
                "Name",
                &read_label(&emulator, &door, blank.path).await,
            );
            assert_blank_refusal(
                &format!("PUT {} with a name of {}", door.collection, blank.label),
                "Name",
                &update_label(&emulator, &door, blank.path).await,
            );

            // Neither refusal reached the one stored resource, whose name trims to the
            // same nothing the refused label does: it still reads back verbatim and still
            // carries no description.
            let case = format!("{} after a name of {}", door.collection, blank.label);
            let survivor = read_resource(&emulator, &door, PADDED_NAME).await;
            assert_eq!(
                survivor.status, 200,
                "case {case:?}: the stored resource must still be readable, got {} with \
                 body {}",
                survivor.status, survivor.body
            );
            assert_eq!(
                survivor.string(&format!("/{}/name", door.view)),
                PADDED_NAME,
                "case {case:?}: the stored name must read back verbatim, got body {}",
                survivor.body
            );
            assert_eq!(
                survivor.at(&format!("/{}/description", door.view)),
                &Value::Null,
                "case {case:?}: a refused update must change nothing, got body {}",
                survivor.body
            );
        }
    }
}

/// The boundary of the same rule: a `{Name}` that carries characters is resolved against
/// the store by both doors — the stored spelling is read and modified, and a name nothing
/// is stored under (including the stored name's trimmed spelling) is the modelled 404
/// rather than the bad request.
#[tokio::test]
async fn a_name_that_carries_characters_is_resolved_against_the_store() {
    let emulator = Emulator::new();

    for door in doors() {
        create_resource(&emulator, &door, PADDED_NAME).await;

        // Whitespace is part of the key, so the padded resource is NOT addressed by its
        // trimmed spelling: that names a resource this store does not hold, which is the
        // lookup that resolves nothing rather than a problem in the request.
        for (label, name) in [
            ("the trimmed spelling", PADDED_NAME.trim()),
            ("a name nothing is stored under", ABSENT_NAME),
        ] {
            let encoded = encode_label(name);
            assert_not_found(
                &format!("GET {} with {label}", door.collection),
                &read_label(&emulator, &door, &encoded).await,
            );
            assert_not_found(
                &format!("PUT {} with {label}", door.collection),
                &update_label(&emulator, &door, &encoded).await,
            );
        }

        // Its own spelling is served by both doors, and the update it carries is the one
        // the later read reports.
        let read = read_resource(&emulator, &door, PADDED_NAME).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: the stored spelling must be readable, got {} with body {}",
            door.label, read.status, read.body
        );
        let updated = update_label(&emulator, &door, &encode_label(PADDED_NAME)).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: the stored spelling must be modifiable, got {} with body {}",
            door.label, updated.status, updated.body
        );
        let after = read_resource(&emulator, &door, PADDED_NAME).await;
        assert_eq!(
            after.string(&format!("/{}/description", door.view)),
            UPDATED_DESCRIPTION,
            "case {:?}: the accepted update must be readable back, got body {}",
            door.label, after.body
        );
        assert_eq!(
            after.string(&format!("/{}/name", door.view)),
            PADDED_NAME,
            "case {:?}: the stored name must read back verbatim, got body {}",
            door.label, after.body
        );
    }
}

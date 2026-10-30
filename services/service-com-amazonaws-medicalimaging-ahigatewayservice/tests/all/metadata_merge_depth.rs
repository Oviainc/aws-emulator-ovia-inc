// SPDX-License-Identifier: CC0-1.0

//! How DEEPLY a delivered attribute document nests is the caller's choice, not the
//! service's.
//!
//! RULE: an attribute document a caller delivers is merged into the one the image set
//! holds LEVEL BY LEVEL however deeply either nests — a level the delivery describes in
//! part gains what it carries without losing the rest, at every depth, and the descent
//! costs heap rather than stack (`crate::state`'s merge of written attributes), so a
//! document nested far deeper than any DICOM hierarchy is answered like any other.
//!
//! The suite is a TABLE over the one axis the rule generalises over, the nesting depth: a
//! depth a real aggregated document reaches, and two far past it. Each row delivers twice,
//! so the second delivery meets a held document of the same depth: the attribute at the
//! bottom of the chain reports what the second delivery wrote, and the one BESIDE it,
//! which only the first delivery named, is still held — which is what a merge that
//! descended the whole chain leaves behind, and what one that stopped early and replaced
//! the level whole would not. Every case drives real HTTP through the production front
//! door, so what is asserted is the wire behaviour a `medical-imaging` SDK client sees.

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::IMAGE_SET_UPDATE_DELAY;

use crate::datastore_lifecycle::{Emulator, SETTLE_MULTIPLE};
use crate::image_set_metadata_updates::{
    at, dicom_updates, read_document, seed_image_set, seeded_tags, update_metadata, STUDY_UID,
};

/// The attribute the chain of levels hangs from, at the patient level — an attribute the
/// model does not name, which is what lets a case choose the shape below it.
const CHAIN_ROOT: &str = "DeepChain";

/// The attribute at the bottom of the chain each delivery writes.
const LEAF: &str = "Leaf";

/// The attribute beside it that only the FIRST delivery names, so what the second delivery
/// does to the level it sits in is observable.
const KEPT: &str = "Kept";

/// `attributes` under a chain of `depth` nested levels.
fn nested(depth: usize, attributes: Value) -> Value {
    let mut value = attributes;
    for level in (0..depth).rev() {
        value = json!({ format!("Level{level}"): value });
    }
    value
}

/// What the document reports at `name` at the bottom of a `depth`-deep chain.
fn deep(document: &Value, depth: usize, name: &str) -> Value {
    let mut cursor = &document["Patient"]["DICOM"][CHAIN_ROOT];
    for level in 0..depth {
        cursor = &cursor[format!("Level{level}")];
    }
    cursor[name].clone()
}

/// The `updatableAttributes` document one delivery carries: the chain, one attribute
/// beside the chain, and nothing about the rest of the patient level.
fn delivered(depth: usize, attributes: Value) -> Value {
    json!({
        "SchemaVersion": "1.1",
        "Patient": {
            "DICOM": {
                "PatientAge": "054Y",
                CHAIN_ROOT: nested(depth, attributes),
            },
        },
    })
}

#[tokio::test]
async fn a_deeply_nested_attribute_document_is_merged_level_by_level() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("metadata.depth", "token-metadata-depth")
        .await;

    for depth in [2_usize, 64, 120] {
        let case = format!("depth={depth}");
        let image_set_id = seed_image_set(&emulator, &datastore_id, seeded_tags(STUDY_UID));

        // The SECOND delivery is what merges one deep document into another: the first
        // only writes the chain, the second meets it level by level.
        let deliveries = [
            json!({ LEAF: "first", KEPT: "held" }),
            json!({ LEAF: "second" }),
        ];
        for (index, attributes) in deliveries.into_iter().enumerate() {
            let version = (index + 1).to_string();
            let updated = update_metadata(
                &emulator,
                &datastore_id,
                &image_set_id,
                &version,
                dicom_updates(Some(&delivered(depth, attributes)), None),
                &[],
            )
            .await;
            assert_eq!(
                updated.status, 200,
                "case {case} delivery {version}: expected HTTP 200 from \
                 UpdateImageSetMetadata, got {updated:?}"
            );
            // The clock, not a reader, settles the update.
            tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_UPDATE_DELAY).await;
        }

        let document = read_document(&emulator, &datastore_id, &image_set_id, &case).await;
        assert_eq!(
            deep(&document, depth, LEAF),
            json!("second"),
            "case {case}: expected the last delivery's attribute at the bottom of the chain, got \
             {document}"
        );
        assert_eq!(
            deep(&document, depth, KEPT),
            json!("held"),
            "case {case}: expected the bottom level to keep the attribute only the first \
             delivery named, got {document}"
        );
        assert_eq!(
            at(&document, &["Patient", "DICOM", "PatientAge"]),
            json!("054Y"),
            "case {case}: expected the attribute delivered beside the chain, got {document}"
        );
        assert_eq!(
            at(&document, &["Patient", "DICOM", "PatientName"]),
            json!("DOE^JANE"),
            "case {case}: expected the patient level to keep the attribute neither delivery \
             mentioned, got {document}"
        );
    }
}

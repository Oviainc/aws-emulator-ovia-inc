// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what `CopyImageSet`'s `promoteToPrimary` does, proved through
//! the `GetImageSet` read.
//!
//! RULE: membership of the primary DICOM hierarchy is a property an image set is MINTED
//! with and that a copy CARRIES — never a flag a request writes onto a record the caller
//! named. `promoteToPrimary` therefore makes the copy mint a NEW image set inside the
//! primary hierarchy and answer with that newly minted id as the destination, leaving
//! the source — and every other image set the data store holds — exactly where it was;
//! a request that both asks for a promotion and names a `destinationImageSet` asks for
//! two destinations at once and is refused with 400 `ValidationException` naming the
//! member it cannot carry.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! hierarchy the source is in, how many versions it holds, whether the copy takes a
//! subset of its SOP instances, and which flags the request carries. Every case drives
//! real HTTP through the production front door.
//!
//! The SOURCE of each copy is seeded through the crate's public `state` module for the
//! reason `image_set_copy` gives: this emulator's DICOM import does not create image
//! sets yet.

use serde_json::json;

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::IMAGE_SET_COPY_DELAY;

use crate::datastore_lifecycle::{Emulator, SETTLE_MULTIPLE};
use crate::image_set_copy::{
    copy_image_set, get_image_set, information, seed_image_set_in_hierarchy,
};

/// One row of the promotion table.
struct PromoteCase {
    case: &'static str,
    /// Which hierarchy the SOURCE is in going in — a promotion answers the same way from
    /// either.
    source_is_primary: bool,
    /// How many versions the source holds, which is also the version the request names.
    source_versions: u32,
    /// `DICOMCopies.copiableAttributes`: a promotion of a whole image set, and of a
    /// subset of its SOP instances.
    copiable_attributes: Option<&'static str>,
    /// Whether the request also carries `force`.
    force: Option<bool>,
}

/// The rows span: a non-primary and an already-primary source; a source at one, two and
/// four versions; a whole copy and a subset copy (object and array forms); and `force`
/// absent, present-false and present-true.
fn promote_cases() -> Vec<PromoteCase> {
    vec![
        PromoteCase {
            case: "a whole copy of a non-primary source at one version",
            source_is_primary: false,
            source_versions: 1,
            copiable_attributes: None,
            force: None,
        },
        PromoteCase {
            case: "a subset copy of a non-primary source at two versions, forced",
            source_is_primary: false,
            source_versions: 2,
            copiable_attributes: Some(r#"{"SchemaVersion":"1.1","Study":{"Series":{}}}"#),
            force: Some(true),
        },
        PromoteCase {
            case: "a subset copy of a non-primary source at four versions, force false",
            source_is_primary: false,
            source_versions: 4,
            copiable_attributes: Some(r#"[{"SOPInstanceUID":"1.2.840.10008.1.2.1"}]"#),
            force: Some(false),
        },
        PromoteCase {
            case: "a whole copy of a source that is already primary",
            source_is_primary: true,
            source_versions: 2,
            copiable_attributes: None,
            force: None,
        },
        PromoteCase {
            case: "a subset copy of a source that is already primary, forced",
            source_is_primary: true,
            source_versions: 1,
            copiable_attributes: Some(r#"{"Study":{"DICOM":{}}}"#),
            force: Some(true),
        },
    ]
}

/// A promotion answers with a NEWLY MINTED primary image set as its destination, and
/// moves no image set the data store already held.
#[tokio::test]
async fn a_promotion_mints_a_new_primary_image_set_as_the_destination() {
    for (row, case) in promote_cases().into_iter().enumerate() {
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore("promote.store", &format!("promote-{row}"))
            .await;
        let source_id = seed_image_set_in_hierarchy(
            &emulator,
            &datastore_id,
            case.source_versions,
            case.source_is_primary,
        );
        // A second image set the request never names, so a promotion that versioned
        // some other record instead of minting one would show up here.
        let bystander_id = seed_image_set_in_hierarchy(&emulator, &datastore_id, 2, false);

        let mut query: Vec<(&str, bool)> = vec![("promoteToPrimary", true)];
        if let Some(force) = case.force {
            query.push(("force", force));
        }
        let source_version = case.source_versions.to_string();
        let promoted = copy_image_set(
            &emulator,
            &datastore_id,
            &source_id,
            information(&source_version, case.copiable_attributes, None),
            &query,
        )
        .await;

        assert_eq!(
            promoted.status, 200,
            "case {:?}: expected HTTP 200 from CopyImageSet, got {promoted:?}",
            case.case
        );

        // The destination is a NEW image set: not the source, not the bystander, and at
        // version 1 because nothing had ever been written into it before this copy.
        let destination_id = promoted.string(&["destinationImageSetProperties", "imageSetId"]);
        assert_ne!(
            destination_id, source_id,
            "case {:?}: a promotion must not answer with the source, got {promoted:?}",
            case.case
        );
        assert_ne!(
            destination_id, bystander_id,
            "case {:?}: a promotion must not answer with another held image set, got {promoted:?}",
            case.case
        );
        assert_eq!(
            destination_id.len(),
            32,
            "case {:?}: a minted image set id is `[0-9a-z]{{32}}`, got {destination_id:?}",
            case.case
        );
        assert!(
            destination_id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "case {:?}: a minted image set id is `[0-9a-z]{{32}}`, got {destination_id:?}",
            case.case
        );
        assert_eq!(
            promoted.string(&["destinationImageSetProperties", "latestVersionId"]),
            "1",
            "case {:?}: a minted destination starts at version 1, got {promoted:?}",
            case.case
        );
        assert_eq!(
            promoted.string(&["sourceImageSetProperties", "latestVersionId"]),
            source_version,
            "case {:?}: a promotion adds no version to the source, got {promoted:?}",
            case.case
        );

        // Once the clock has landed the copy, the minted destination is the primary image
        // set the promotion asked for.
        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY).await;
        let destination = get_image_set(&emulator, &datastore_id, &destination_id, None).await;
        assert_eq!(
            destination.status, 200,
            "case {:?}: expected HTTP 200 from GetImageSet, got {destination:?}",
            case.case
        );
        assert_eq!(
            destination.field(&["isPrimary"]),
            &json!(true),
            "case {:?}: the minted destination must be primary, got {destination:?}",
            case.case
        );
        assert_eq!(
            destination.string(&["versionId"]),
            "1",
            "case {:?}: the minted destination holds one version, got {destination:?}",
            case.case
        );
        assert_eq!(
            destination.string(&["imageSetWorkflowStatus"]),
            "COPIED",
            "case {:?}: the destination must report COPIED once the copy is due, got {destination:?}",
            case.case
        );

        // The source keeps its own hierarchy and its own version count: a promotion
        // copies it, it does not move it.
        let source = get_image_set(&emulator, &datastore_id, &source_id, None).await;
        assert_eq!(
            source.field(&["isPrimary"]),
            &json!(case.source_is_primary),
            "case {:?}: the source's hierarchy must be untouched, got {source:?}",
            case.case
        );
        assert_eq!(
            source.string(&["versionId"]),
            source_version,
            "case {:?}: the source must gain no version, got {source:?}",
            case.case
        );

        // And the image set the request never named is untouched in both respects.
        let bystander = get_image_set(&emulator, &datastore_id, &bystander_id, None).await;
        assert_eq!(
            bystander.field(&["isPrimary"]),
            &json!(false),
            "case {:?}: an image set the request never named must not be promoted, got {bystander:?}",
            case.case
        );
        assert_eq!(
            bystander.string(&["versionId"]),
            "2",
            "case {:?}: an image set the request never named must gain no version, got {bystander:?}",
            case.case
        );
    }
}

/// Two promotions of the same source mint two DIFFERENT primary image sets: the
/// destination is minted per call, not looked up.
#[tokio::test]
async fn each_promotion_mints_its_own_destination() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("promote.twice", "promote-twice")
        .await;
    let source_id = seed_image_set_in_hierarchy(&emulator, &datastore_id, 1, false);

    let mut minted: Vec<String> = Vec::new();
    // Rows: the same promotion run twice, and a third time as a subset copy.
    for (case, attributes) in [
        ("first whole copy", None),
        ("second whole copy", None),
        ("subset copy", Some(r#"{"Study":{"Series":{}}}"#)),
    ] {
        let promoted = copy_image_set(
            &emulator,
            &datastore_id,
            &source_id,
            information("1", attributes, None),
            &[("promoteToPrimary", true)],
        )
        .await;
        assert_eq!(
            promoted.status, 200,
            "case {case:?}: expected HTTP 200 from CopyImageSet, got {promoted:?}"
        );
        let destination_id = promoted.string(&["destinationImageSetProperties", "imageSetId"]);
        assert!(
            !minted.contains(&destination_id),
            "case {case:?}: each promotion mints its own destination, got {destination_id:?} again after {minted:?}"
        );
        minted.push(destination_id);
        // The source is LOCKED read-only while the copy runs, so the next promotion has
        // to wait for the clock to release it.
        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY).await;
    }

    for destination_id in &minted {
        let destination = get_image_set(&emulator, &datastore_id, destination_id, None).await;
        assert_eq!(
            destination.field(&["isPrimary"]),
            &json!(true),
            "case {destination_id:?}: every promoted destination is primary, got {destination:?}"
        );
    }
}

/// One row of the refusal table: a promotion that also names a destination.
struct PromoteRefusalCase {
    case: &'static str,
    /// The version the request claims is the destination's latest; the seeded
    /// destination holds exactly two versions, so "2" is the only correct claim.
    claimed_destination_version: &'static str,
    /// Whether the named destination is already primary.
    destination_is_primary: bool,
    /// Whether the request also takes a subset of the source's SOP instances.
    copiable_attributes: Option<&'static str>,
    /// Whether the request also carries `force`.
    force: Option<bool>,
}

/// The rows span: a destination version claimed correctly, below and above the one held;
/// a destination inside and outside the primary hierarchy; a whole and a subset copy;
/// and `force` absent and present.
fn promote_refusal_cases() -> Vec<PromoteRefusalCase> {
    vec![
        PromoteRefusalCase {
            case: "the destination's version claimed correctly",
            claimed_destination_version: "2",
            destination_is_primary: false,
            copiable_attributes: None,
            force: None,
        },
        PromoteRefusalCase {
            case: "the destination's version claimed one below the latest",
            claimed_destination_version: "1",
            destination_is_primary: false,
            copiable_attributes: Some(r#"{"Study":{"Series":{}}}"#),
            force: Some(true),
        },
        PromoteRefusalCase {
            case: "the destination's version claimed one above the latest",
            claimed_destination_version: "3",
            destination_is_primary: true,
            copiable_attributes: None,
            force: Some(false),
        },
        PromoteRefusalCase {
            case: "an already-primary destination, subset copy, forced",
            claimed_destination_version: "2",
            destination_is_primary: true,
            copiable_attributes: Some(r#"[{"SOPInstanceUID":"1.2.3"}]"#),
            force: Some(true),
        },
    ]
}

/// A promotion mints its own destination, so a request that also names one cannot hold
/// together: it is the modelled input-constraint failure, and it writes nothing.
#[tokio::test]
async fn a_promotion_that_also_names_a_destination_is_refused() {
    for (row, case) in promote_refusal_cases().into_iter().enumerate() {
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore("promote.refusal", &format!("promote-refusal-{row}"))
            .await;
        let source_id = seed_image_set_in_hierarchy(&emulator, &datastore_id, 2, false);
        let destination_id =
            seed_image_set_in_hierarchy(&emulator, &datastore_id, 2, case.destination_is_primary);

        let mut query: Vec<(&str, bool)> = vec![("promoteToPrimary", true)];
        if let Some(force) = case.force {
            query.push(("force", force));
        }
        let payload = information(
            "2",
            case.copiable_attributes,
            Some((destination_id.as_str(), case.claimed_destination_version)),
        );
        let refused =
            copy_image_set(&emulator, &datastore_id, &source_id, payload.clone(), &query).await;

        assert_eq!(
            refused.status, 400,
            "case {:?}: expected HTTP 400, got {refused:?}",
            case.case
        );
        assert_eq!(
            refused.error_code, "ValidationException",
            "case {:?}: expected ValidationException, got {refused:?}",
            case.case
        );
        let message = refused.string(&["message"]);
        assert!(
            message.contains("copyImageSetInformation.destinationImageSet"),
            "case {:?}: the refusal must name the member, got {message:?}",
            case.case
        );

        // Nothing was written: neither image set gained a version, changed state or
        // changed hierarchy.
        for (role, image_set_id, is_primary) in [
            ("source", &source_id, false),
            ("destination", &destination_id, case.destination_is_primary),
        ] {
            let read = get_image_set(&emulator, &datastore_id, image_set_id, None).await;
            assert_eq!(
                read.string(&["imageSetState"]),
                "ACTIVE",
                "case {:?}: the {role} must be untouched by a refusal, got {read:?}",
                case.case
            );
            assert_eq!(
                read.string(&["versionId"]),
                "2",
                "case {:?}: the {role} must gain no version from a refusal, got {read:?}",
                case.case
            );
            assert_eq!(
                read.field(&["isPrimary"]),
                &json!(is_primary),
                "case {:?}: the {role}'s hierarchy must be untouched by a refusal, got {read:?}",
                case.case
            );
        }

        // The SAME request without the promotion is admitted, so the refusal is about
        // the two members together and not about the request's shape.
        let without_promotion = copy_image_set(
            &emulator,
            &datastore_id,
            &source_id,
            payload,
            &[("promoteToPrimary", false)],
        )
        .await;
        let expected_status = if case.claimed_destination_version == "2" {
            200
        } else {
            // A destination version that is not the one held is the optimistic-
            // concurrency conflict, which is a different refusal entirely.
            409
        };
        assert_eq!(
            without_promotion.status, expected_status,
            "case {:?}: expected HTTP {expected_status} without the promotion, got {without_promotion:?}",
            case.case
        );
    }
}

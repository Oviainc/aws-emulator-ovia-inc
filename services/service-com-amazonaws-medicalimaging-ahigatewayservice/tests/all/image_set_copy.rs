// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CopyImageSet` and the `GetImageSet` read it is proved
//! through.
//!
//! RULE: a copy is admitted only from an image set the data store holds whose CURRENT
//! latest version the caller named, and whose state is `ACTIVE`; it lands as a new
//! VERSION of the destination — version 1 of an image set the call mints when the
//! request names no destination, and one past the named version of the destination it
//! does name — carrying the request's `force` as that version's `Overrides.forced`.
//! Membership of the primary DICOM hierarchy (`isPrimary`) is carried by the copy, never
//! set as a flag on a record the caller named: a `promoteToPrimary` request MINTS a new
//! primary image set and answers with ITS id as the destination (so a request that also
//! names `destinationImageSet` is refused), a copy of a whole image set mints a
//! destination in the hierarchy its source is in, a copy of a SUBSET of the source's SOP
//! instances mints one outside the primary hierarchy, and an image set the copy versions
//! keeps the hierarchy it already belonged to. Both image sets
//! answer `LOCKED` while the copy is in flight (the source
//! `COPYING_WITH_READ_ONLY_ACCESS`, the destination `COPYING`) and the CLOCK, not a
//! later reader, returns them to `ACTIVE` — the destination `COPIED`, the source the
//! status it carried before the copy, which a copy does not change. Anything the rule
//! does not admit is the modelled failure: an absent resource is 404
//! `ResourceNotFoundException`, a version that is not the latest or an image set that
//! is not `ACTIVE` is 409 `ConflictException`, and a request that cannot hold together
//! is 400 `ValidationException`.
//!
//! Every suite is a TABLE whose rows differ along what the rule generalises over:
//! whether the destination is minted or versioned, how many versions each image set
//! already holds, which optional flags the request carries, what the
//! `copiableAttributes` document names, and which resource or version the request gets
//! wrong.
//! Each case drives real HTTP through the production front door, so what is asserted
//! is the wire behaviour a `medical-imaging` SDK client sees.
//!
//! ## Why a seeded source
//!
//! In AWS an image set comes into existence through a DICOM import, and this emulator's
//! import job does not create image sets yet — that is
//! `issues/things-that-do-not-work/20261024T002353Z-medical-imaging-startdicomimportjob-event-propagation.md`.
//! So the SOURCE of a copy is seeded through the crate's public `state` module, with
//! `ImageSet::new` — the one constructor the handler itself mints image sets with — and
//! everything each case then asserts goes over HTTP.

use std::time::{Instant, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    image_set_arn, service_state, ImageSet, ImageSetState, ImageSetWorkflowStatus,
    IMAGE_SET_COPY_DELAY,
};

use crate::datastore_lifecycle::{Answer, Emulator, ACCOUNT, SETTLE_MULTIPLE};

/// A well-formed `[0-9a-z]{32}` resource id that no call ever mints (the ids `mint_id`
/// produces are SplitMix64 output, never all zeros).
fn unheld_id() -> String {
    "0".repeat(32)
}

/// The workflow status a seeded image set carries going in: `IMPORTED`, which is what a
/// DICOM import leaves behind.
///
/// `pub(crate)` because `crate::image_set_lock_visibility` asserts what a seeded version
/// still reports while a write holds the image set, so its expectation is tied to what
/// this seeder writes rather than spelled again.
pub(crate) const SEEDED_WORKFLOW_STATUS: ImageSetWorkflowStatus = ImageSetWorkflowStatus::Imported;

/// The workflow status a seeded image set's LATER versions carry, so a case that reads
/// an earlier version back can tell the versions apart.
///
/// `pub(crate)` for the reason [`SEEDED_WORKFLOW_STATUS`] is.
pub(crate) const SEEDED_LATER_WORKFLOW_STATUS: ImageSetWorkflowStatus =
    ImageSetWorkflowStatus::Updated;

/// Seed one `ACTIVE` image set with `versions` versions into a data store the emulator
/// already holds, and answer its identifier. The image set is seeded OUTSIDE the primary
/// DICOM hierarchy; [`seed_image_set_in_hierarchy`] chooses.
///
/// Built with the crate's own `ImageSet::new` / `push_version`, so a seeded image set is
/// shaped exactly like one a handler mints.
pub(crate) fn seed_image_set(emulator: &Emulator, datastore_id: &str, versions: u32) -> String {
    seed_image_set_in_hierarchy(emulator, datastore_id, versions, false)
}

/// [`seed_image_set`], with the seeded image set's membership of the primary DICOM
/// hierarchy (`isPrimary`) chosen by the caller — what a copy of it carries.
pub(crate) fn seed_image_set_in_hierarchy(
    emulator: &Emulator,
    datastore_id: &str,
    versions: u32,
    is_primary: bool,
) -> String {
    assert!(
        versions >= 1,
        "an image set always holds at least one version, asked for {versions}"
    );
    let state = service_state(emulator.engine()).expect("the medical-imaging state store");
    let mut store = state.lock();
    let image_set_id = store.mint_id();
    let datastore = store
        .datastores
        .get_mut(datastore_id)
        .expect("the seeded data store");
    let mut image_set = ImageSet::new(
        image_set_id.clone(),
        image_set_arn(&datastore.datastore_arn, &image_set_id),
        ImageSetState::Active,
        SEEDED_WORKFLOW_STATUS,
        is_primary,
        false,
        SystemTime::now(),
    );
    for _ in 1..versions {
        image_set.push_version(
            ImageSetState::Active,
            SEEDED_LATER_WORKFLOW_STATUS,
            false,
            SystemTime::now(),
        );
    }
    datastore.image_sets.insert(image_set_id.clone(), image_set);
    image_set_id
}

/// `POST /datastore/{datastoreId}/imageSet/{sourceImageSetId}/copyImageSet` —
/// `CopyImageSet`. `force` and `promoteToPrimary` are `@httpQuery`; the body IS the
/// `copyImageSetInformation` payload.
pub(crate) async fn copy_image_set(
    emulator: &Emulator,
    datastore_id: &str,
    source_image_set_id: &str,
    information: Value,
    query: &[(&str, bool)],
) -> Answer {
    let mut uri = format!("/datastore/{datastore_id}/imageSet/{source_image_set_id}/copyImageSet");
    for (index, (name, value)) in query.iter().enumerate() {
        let separator = if index == 0 { '?' } else { '&' };
        uri.push(separator);
        uri.push_str(&format!("{name}={value}"));
    }
    emulator.call("POST", &uri, Some(information)).await
}

/// `POST /datastore/{datastoreId}/imageSet/{imageSetId}/getImageSet` — `GetImageSet`,
/// optionally for one version (`version` is an `@httpQuery`).
pub(crate) async fn get_image_set(
    emulator: &Emulator,
    datastore_id: &str,
    image_set_id: &str,
    version_id: Option<&str>,
) -> Answer {
    let mut uri = format!("/datastore/{datastore_id}/imageSet/{image_set_id}/getImageSet");
    if let Some(version_id) = version_id {
        uri.push_str(&format!("?version={version_id}"));
    }
    emulator.call("POST", &uri, None).await
}

/// The `copyImageSetInformation` payload: the source's claimed latest version, an
/// optional `DICOMCopies` subset, and an optional destination.
pub(crate) fn information(
    source_latest_version: &str,
    copiable_attributes: Option<&str>,
    destination: Option<(&str, &str)>,
) -> Value {
    let mut source = json!({ "latestVersionId": source_latest_version });
    if let Some(attributes) = copiable_attributes {
        source["DICOMCopies"] = json!({ "copiableAttributes": attributes });
    }
    let mut payload = json!({ "sourceImageSet": source });
    if let Some((image_set_id, latest_version_id)) = destination {
        payload["destinationImageSet"] = json!({
            "imageSetId": image_set_id,
            "latestVersionId": latest_version_id,
        });
    }
    payload
}

/// One row of the copy table.
struct CopyCase {
    case: &'static str,
    /// How many versions the SOURCE image set is seeded with — also the version the
    /// request must name as the source's latest.
    source_versions: u32,
    /// Whether the SOURCE is in the primary DICOM hierarchy going in.
    source_is_primary: bool,
    /// How many versions the DESTINATION image set is seeded with, or `None` for a
    /// request that names no destination at all, which mints one.
    destination_versions: Option<u32>,
    /// Whether a SEEDED destination is in the primary DICOM hierarchy going in; ignored
    /// for a request that mints its destination.
    destination_is_primary: bool,
    force: Option<bool>,
    promote_to_primary: Option<bool>,
    /// `DICOMCopies.copiableAttributes` — a JSON document naming the subset to copy.
    copiable_attributes: Option<&'static str>,
    /// The version the destination must report once the copy has landed.
    expected_destination_version: u32,
    /// The hierarchy the destination must report once the copy has landed.
    expected_destination_primary: bool,
}

/// The rows span: minting a destination versus versioning one; a source and a
/// destination at one, two and three versions; a source and a destination inside and
/// outside the primary DICOM hierarchy; each optional flag present-true, present-false
/// and absent; and `copiableAttributes` present as a document naming no instance, as a
/// document naming one, and absent.
fn copy_cases() -> Vec<CopyCase> {
    vec![
        CopyCase {
            case: "no destination and no flags, from a primary source: a primary image set at version 1",
            source_versions: 1,
            source_is_primary: true,
            destination_versions: None,
            destination_is_primary: false,
            force: None,
            promote_to_primary: None,
            copiable_attributes: None,
            expected_destination_version: 1,
            // A whole copy carries its source's hierarchy.
            expected_destination_primary: true,
        },
        CopyCase {
            case: "no destination, forced, a copiable subset of a primary source: minted outside the primary hierarchy",
            source_versions: 1,
            source_is_primary: true,
            destination_versions: None,
            destination_is_primary: false,
            force: Some(true),
            promote_to_primary: None,
            copiable_attributes: Some(r#"{"SchemaVersion":"1.1","Study":{"Series":{}}}"#),
            expected_destination_version: 1,
            expected_destination_primary: false,
        },
        CopyCase {
            case: "no destination, both flags present and false, from a non-primary source",
            source_versions: 3,
            source_is_primary: false,
            destination_versions: None,
            destination_is_primary: false,
            force: Some(false),
            promote_to_primary: Some(false),
            copiable_attributes: None,
            expected_destination_version: 1,
            expected_destination_primary: false,
        },
        CopyCase {
            case: "no destination, promoted, a subset of a non-primary source: a newly minted primary image set",
            source_versions: 1,
            source_is_primary: false,
            destination_versions: None,
            destination_is_primary: false,
            force: Some(true),
            promote_to_primary: Some(true),
            copiable_attributes: Some(r#"{"Study":{"DICOM":{}}}"#),
            expected_destination_version: 1,
            expected_destination_primary: true,
        },
        CopyCase {
            case: "into a non-primary destination at version 1: the copy becomes version 2 and the hierarchy is kept",
            source_versions: 1,
            source_is_primary: true,
            destination_versions: Some(1),
            destination_is_primary: false,
            force: None,
            promote_to_primary: None,
            copiable_attributes: None,
            expected_destination_version: 2,
            // An image set the copy VERSIONS keeps the hierarchy it was already in, even
            // where the source is in the other one.
            expected_destination_primary: false,
        },
        CopyCase {
            case: "into a primary destination at version 3, forced, from a non-primary source at version 2",
            source_versions: 2,
            source_is_primary: false,
            destination_versions: Some(3),
            destination_is_primary: true,
            force: Some(true),
            promote_to_primary: Some(false),
            copiable_attributes: Some(
                r#"{"Study":{"Series":{"1.2.3":{"Instances":{"1.2.840.10008.1.2.1":{}}}}}}"#,
            ),
            expected_destination_version: 4,
            expected_destination_primary: true,
        },
    ]
}

/// A copy locks both image sets, lands on the destination as a new version carrying the
/// overrides the request asked for, and the clock — not the read — releases both.
#[tokio::test]
async fn a_copy_lands_a_new_version_and_the_clock_releases_both_image_sets() {
    for (row, case) in copy_cases().into_iter().enumerate() {
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore("copy.store", &format!("copy-{row}"))
            .await;
        let source_id = seed_image_set_in_hierarchy(
            &emulator,
            &datastore_id,
            case.source_versions,
            case.source_is_primary,
        );
        let destination_seed = case.destination_versions.map(|versions| {
            seed_image_set_in_hierarchy(
                &emulator,
                &datastore_id,
                versions,
                case.destination_is_primary,
            )
        });

        let mut query: Vec<(&str, bool)> = Vec::new();
        if let Some(force) = case.force {
            query.push(("force", force));
        }
        if let Some(promote) = case.promote_to_primary {
            query.push(("promoteToPrimary", promote));
        }

        let source_version = case.source_versions.to_string();
        // The destination member names the destination's OWN current latest version,
        // which is what the copy checks it against.
        let destination_version = case.destination_versions.unwrap_or(1).to_string();
        let destination_argument = destination_seed
            .as_ref()
            .map(|id| (id.as_str(), destination_version.as_str()));
        let copied = copy_image_set(
            &emulator,
            &datastore_id,
            &source_id,
            information(&source_version, case.copiable_attributes, destination_argument),
            &query,
        )
        .await;

        assert_eq!(
            copied.status, 200,
            "case {:?}: expected HTTP 200 from CopyImageSet, got {copied:?}",
            case.case
        );
        assert_eq!(
            copied.string(&["datastoreId"]),
            datastore_id,
            "case {:?}: CopyImageSet must report the data store it copied in, got {copied:?}",
            case.case
        );

        // The source: same image set, same version, locked read-only for the copy.
        assert_eq!(
            copied.string(&["sourceImageSetProperties", "imageSetId"]),
            source_id,
            "case {:?}: the source properties must name the source, got {copied:?}",
            case.case
        );
        assert_eq!(
            copied.string(&["sourceImageSetProperties", "latestVersionId"]),
            source_version,
            "case {:?}: a copy adds no version to the source, got {copied:?}",
            case.case
        );
        assert_eq!(
            copied.string(&["sourceImageSetProperties", "imageSetState"]),
            "LOCKED",
            "case {:?}: the source must be LOCKED while the copy runs, got {copied:?}",
            case.case
        );
        assert_eq!(
            copied.string(&["sourceImageSetProperties", "imageSetWorkflowStatus"]),
            "COPYING_WITH_READ_ONLY_ACCESS",
            "case {:?}: the source must report read-only copying, got {copied:?}",
            case.case
        );

        // The destination: the expected new version, locked and copying.
        let destination_id = copied.string(&["destinationImageSetProperties", "imageSetId"]);
        if let Some(seeded) = &destination_seed {
            assert_eq!(
                &destination_id, seeded,
                "case {:?}: a named destination must be the one copied into, got {copied:?}",
                case.case
            );
        } else {
            assert_ne!(
                destination_id, source_id,
                "case {:?}: a minted destination is a NEW image set, got {copied:?}",
                case.case
            );
            assert_eq!(
                destination_id.len(),
                32,
                "case {:?}: a minted image set id is `[0-9a-z]{{32}}`, got {destination_id:?}",
                case.case
            );
        }
        assert_eq!(
            copied.string(&["destinationImageSetProperties", "latestVersionId"]),
            case.expected_destination_version.to_string(),
            "case {:?}: expected destination version {}, got {copied:?}",
            case.case,
            case.expected_destination_version
        );
        assert_eq!(
            copied.string(&["destinationImageSetProperties", "imageSetState"]),
            "LOCKED",
            "case {:?}: the destination must be LOCKED while the copy runs, got {copied:?}",
            case.case
        );
        assert_eq!(
            copied.string(&["destinationImageSetProperties", "imageSetWorkflowStatus"]),
            "COPYING",
            "case {:?}: the destination must report COPYING, got {copied:?}",
            case.case
        );
        assert_eq!(
            copied.string(&["destinationImageSetProperties", "imageSetArn"]),
            format!(
                "arn:aws:medical-imaging:us-east-1:{ACCOUNT}:datastore/{datastore_id}/imageset/{destination_id}"
            ),
            "case {:?}: the destination ARN is the data store's ARN plus the image set, got {copied:?}",
            case.case
        );

        // The transient state is OBSERVABLE: a caller reading straight after the copy
        // still sees the lock, because the clock owes the release a span.
        let in_flight = get_image_set(&emulator, &datastore_id, &destination_id, None).await;
        assert_eq!(
            in_flight.string(&["imageSetState"]),
            "LOCKED",
            "case {:?}: GetImageSet must show the copy in flight, got {in_flight:?}",
            case.case
        );
        assert_eq!(
            in_flight.string(&["imageSetWorkflowStatus"]),
            "COPYING",
            "case {:?}: GetImageSet must show the copy in flight, got {in_flight:?}",
            case.case
        );

        // Then the clock releases both, with no further call needed.
        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY).await;

        let destination = get_image_set(&emulator, &datastore_id, &destination_id, None).await;
        assert_eq!(
            destination.status, 200,
            "case {:?}: expected HTTP 200 from GetImageSet, got {destination:?}",
            case.case
        );
        assert_eq!(
            destination.string(&["versionId"]),
            case.expected_destination_version.to_string(),
            "case {:?}: expected destination version {}, got {destination:?}",
            case.case,
            case.expected_destination_version
        );
        assert_eq!(
            destination.string(&["imageSetState"]),
            "ACTIVE",
            "case {:?}: the destination must be ACTIVE once the copy is due, got {destination:?}",
            case.case
        );
        assert_eq!(
            destination.string(&["imageSetWorkflowStatus"]),
            "COPIED",
            "case {:?}: the destination must report COPIED, got {destination:?}",
            case.case
        );
        assert_eq!(
            destination.field(&["overrides", "forced"]),
            &match case.force {
                // "If an image set was copied using the force flag, this object will
                // contain the forced flag" — present only when it was used.
                Some(true) => json!(true),
                _ => Value::Null,
            },
            "case {:?}: overrides must report the force flag the copy used, got {destination:?}",
            case.case
        );
        assert_eq!(
            destination.field(&["isPrimary"]),
            &json!(case.expected_destination_primary),
            "case {:?}: expected isPrimary {}, got {destination:?}",
            case.case,
            case.expected_destination_primary
        );
        // Whatever the destination landed in, the SOURCE's own hierarchy is untouched: a
        // copy reads its source, it does not move it.
        let source_read = get_image_set(&emulator, &datastore_id, &source_id, None).await;
        assert_eq!(
            source_read.field(&["isPrimary"]),
            &json!(case.source_is_primary),
            "case {:?}: a copy must leave the source's hierarchy alone, got {source_read:?}",
            case.case
        );
        assert_eq!(
            destination.string(&["storageTier"]),
            "FREQUENT_ACCESS",
            "case {:?}: an image set this emulator holds is in frequent access, got {destination:?}",
            case.case
        );

        // A settled read does not move: reading twice reports the same instant.
        let again = get_image_set(&emulator, &datastore_id, &destination_id, None).await;
        assert_eq!(
            again.field(&["updatedAt"]),
            destination.field(&["updatedAt"]),
            "case {:?}: updatedAt must be stable across reads, got {again:?} after {destination:?}",
            case.case
        );

        let source = get_image_set(&emulator, &datastore_id, &source_id, None).await;
        assert_eq!(
            source.string(&["versionId"]),
            source_version,
            "case {:?}: the source keeps its version count, got {source:?}",
            case.case
        );
        assert_eq!(
            source.string(&["imageSetState"]),
            "ACTIVE",
            "case {:?}: the source must be released once the copy is due, got {source:?}",
            case.case
        );
        assert_eq!(
            source.string(&["imageSetWorkflowStatus"]),
            "COPIED",
            "case {:?}: a completed copy is what last happened to the source too, got \
             {source:?}",
            case.case
        );

        // Every version the destination held before the copy is still readable at the
        // identifier it had.
        if let Some(versions) = case.destination_versions {
            for version in 1..=versions {
                let earlier = get_image_set(
                    &emulator,
                    &datastore_id,
                    &destination_id,
                    Some(&version.to_string()),
                )
                .await;
                assert_eq!(
                    earlier.string(&["versionId"]),
                    version.to_string(),
                    "case {:?}: version {version} must still read back, got {earlier:?}",
                    case.case
                );
            }
        }
    }
}

/// `GetImageSet` reports the version asked for, the latest when none is asked for, and
/// a 404 for a version the image set does not hold.
#[tokio::test]
async fn get_image_set_reports_the_version_asked_for() {
    let emulator = Emulator::new();
    let datastore_id = emulator.active_datastore("versions.store", "versions").await;

    // Rows: an image set at one, two and four versions — the version count is what the
    // rule generalises over.
    for versions in [1_u32, 2, 4] {
        let image_set_id = seed_image_set(&emulator, &datastore_id, versions);

        let latest = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
        assert_eq!(
            latest.string(&["versionId"]),
            versions.to_string(),
            "case versions={versions}: no `version` reports the latest, got {latest:?}"
        );

        for version in 1..=versions {
            let read = get_image_set(
                &emulator,
                &datastore_id,
                &image_set_id,
                Some(&version.to_string()),
            )
            .await;
            assert_eq!(
                read.status, 200,
                "case versions={versions} version={version}: expected HTTP 200, got {read:?}"
            );
            assert_eq!(
                read.string(&["versionId"]),
                version.to_string(),
                "case versions={versions} version={version}: wrong version reported, got {read:?}"
            );
            assert_eq!(
                read.string(&["imageSetWorkflowStatus"]),
                if version > 1 {
                    SEEDED_LATER_WORKFLOW_STATUS.as_wire()
                } else {
                    SEEDED_WORKFLOW_STATUS.as_wire()
                },
                "case versions={versions} version={version}: each version keeps its own status, got {read:?}"
            );
        }

        // One past the highest version, and a wholly different one.
        for absent in [(versions + 1).to_string(), "99".to_string()] {
            let missing =
                get_image_set(&emulator, &datastore_id, &image_set_id, Some(&absent)).await;
            assert_eq!(
                missing.status, 404,
                "case versions={versions} version={absent}: an absent version is a 404, got {missing:?}"
            );
            assert_eq!(
                missing.error_code, "ResourceNotFoundException",
                "case versions={versions} version={absent}: wrong error code, got {missing:?}"
            );
        }
    }
}

/// Which resource or version a refusal row gets wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    /// The resource this case seeded.
    Held,
    /// A well-formed identifier nothing holds.
    Unheld,
}

/// One row of the refusal table.
struct RefusalCase {
    case: &'static str,
    datastore: Named,
    source: Named,
    /// The version the request CLAIMS is the source's latest; the seeded source holds
    /// exactly two versions, so "2" is the only correct claim.
    claimed_source_version: &'static str,
    /// The destination member: which image set, and the version it claims. `None` for a
    /// request that names no destination.
    destination: Option<(Named, &'static str)>,
    expected_status: u16,
    expected_code: &'static str,
}

/// The rows span: each identifier in the request naming nothing held; a claimed source
/// version below, above and beside the real one; and the same for the destination.
fn refusal_cases() -> Vec<RefusalCase> {
    vec![
        RefusalCase {
            case: "a data store nothing holds",
            datastore: Named::Unheld,
            source: Named::Held,
            claimed_source_version: "2",
            destination: None,
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "a source image set nothing holds",
            datastore: Named::Held,
            source: Named::Unheld,
            claimed_source_version: "2",
            destination: None,
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "a destination image set nothing holds",
            datastore: Named::Held,
            source: Named::Held,
            claimed_source_version: "2",
            destination: Some((Named::Unheld, "1")),
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "a source version one BELOW the latest",
            datastore: Named::Held,
            source: Named::Held,
            claimed_source_version: "1",
            destination: None,
            expected_status: 409,
            expected_code: "ConflictException",
        },
        RefusalCase {
            case: "a source version one ABOVE the latest",
            datastore: Named::Held,
            source: Named::Held,
            claimed_source_version: "3",
            destination: None,
            expected_status: 409,
            expected_code: "ConflictException",
        },
        RefusalCase {
            case: "a source version far past the latest",
            datastore: Named::Held,
            source: Named::Held,
            claimed_source_version: "4096",
            destination: None,
            expected_status: 409,
            expected_code: "ConflictException",
        },
        RefusalCase {
            case: "a destination version below the destination's latest",
            datastore: Named::Held,
            source: Named::Held,
            claimed_source_version: "2",
            destination: Some((Named::Held, "1")),
            expected_status: 409,
            expected_code: "ConflictException",
        },
        RefusalCase {
            case: "a destination version above the destination's latest",
            datastore: Named::Held,
            source: Named::Held,
            claimed_source_version: "2",
            destination: Some((Named::Held, "3")),
            expected_status: 409,
            expected_code: "ConflictException",
        },
    ]
}

/// An absent resource, and a version that is not the one held, are the modelled
/// failures — and the state is left exactly as it was.
#[tokio::test]
async fn a_copy_that_the_rule_does_not_admit_is_refused() {
    for case in refusal_cases() {
        let emulator = Emulator::new();
        let held_datastore = emulator.active_datastore("refusal.store", "refusal").await;
        // Two versions each, so a claim can be made one below and one above the latest.
        let held_source = seed_image_set(&emulator, &held_datastore, 2);
        let held_destination = seed_image_set(&emulator, &held_datastore, 2);
        let unheld = unheld_id();

        let datastore_id = match case.datastore {
            Named::Held => held_datastore.clone(),
            Named::Unheld => unheld.clone(),
        };
        let source_id = match case.source {
            Named::Held => held_source.clone(),
            Named::Unheld => unheld.clone(),
        };
        let destination = case.destination.map(|(named, version)| {
            let id = match named {
                Named::Held => held_destination.clone(),
                Named::Unheld => unheld.clone(),
            };
            (id, version)
        });

        let refused = copy_image_set(
            &emulator,
            &datastore_id,
            &source_id,
            information(
                case.claimed_source_version,
                None,
                destination
                    .as_ref()
                    .map(|(id, version)| (id.as_str(), *version)),
            ),
            &[],
        )
        .await;

        assert_eq!(
            refused.status, case.expected_status,
            "case {:?}: expected HTTP {}, got {refused:?}",
            case.case, case.expected_status
        );
        assert_eq!(
            refused.error_code, case.expected_code,
            "case {:?}: expected {}, got {refused:?}",
            case.case, case.expected_code
        );

        // A refusal writes nothing: both image sets are still ACTIVE at version 2.
        for (role, image_set_id) in [("source", &held_source), ("destination", &held_destination)] {
            let read = get_image_set(&emulator, &held_datastore, image_set_id, None).await;
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
        }
    }
}

/// An image set already LOCKED by a copy in flight cannot take part in another one —
/// as source or as destination — until the clock releases it.
#[tokio::test]
async fn an_image_set_locked_by_a_copy_in_flight_is_a_conflict() {
    // Rows: the locked image set in each of the two roles the next copy could give it.
    for role in ["source", "destination"] {
        let emulator = Emulator::new();
        let datastore_id = emulator.active_datastore("locked.store", "locked").await;
        let first_source = seed_image_set(&emulator, &datastore_id, 1);
        let bystander = seed_image_set(&emulator, &datastore_id, 1);

        let started = Instant::now();
        let first = copy_image_set(
            &emulator,
            &datastore_id,
            &first_source,
            information("1", None, None),
            &[],
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case role={role}: expected HTTP 200 from the first copy, got {first:?}"
        );
        let locked = first.string(&["destinationImageSetProperties", "imageSetId"]);

        let (second_source, destination) = match role {
            "source" => (locked.clone(), None),
            _ => (bystander.clone(), Some((locked.as_str(), "1"))),
        };
        let refused = copy_image_set(
            &emulator,
            &datastore_id,
            &second_source,
            information("1", None, destination),
            &[],
        )
        .await;
        // Only meaningful while the copy is still in flight; the span is never zero,
        // so a call made straight after the first copy is.
        assert!(
            started.elapsed() < IMAGE_SET_COPY_DELAY,
            "case role={role}: the copy was already due before the second call ran"
        );
        assert_eq!(
            refused.status, 409,
            "case role={role}: a LOCKED image set is a conflict, got {refused:?}"
        );
        assert_eq!(
            refused.error_code, "ConflictException",
            "case role={role}: wrong error code, got {refused:?}"
        );

        // Once the clock has released it, the same call is admitted.
        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY).await;
        let admitted = copy_image_set(
            &emulator,
            &datastore_id,
            &second_source,
            information("1", None, destination),
            &[],
        )
        .await;
        assert_eq!(
            admitted.status, 200,
            "case role={role}: expected HTTP 200 once the lock is released, got {admitted:?}"
        );
    }
}

/// A destination naming the source itself cannot hold together, and is refused as the
/// input-constraint failure it is.
#[tokio::test]
async fn a_destination_naming_the_source_is_refused() {
    let emulator = Emulator::new();
    let datastore_id = emulator.active_datastore("self.store", "self").await;

    // Rows: the self-naming request at one and at three versions, and with the version
    // claimed correctly and incorrectly — none of them is admitted.
    for (versions, claimed) in [(1_u32, "1"), (3, "3"), (3, "1")] {
        let image_set_id = seed_image_set(&emulator, &datastore_id, versions);
        let refused = copy_image_set(
            &emulator,
            &datastore_id,
            &image_set_id,
            information(claimed, None, Some((image_set_id.as_str(), claimed))),
            &[],
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case versions={versions} claimed={claimed}: expected HTTP 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_code, "ValidationException",
            "case versions={versions} claimed={claimed}: wrong error code, got {refused:?}"
        );
        let message = refused.string(&["message"]);
        assert!(
            message.contains("copyImageSetInformation.destinationImageSet.imageSetId"),
            "case versions={versions} claimed={claimed}: the refusal must name the member, got {message:?}"
        );

        let read = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
        assert_eq!(
            read.string(&["imageSetState"]),
            "ACTIVE",
            "case versions={versions} claimed={claimed}: the image set must be untouched, got {read:?}"
        );
    }
}

// Which JSON forms `copiableAttributes` admits, and the refusal every other form draws,
// is the rule `crate::metadata_document_shape` owns: that member and the two
// `UpdateImageSetMetadata` blobs beside it are documented as carrying the same kind of
// document, so one suite drives the rule over all three.

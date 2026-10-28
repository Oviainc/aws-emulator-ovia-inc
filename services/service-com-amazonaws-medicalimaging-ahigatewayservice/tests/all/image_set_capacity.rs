// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what a data store's capacity for image sets counts, and for the
//! work the image sets of a data store at large take part in.
//!
//! RULE: a bound on how many of a resource a container holds, and work that reaches the
//! resources a container holds, count only the resources the container can still serve;
//! the tombstone of a landed deletion has given up what it held, so it takes no place and
//! takes part in no further work, and a deletion gives its place back. Here that is the
//! per-data-store cap on image sets `CopyImageSet` refuses a mint past
//! (`MAX_IMAGE_SETS_PER_DATASTORE`, the modelled `ServiceQuotaExceededException`, HTTP
//! 402) and the study a `includeStudyImageSets` update is applied across. What a
//! tombstone does keep is its RECORD, which is still reported, so the records one data
//! store holds are bounded in their own right (`MAX_IMAGE_SET_RECORDS_PER_DATASTORE`) and
//! a mint past THAT is refused naming the records.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: how many
//! image sets the data store still holds (none beyond the copy's source, one below the
//! cap, exactly at it), how many tombstones sit beside them (none, a handful, more than
//! the cap itself), and whether the request mints a destination or versions an image set
//! that already exists — which is what shows the cap is about coming into existence.
//! Every case drives real HTTP through the production front door, so what is asserted is
//! the wire behaviour a `medical-imaging` SDK client sees.
//!
//! ## Why a seeded image set
//!
//! In AWS an image set enters a data store through a DICOM import, which this emulator's
//! import job does not do yet
//! (`issues/things-that-do-not-work/20261024T002353Z-medical-imaging-startdicomimportjob-event-propagation.md`),
//! so each case seeds its image sets through the crate's public `state` module with the
//! same `ImageSet` API the handlers use, and then asserts only over the wire. Every
//! tombstone, though, is made the way a caller makes one: `DeleteImageSet` accepted over
//! HTTP and landed on `DELETED` by the clock.

use serde_json::json;

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    IMAGE_SET_COPY_DELAY, IMAGE_SET_DELETION_DELAY, MAX_IMAGE_SETS_PER_DATASTORE,
    MAX_IMAGE_SET_RECORDS_PER_DATASTORE,
};

use crate::datastore_lifecycle::{Emulator, SETTLE_MULTIPLE};
use crate::image_set_copy::{copy_image_set, get_image_set, information, seed_image_set};
use crate::image_set_deletion::delete_image_set;
use crate::image_set_metadata_updates::{
    dicom_updates, seed_image_set as seed_image_set_with_tags, seeded_tags, update_metadata,
    STUDY_UID,
};
use crate::image_set_search::search;

/// The refusal text for a mint past the cap on the image sets one data store HOLDS.
fn held_cap_message() -> String {
    format!("this emulator holds at most {MAX_IMAGE_SETS_PER_DATASTORE} image sets per data store")
}

/// The refusal text for a mint past the cap on the image set RECORDS one data store holds.
fn record_cap_message() -> String {
    format!(
        "this emulator holds at most {MAX_IMAGE_SET_RECORDS_PER_DATASTORE} image set records per \
         data store"
    )
}

/// Seed `count` live `ACTIVE` image sets, each at one version, and answer their
/// identifiers in the order they were seeded.
fn seed_live(emulator: &Emulator, datastore_id: &str, count: usize) -> Vec<String> {
    (0..count)
        .map(|_| seed_image_set(emulator, datastore_id, 1))
        .collect()
}

/// Seed `count` image sets and RETIRE every one of them the way a caller does:
/// `DeleteImageSet` over HTTP, then the clock landing the deletion on `DELETED`. Answers
/// the identifiers of the records left behind, which are tombstones.
///
/// Every delete is issued before the one wait, so the whole batch falls due inside it.
async fn seed_tombstones(
    emulator: &Emulator,
    datastore_id: &str,
    case: &str,
    count: usize,
) -> Vec<String> {
    let retired = seed_live(emulator, datastore_id, count);
    for image_set_id in &retired {
        let accepted = delete_image_set(emulator, datastore_id, image_set_id).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected HTTP 200 from DeleteImageSet, got {accepted:?}"
        );
    }
    if !retired.is_empty() {
        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_DELETION_DELAY).await;
    }
    for image_set_id in &retired {
        let read = get_image_set(emulator, datastore_id, image_set_id, None).await;
        assert_eq!(
            read.string(&["imageSetState"]),
            "DELETED",
            "case {case:?}: expected the retired image set on DELETED, got {read:?}"
        );
    }
    retired
}

/// One row of the capacity table.
#[derive(Debug)]
struct Case {
    case: &'static str,
    /// How many image sets the data store still HOLDS when the copy arrives — the copy's
    /// source among them, so this is never zero.
    held: usize,
    /// How many tombstones sit beside them.
    tombstones: usize,
    /// Whether the request names a destination image set (so the copy VERSIONS one that
    /// already exists) instead of leaving the copy to mint one.
    versions_destination: bool,
    expected_status: u16,
}

/// The rows span: a data store holding nothing but the source; one image set below the
/// cap; exactly at the cap; tombstones absent, a handful, as many as the cap and more
/// than the cap; and a request that versions an existing destination rather than minting
/// one.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "nothing held beyond the source, no tombstone",
            held: 1,
            tombstones: 0,
            versions_destination: false,
            expected_status: 200,
        },
        Case {
            case: "one place left under the cap",
            held: MAX_IMAGE_SETS_PER_DATASTORE - 1,
            tombstones: 0,
            versions_destination: false,
            expected_status: 200,
        },
        Case {
            case: "the cap reached by image sets the store holds",
            held: MAX_IMAGE_SETS_PER_DATASTORE,
            tombstones: 0,
            versions_destination: false,
            expected_status: 402,
        },
        Case {
            case: "the cap reached, with a handful of tombstones beside it",
            held: MAX_IMAGE_SETS_PER_DATASTORE,
            tombstones: 7,
            versions_destination: false,
            expected_status: 402,
        },
        Case {
            case: "the cap reached, versioning an existing destination instead of minting",
            held: MAX_IMAGE_SETS_PER_DATASTORE,
            tombstones: 3,
            versions_destination: true,
            expected_status: 200,
        },
        Case {
            case: "as many tombstones as the cap, one image set held",
            held: 1,
            tombstones: MAX_IMAGE_SETS_PER_DATASTORE,
            versions_destination: false,
            expected_status: 200,
        },
        Case {
            case: "more tombstones than the cap, a few image sets held",
            held: 3,
            tombstones: MAX_IMAGE_SETS_PER_DATASTORE + 5,
            versions_destination: false,
            expected_status: 200,
        },
        Case {
            case: "one place left under the cap, with more tombstones than the cap",
            held: MAX_IMAGE_SETS_PER_DATASTORE - 1,
            tombstones: MAX_IMAGE_SETS_PER_DATASTORE + 1,
            versions_destination: false,
            expected_status: 200,
        },
    ]
}

/// RULE: the cap on how many image sets one data store holds counts the image sets it
/// still HOLDS — a tombstone takes no place, so a deletion gives its place back — and it
/// bounds only a copy that MINTS its destination: a copy into an image set that already
/// exists adds none.
#[tokio::test]
async fn the_image_set_cap_counts_only_the_image_sets_the_data_store_still_holds() {
    let emulator = Emulator::new();

    for (index, case) in cases().into_iter().enumerate() {
        assert!(
            case.held >= 1,
            "case {:?}: the copy needs a live source, so a row holds at least one image set",
            case.case
        );
        let datastore_id = emulator
            .active_datastore(
                &format!("capacity.{index}"),
                &format!("token-capacity-{index}"),
            )
            .await;

        // The tombstones first, so the places they would take if they were counted are
        // the ones the live image sets below go on to fill.
        let retired = seed_tombstones(&emulator, &datastore_id, case.case, case.tombstones).await;
        let held = seed_live(&emulator, &datastore_id, case.held);
        let source = held[0].clone();

        // What the data store REPORTS it holds: the live image sets, never a tombstone.
        // The first page is as large as the service serves, which is what bounds how many
        // of them one answer can carry.
        let reported = search(&emulator, &datastore_id, None, "maxResults=50").await;
        assert_eq!(
            reported.status, 200,
            "case {:?}: expected HTTP 200 from SearchImageSets, got {reported:?}",
            case.case
        );
        let reported_ids: Vec<String> = reported
            .field(&["imageSetsMetadataSummaries"])
            .as_array()
            .map(|summaries| {
                summaries
                    .iter()
                    .map(|summary| summary["imageSetId"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            reported_ids.len(),
            case.held.min(50),
            "case {:?}: expected the live image sets reported, got {reported:?}",
            case.case
        );
        for reported_id in &reported_ids {
            assert!(
                !retired.contains(reported_id),
                "case {:?}: expected no tombstone reported, got {reported_id:?} among \
                 {reported_ids:?}",
                case.case
            );
        }

        let destination = if case.versions_destination {
            assert!(
                case.held >= 2,
                "case {:?}: versioning a destination needs a second live image set",
                case.case
            );
            Some(held[1].clone())
        } else {
            None
        };
        let answer = copy_image_set(
            &emulator,
            &datastore_id,
            &source,
            information(
                "1",
                None,
                destination.as_ref().map(|id| (id.as_str(), "1")),
            ),
            &[],
        )
        .await;

        assert_eq!(
            answer.status, case.expected_status,
            "case {:?}: expected HTTP {}, got {answer:?}",
            case.case, case.expected_status
        );
        if case.expected_status == 402 {
            assert_eq!(
                answer.error_code, "ServiceQuotaExceededException",
                "case {:?}: expected ServiceQuotaExceededException, got {answer:?}",
                case.case
            );
            assert_eq!(
                answer.string(&["message"]),
                held_cap_message(),
                "case {:?}: expected {:?}, got {answer:?}",
                case.case,
                held_cap_message()
            );
            // A refusal mints nothing: what the store holds is what it held going in.
            let unchanged = search(&emulator, &datastore_id, None, "maxResults=50").await;
            assert_eq!(
                unchanged.field(&["imageSetsMetadataSummaries"]).as_array().map(Vec::len),
                Some(case.held.min(50)),
                "case {:?}: the refused copy must leave the data store as it was, got \
                 {unchanged:?}",
                case.case
            );
            continue;
        }

        // The copy was admitted, so the destination it reports is readable: a minted one
        // at its first version, an existing one at the version the copy pushed onto it.
        let destination_id = answer.string(&["destinationImageSetProperties", "imageSetId"]);
        match &destination {
            None => {
                assert!(
                    !held.contains(&destination_id) && !retired.contains(&destination_id),
                    "case {:?}: expected a freshly minted destination, got {destination_id:?}",
                    case.case
                );
            }
            Some(named) => {
                assert_eq!(
                    &destination_id, named,
                    "case {:?}: expected the named destination reported, got {answer:?}",
                    case.case
                );
            }
        }
        let read = get_image_set(&emulator, &datastore_id, &destination_id, None).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected the destination readable, got {read:?}",
            case.case
        );
        let expected_version = if destination.is_some() { "2" } else { "1" };
        assert_eq!(
            read.string(&["versionId"]),
            expected_version,
            "case {:?}: expected the destination at version {expected_version}, got {read:?}",
            case.case
        );
    }
}

/// RULE: a tombstone gives up its place among the image sets a data store holds but keeps
/// its RECORD, which is still reported, so the records are bounded in their own right: a
/// mint is refused naming the records once the data store holds that many, and admitted
/// one record below.
///
/// The two rows differ along where the record count stands: one below the cap, and at it.
#[tokio::test]
async fn the_record_cap_counts_every_image_set_record_including_the_tombstones() {
    let emulator = Emulator::new();

    // One row below the cap, and at it. The live image set each store holds is the copy's
    // source, so the cap on what a store HOLDS is nowhere near reached and the records are
    // what decides.
    for (case, records) in [
        ("one record below the cap", MAX_IMAGE_SET_RECORDS_PER_DATASTORE - 1),
        ("the cap reached", MAX_IMAGE_SET_RECORDS_PER_DATASTORE),
    ] {
        let datastore_id = emulator
            .active_datastore(
                &format!("records.{records}"),
                &format!("token-records-{records}"),
            )
            .await;
        let retired = seed_tombstones(&emulator, &datastore_id, case, records - 1).await;
        assert_eq!(
            retired.len(),
            records - 1,
            "case {case:?}: expected {} tombstones seeded, got {}",
            records - 1,
            retired.len()
        );
        let source = seed_image_set(&emulator, &datastore_id, 1);

        let answer = copy_image_set(
            &emulator,
            &datastore_id,
            &source,
            information("1", None, None),
            &[],
        )
        .await;

        if records < MAX_IMAGE_SET_RECORDS_PER_DATASTORE {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200 one record below the cap, got {answer:?}"
            );
            // The mint took the records to the cap, so the NEXT one is refused. The copy
            // just admitted holds its source read-only until the clock completes it, so
            // the next one waits for that rather than reading a state error instead.
            tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY).await;
            let refused = copy_image_set(
                &emulator,
                &datastore_id,
                &source,
                information("1", None, None),
                &[],
            )
            .await;
            assert_eq!(
                refused.status, 402,
                "case {case:?}: expected HTTP 402 at the cap, got {refused:?}"
            );
            assert_eq!(
                refused.string(&["message"]),
                record_cap_message(),
                "case {case:?}: expected {:?}, got {refused:?}",
                record_cap_message()
            );
            continue;
        }

        assert_eq!(
            answer.status, 402,
            "case {case:?}: expected HTTP 402 at the cap, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ServiceQuotaExceededException",
            "case {case:?}: expected ServiceQuotaExceededException, got {answer:?}"
        );
        assert_eq!(
            answer.string(&["message"]),
            record_cap_message(),
            "case {case:?}: expected {:?}, got {answer:?}",
            record_cap_message()
        );
    }
}

/// RULE: the work that reaches the image sets of a data store at large reaches the ones it
/// still HOLDS. A study-wide metadata update is one such walk: a retired image set of the
/// same study takes part in no further work, so it neither takes the update nor blocks it,
/// while every image set the store still holds in that study takes it.
#[tokio::test]
async fn a_retired_image_set_of_the_study_neither_takes_a_study_wide_update_nor_blocks_it() {
    // The rows differ along how many image sets of the study are retired, and how many are
    // still held beside the one the request names.
    for (case, held_siblings, retired_siblings) in [
        ("one retired sibling, one held", 1usize, 1usize),
        ("every sibling retired", 0, 2),
        ("two retired siblings, two held", 2, 2),
    ] {
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore(
                &format!("study.retired.{held_siblings}.{retired_siblings}"),
                &format!("token-study-retired-{held_siblings}-{retired_siblings}"),
            )
            .await;

        let named = seed_image_set_with_tags(&emulator, &datastore_id, seeded_tags(STUDY_UID));
        let held: Vec<String> = (0..held_siblings)
            .map(|_| seed_image_set_with_tags(&emulator, &datastore_id, seeded_tags(STUDY_UID)))
            .collect();
        let retired: Vec<String> = (0..retired_siblings)
            .map(|_| seed_image_set_with_tags(&emulator, &datastore_id, seeded_tags(STUDY_UID)))
            .collect();
        for image_set_id in &retired {
            let accepted = delete_image_set(&emulator, &datastore_id, image_set_id).await;
            assert_eq!(
                accepted.status, 200,
                "case {case:?}: expected HTTP 200 from DeleteImageSet, got {accepted:?}"
            );
        }
        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_DELETION_DELAY).await;

        let document = json!({
            "SchemaVersion": "1.1",
            "Patient": { "DICOM": { "PatientName": "ROE^RICHARD" } },
        });
        let updated = update_metadata(
            &emulator,
            &datastore_id,
            &named,
            "1",
            dicom_updates(Some(&document), None),
            &[("includeStudyImageSets", true)],
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 with a retired image set in the study, got \
             {updated:?}"
        );
        assert_eq!(
            updated.string(&["latestVersionId"]),
            "2",
            "case {case:?}: expected the named image set at version 2, got {updated:?}"
        );

        // Every image set the store still holds in the study took the update.
        for sibling in &held {
            let read = get_image_set(&emulator, &datastore_id, sibling, None).await;
            assert_eq!(
                (
                    read.string(&["versionId"]),
                    read.string(&["imageSetWorkflowStatus"]),
                ),
                (
                    "2".to_string(),
                    "UPDATING_FOR_STUDY_CONSISTENCY".to_string()
                ),
                "case {case:?}: expected the held sibling brought into line, got {read:?}"
            );
        }

        // A tombstone took no part: no version was pushed onto it and it is still retired.
        for tombstone in &retired {
            let read = get_image_set(&emulator, &datastore_id, tombstone, None).await;
            assert_eq!(
                (
                    read.string(&["versionId"]),
                    read.string(&["imageSetState"]),
                ),
                ("1".to_string(), "DELETED".to_string()),
                "case {case:?}: expected the retired sibling untouched and still DELETED, got \
                 {read:?}"
            );
        }
    }
}

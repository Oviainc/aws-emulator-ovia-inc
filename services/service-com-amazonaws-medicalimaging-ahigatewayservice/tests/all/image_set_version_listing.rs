// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListImageSetVersions`.
//!
//! RULE: a paginated listing reports exactly the records the emulator PERSISTED for the
//! collection the request names — here every VERSION of one image set — in a stable total
//! order (the external version identifier, ascending, compared as a number), one
//! `maxResults`-sized page at a time, with an opaque token that resumes the walk exactly
//! where the previous page stopped; each record reports the CURRENT values the record's
//! own reader reports for it (`GetImageSet` for that version). A request naming a
//! collection the emulator does not hold is refused with the modelled not-found error
//! rather than answered with an empty page, a token this service did not issue for that
//! collection is refused rather than honoured, and the listing — a reporting operation —
//! changes nothing a later caller sees.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: how many
//! versions the image set holds (including counts either side of the nine-to-ten boundary,
//! where a lexicographic order would invert the identifiers), page size (both published
//! `maxResults` bounds, one past each, and sizes either side of the collection), which
//! resource the request gets wrong, what the supplied token is, and whether a version was
//! created with the force override. Every case drives real HTTP through the production
//! front door (`crate::datastore_lifecycle`'s harness), so what is asserted is the wire
//! behaviour a `medical-imaging` SDK client sees.
//!
//! ## Why a seeded image set
//!
//! In AWS an image set comes into existence through a DICOM import, which this emulator
//! does not create image sets from yet, so the suites that need an image set at a given
//! version count seed it exactly as `tests/all/image_set_copy.rs` does — through the
//! crate's own `ImageSet::new` / `push_version`. The last suite builds its versions the
//! other way, over HTTP through `CopyImageSet`, so the listing is also proved against
//! versions a handler minted.

use serde_json::Value;

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    FIRST_IMAGE_SET_VERSION_ID, IMAGE_SET_COPY_DELAY, MAX_LIST_PAGE_SIZE, MIN_LIST_PAGE_SIZE,
};

use crate::datastore_lifecycle::{Answer, Emulator, SETTLE_MULTIPLE};
use crate::image_set_copy::{copy_image_set, information, seed_image_set};

/// A `RoleArn` the import-job submits below use (len 20..2048,
/// `^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/ImportRole";

/// The service's constant text for a data store it does not hold.
const DATASTORE_NOT_FOUND: &str = "Datastore resource is not found.";

/// The service's constant text for an image set a data store does not hold.
const IMAGE_SET_NOT_FOUND: &str = "ImageSet resource is not found.";

/// The service's fixed sentence for a `nextToken` it did not issue.
const INVALID_NEXT_TOKEN: &str =
    "Failed to perform requested operation due to invalid NextToken. Please try again.";

/// A well-formed `[0-9a-z]{32}` resource id no call ever mints (the ids the service mints
/// are SplitMix64 output, never all zeros).
fn unheld_id() -> String {
    "0".repeat(32)
}

/// `POST /datastore/{datastoreId}/imageSet/{imageSetId}/listImageSetVersions` with the
/// query string `query`
/// (`smithy_query(query="operation",operation="ListImageSetVersions")`).
async fn list_versions(
    emulator: &Emulator,
    datastore_id: &str,
    image_set_id: &str,
    query: &str,
) -> Answer {
    let mut uri = format!("/datastore/{datastore_id}/imageSet/{image_set_id}/listImageSetVersions");
    if !query.is_empty() {
        uri.push('?');
        uri.push_str(query);
    }
    emulator.call("POST", &uri, None).await
}

/// `POST /datastore/{datastoreId}/imageSet/{imageSetId}/getImageSet` — the reader one
/// listed version is checked against (`version` is an `@httpQuery`).
async fn get_image_set(
    emulator: &Emulator,
    datastore_id: &str,
    image_set_id: &str,
    version_id: &str,
) -> Answer {
    let uri =
        format!("/datastore/{datastore_id}/imageSet/{image_set_id}/getImageSet?version={version_id}");
    emulator.call("POST", &uri, None).await
}

/// The `imageSetPropertiesList` of one answer, as JSON objects.
fn properties(answer: &Answer) -> Vec<Value> {
    answer
        .field(&["imageSetPropertiesList"])
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `versionId` of every entry in one answer, in the order served.
fn listed_versions(answer: &Answer) -> Vec<String> {
    properties(answer)
        .iter()
        .map(|entry| entry["versionId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The version identifiers an image set holding `versions` versions must report, oldest
/// first.
fn expected_versions(versions: u32) -> Vec<String> {
    (FIRST_IMAGE_SET_VERSION_ID..FIRST_IMAGE_SET_VERSION_ID.saturating_add(versions))
        .map(|version| version.to_string())
        .collect()
}

/// The members a listed version and `GetImageSet` must agree on, listed as
/// (`ImageSetProperties` member, `GetImageSetResponse` member) — the two shapes spell the
/// workflow status differently
/// (`smithy_query(query="shape",shape="ImageSetProperties,GetImageSetResponse")`).
const AGREEING_MEMBERS: &[(&str, &str)] = &[
    ("imageSetId", "imageSetId"),
    ("versionId", "versionId"),
    ("imageSetState", "imageSetState"),
    ("ImageSetWorkflowStatus", "imageSetWorkflowStatus"),
    ("createdAt", "createdAt"),
    ("updatedAt", "updatedAt"),
    ("deletedAt", "deletedAt"),
    ("message", "message"),
    ("overrides", "overrides"),
    ("isPrimary", "isPrimary"),
];

/// RULE: the listing reports every version the image set holds, oldest first, and each
/// listed version reports what that version's own reader reports for it.
///
/// Rows span how many versions the image set holds, including counts either side of the
/// nine-to-ten boundary, where ordering the identifiers as text rather than as numbers
/// would put version 10 before version 9.
#[tokio::test]
async fn every_persisted_version_is_listed_oldest_first_and_agrees_with_its_own_reader() {
    for versions in [1_u32, 2, 3, 9, 12] {
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore("versions.store", &format!("versions-{versions}"))
            .await;
        let image_set_id = seed_image_set(&emulator, &datastore_id, versions);

        let listed = list_versions(&emulator, &datastore_id, &image_set_id, "").await;
        assert_eq!(
            listed.status, 200,
            "case versions={versions}: expected HTTP 200, got {listed:?}"
        );
        assert_eq!(
            listed_versions(&listed),
            expected_versions(versions),
            "case versions={versions}: expected every version oldest first, got {listed:?}"
        );
        assert!(
            listed.field(&["nextToken"]).is_null(),
            "case versions={versions}: expected no nextToken on a page that exhausts the collection, got {listed:?}"
        );

        for entry in properties(&listed) {
            let version_id = entry["versionId"].as_str().unwrap_or_default().to_string();
            let read = get_image_set(&emulator, &datastore_id, &image_set_id, &version_id).await;
            assert_eq!(
                read.status, 200,
                "case versions={versions} version={version_id}: expected HTTP 200 from GetImageSet, got {read:?}"
            );
            for (listed_member, read_member) in AGREEING_MEMBERS {
                let listed_value = &entry[*listed_member];
                let read_value = read.field(&[*read_member]);
                assert_eq!(
                    listed_value, read_value,
                    "case versions={versions} version={version_id}: expected {listed_member} to equal GetImageSet's {read_member} {read_value:?}, got {listed_value:?}"
                );
            }
        }
    }
}

/// RULE: paging walks the whole collection in that one order — every version exactly
/// once, never more than `maxResults` in a page, and no token on the page that exhausts
/// it.
///
/// Rows span page size against collection size: the published `maxResults` bounds, a page
/// smaller than the collection, one exactly its size, and one larger.
#[tokio::test]
async fn paging_serves_every_version_exactly_once_in_order() {
    // `maxResults` is range 1..50
    // (`smithy_query(query="operation",operation="ListImageSetVersions")`), which
    // `MIN_LIST_PAGE_SIZE` / `MAX_LIST_PAGE_SIZE` name.
    let cases: Vec<(u32, usize)> = vec![
        (1, MIN_LIST_PAGE_SIZE),
        (12, MIN_LIST_PAGE_SIZE),
        (12, 5),
        (12, 11),
        (12, 12),
        (12, 13),
        (12, MAX_LIST_PAGE_SIZE),
    ];

    for (versions, page_size) in cases {
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore("paging.store", &format!("paging-{versions}-{page_size}"))
            .await;
        let image_set_id = seed_image_set(&emulator, &datastore_id, versions);

        let mut served: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            pages += 1;
            assert!(
                pages <= 64,
                "case versions={versions} maxResults={page_size}: paging did not terminate after {pages} pages"
            );
            let mut query = format!("maxResults={page_size}");
            if let Some(value) = &token {
                query.push_str(&format!("&nextToken={value}"));
            }
            let page = list_versions(&emulator, &datastore_id, &image_set_id, &query).await;
            assert_eq!(
                page.status, 200,
                "case versions={versions} maxResults={page_size}: expected HTTP 200, got {page:?}"
            );
            let on_this_page = listed_versions(&page);
            assert!(
                on_this_page.len() <= page_size,
                "case versions={versions} maxResults={page_size}: a page served {} versions, got {page:?}",
                on_this_page.len()
            );
            served.extend(on_this_page);
            token = page.field(&["nextToken"]).as_str().map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            served,
            expected_versions(versions),
            "case versions={versions} maxResults={page_size}: expected the walk to serve every version exactly once in order, got {served:?}"
        );
    }
}

/// RULE: `maxResults` is refused outside the range the model publishes, and served on
/// each of its bounds.
#[tokio::test]
async fn max_results_outside_the_published_range_is_refused() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("bounds.store", "bounds-page-size")
        .await;
    let image_set_id = seed_image_set(&emulator, &datastore_id, 3);

    // The published bounds of `maxResults` (range 1..50) and one past each
    // (`smithy_query(query="operation",operation="ListImageSetVersions")`).
    for (requested, expected_status) in [("0", 400_u16), ("1", 200), ("50", 200), ("51", 400)] {
        let answer = list_versions(
            &emulator,
            &datastore_id,
            &image_set_id,
            &format!("maxResults={requested}"),
        )
        .await;
        assert_eq!(
            answer.status, expected_status,
            "case maxResults={requested}: expected HTTP {expected_status}, got {answer:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                answer.error_code, "ValidationException",
                "case maxResults={requested}: expected ValidationException, got {answer:?}"
            );
        }
    }
}

/// RULE: a request naming a collection the emulator does not hold is refused with the
/// modelled not-found error, stated by resource KIND, rather than answered with an empty
/// page.
///
/// Rows span which resource of the pair is absent.
#[tokio::test]
async fn an_absent_datastore_or_image_set_is_refused_rather_than_answered_empty() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("absent.store", "absent-1")
        .await;
    let image_set_id = seed_image_set(&emulator, &datastore_id, 2);

    let cases: Vec<(&str, String, String, &str)> = vec![
        (
            "a data store the service does not hold",
            unheld_id(),
            image_set_id.clone(),
            DATASTORE_NOT_FOUND,
        ),
        (
            "an image set the data store does not hold",
            datastore_id.clone(),
            unheld_id(),
            IMAGE_SET_NOT_FOUND,
        ),
    ];

    for (case, datastore, image_set, expected_message) in cases {
        let answer = list_versions(&emulator, &datastore, &image_set, "").await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.string(&["message"]),
            expected_message,
            "case {case:?}: expected the service's constant text for the missing resource, got {answer:?}"
        );
    }
}

/// RULE: a continuation token is honoured only where this service issued it — for THIS
/// collection; anything else is refused with the service's one fixed sentence rather than
/// honoured as a resume point.
///
/// Rows span the token forms a caller can supply: one that is not this service's encoding
/// at all, one that is but carries no position this service minted, one minted for
/// ANOTHER image set's versions, and one minted for a different collection entirely
/// (the data store's import jobs).
#[tokio::test]
async fn a_token_this_listing_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    let datastore_id = emulator.active_datastore("token.store", "token-1").await;
    let listed_image_set = seed_image_set(&emulator, &datastore_id, 4);
    let other_image_set = seed_image_set(&emulator, &datastore_id, 4);

    // A token minted against ANOTHER image set's versions: a real token, for the wrong
    // collection.
    let other_page = list_versions(&emulator, &datastore_id, &other_image_set, "maxResults=1").await;
    let other_collection_token = other_page
        .field(&["nextToken"])
        .as_str()
        .expect("a second page of the other image set's versions")
        .to_string();

    // A token minted by a different listing of the same data store: `ListDICOMImportJobs`.
    for index in 0..2 {
        let started = emulator
            .start_import_job(
                &datastore_id,
                serde_json::json!({
                    "clientToken": format!("token-job-{index}"),
                    "dataAccessRoleArn": ROLE_ARN,
                    "inputS3Uri": "s3://imaging-in/",
                    "outputS3Uri": "s3://imaging-out/",
                }),
            )
            .await;
        assert_eq!(
            started.status, 200,
            "case \"import job {index}\": expected HTTP 200 from StartDICOMImportJob, got {started:?}"
        );
    }
    let jobs_page = emulator
        .call(
            "GET",
            &format!("/listDICOMImportJobs/datastore/{datastore_id}?maxResults=1"),
            None,
        )
        .await;
    let other_listing_token = jobs_page
        .field(&["nextToken"])
        .as_str()
        .expect("a second page of the data store's import jobs")
        .to_string();

    let cases: Vec<(&str, String)> = vec![
        ("not this service's encoding", "not-a-token".to_string()),
        (
            "well-formed hex holding no position",
            ::hex::encode("some other payload"),
        ),
        ("another image set's versions", other_collection_token),
        ("another collection of the same data store", other_listing_token),
    ];

    for (case, token) in cases {
        let answer = list_versions(
            &emulator,
            &datastore_id,
            &listed_image_set,
            &format!("nextToken={token}"),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.string(&["message"]),
            INVALID_NEXT_TOKEN,
            "case {case:?}: expected the service's fixed sentence for an invalid token, got {answer:?}"
        );
    }
}

/// RULE: the listing is a reporting operation — it changes nothing a later caller sees,
/// so repeating it answers the same page and the version's own reader still reports what
/// it reported before.
///
/// Rows span how many versions the image set holds, so a one-version and a many-version
/// collection are both pinned.
#[tokio::test]
async fn listing_versions_changes_nothing_a_later_caller_sees() {
    for versions in [1_u32, 5] {
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore("stable.store", &format!("stable-{versions}"))
            .await;
        let image_set_id = seed_image_set(&emulator, &datastore_id, versions);

        let before = get_image_set(
            &emulator,
            &datastore_id,
            &image_set_id,
            &versions.to_string(),
        )
        .await;
        let first = list_versions(&emulator, &datastore_id, &image_set_id, "").await;
        let second = list_versions(&emulator, &datastore_id, &image_set_id, "").await;
        let after = get_image_set(
            &emulator,
            &datastore_id,
            &image_set_id,
            &versions.to_string(),
        )
        .await;

        assert_eq!(
            properties(&first),
            properties(&second),
            "case versions={versions}: expected two listings to report the same versions, got {first:?} then {second:?}"
        );
        assert_eq!(
            before.body, after.body,
            "case versions={versions}: expected the version's own reader to be unchanged by the listing, got {:?} then {:?}",
            before.body, after.body
        );
    }
}

/// RULE: the versions a handler mints are the versions the listing reports — a copy that
/// lands on an image set appears as the next version, carrying the overrides that copy
/// asked for and no others.
#[tokio::test]
async fn versions_a_copy_lands_appear_in_the_listing_with_that_version_s_overrides() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("copy.listing.store", "copy-listing-1")
        .await;
    let source_id = seed_image_set(&emulator, &datastore_id, 1);

    // A copy naming no destination mints an image set at its first version, copied
    // WITHOUT the force override.
    let minted = copy_image_set(
        &emulator,
        &datastore_id,
        &source_id,
        information("1", None, None),
        &[],
    )
    .await;
    assert_eq!(
        minted.status, 200,
        "case \"mint the destination\": expected HTTP 200 from CopyImageSet, got {minted:?}"
    );
    let destination_id = minted.string(&["destinationImageSetProperties", "imageSetId"]);
    tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY).await;

    // A forced copy into that image set lands its second version, which is the one that
    // carries the override.
    let versioned = copy_image_set(
        &emulator,
        &datastore_id,
        &source_id,
        information("1", None, Some((&destination_id, "1"))),
        &[("force", true)],
    )
    .await;
    assert_eq!(
        versioned.status, 200,
        "case \"version the destination\": expected HTTP 200 from CopyImageSet, got {versioned:?}"
    );
    tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY).await;

    let listed = list_versions(&emulator, &datastore_id, &destination_id, "").await;
    assert_eq!(
        listed.status, 200,
        "case \"list the copied versions\": expected HTTP 200, got {listed:?}"
    );
    assert_eq!(
        listed_versions(&listed),
        expected_versions(2),
        "case \"list the copied versions\": expected both copied versions oldest first, got {listed:?}"
    );

    let entries = properties(&listed);
    assert!(
        entries[0]["overrides"].is_null(),
        "case \"version 1\": expected no overrides on a version copied without force, got {:?}",
        entries[0]
    );
    assert_eq!(
        entries[1]["overrides"]["forced"].as_bool(),
        Some(true),
        "case \"version 2\": expected overrides.forced on the forced copy, got {:?}",
        entries[1]
    );
    for (case, entry) in [("version 1", &entries[0]), ("version 2", &entries[1])] {
        assert_eq!(
            entry["imageSetState"].as_str(),
            Some("ACTIVE"),
            "case {case:?}: expected ACTIVE once the copy the clock owed has landed, got {entry:?}"
        );
    }
    assert_eq!(
        entries[1]["ImageSetWorkflowStatus"].as_str(),
        Some("COPIED"),
        "case \"version 2\": expected COPIED on the version the copy landed, got {:?}",
        entries[1]
    );
}

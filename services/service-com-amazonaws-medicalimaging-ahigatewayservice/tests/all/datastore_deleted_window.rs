// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the LAST phase of a data store's life: the `DELETED` status.
//!
//! RULE: `DELETED` is one of the statuses `DatastoreProperties` / `DatastoreSummary`
//! report and one of the values the `ListDatastores` `datastoreStatus` filter accepts
//! (`smithy_query(query="shape",shape="DatastoreStatus")`), so a deletion the clock has
//! FINISHED is observable in it: the data store reports `DELETED` to `GetDatastore` and
//! appears in the listing — unfiltered, and under the `datastoreStatus=DELETED` filter —
//! for as long as its record survives, and only once the clock reaps that record is the
//! data store gone (404 `ResourceNotFoundException`, out of every listing). A record in
//! that status is REPORTABLE and not reachable: every operation that acts on a data
//! store, or on anything it held, answers as it does for an id nothing ever minted.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: how
//! many other data stores share the emulator, whether the store carried a name of its
//! own, whether it ever held content, which way the status is observed, and which
//! operation arrives while the record is a tombstone. Every case drives real HTTP
//! through the production front door, so what is asserted is the wire behaviour a
//! `medical-imaging` SDK client sees.
//!
//! ## Why the phases are POLLED, and why one suite holds the record still
//!
//! The deletion and the reaping that ends the `DELETED` window are both changes the
//! CLOCK owes, so a case that sleeps a fixed span races the scheduler. The first suite
//! instead polls the data store from the instant of the delete and asserts the SEQUENCE
//! of phases it observes, which is a property of the lifecycle rather than of any one
//! timing. The second suite is about what a tombstone ANSWERS rather than about how
//! long it lives, so it brings the record to that phase through the crate's public
//! `state` module (the same seeding seam the image-set suites use) and pushes the
//! reaping far out, so no case there is racing the clock either.

use std::collections::BTreeSet;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    service_state, DATASTORE_DELETED_RETENTION, DATASTORE_DELETION_DELAY,
};

use crate::datastore_lifecycle::{unheld_id, Answer, Emulator, ACCOUNT, DEFAULT_TEST_REGION};
use crate::image_set_copy::seed_image_set;
use crate::image_set_deletion::delete_image_set;
use crate::resource_tagging::{list_tags, tag, untag};

/// The service's constant text for a data store it does not hold.
const DATASTORE_NOT_FOUND: &str = "Datastore resource is not found.";

/// How far out the reaping of a tombstone is pushed so the record stays in `DELETED` for
/// the whole of a case. Long enough that no scheduling hiccup can let the clock reap it
/// mid-case.
const HELD: Duration = Duration::from_secs(3600);

/// How often the lifecycle is sampled while the clock works through it. Short against
/// both spans ([`DATASTORE_DELETION_DELAY`], [`DATASTORE_DELETED_RETENTION`]), so every
/// phase is sampled many times over.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// The phase a reading of a deleted data store shows — ordered, which is the whole
/// point: the lifecycle only ever moves forwards through them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    /// `DELETING`: the deletion the clock owes has not landed yet.
    Deleting,
    /// `DELETED`: the deletion has landed and the record has not been reaped.
    Deleted,
    /// 404: the record has been reaped and the data store is gone.
    Gone,
}

/// The phase one `GetDatastore` answer shows, or a panic naming the case and the answer
/// when it shows none of them — so no unexpected answer is read as a phase.
fn phase_of(case: &str, answer: &Answer) -> Phase {
    let status = answer.string(&["datastoreProperties", "datastoreStatus"]);
    match (answer.status, status.as_str()) {
        (200, "DELETING") => Phase::Deleting,
        (200, "DELETED") => Phase::Deleted,
        (404, _) => {
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException once the record is reaped, got {answer:?}"
            );
            assert_eq!(
                answer.string(&["message"]),
                DATASTORE_NOT_FOUND,
                "case {case:?}: expected the service's constant not-found text, got {answer:?}"
            );
            Phase::Gone
        }
        _ => panic!(
            "case {case:?}: expected DELETING, DELETED or a 404 after a delete, got {answer:?}"
        ),
    }
}

/// The ids one listing answer served.
fn listed_ids(answer: &Answer) -> BTreeSet<String> {
    answer
        .field(&["datastoreSummaries"])
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| summary["datastoreId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The ids the listing serves under `datastoreStatus=status`.
async fn listed_in_status(emulator: &Emulator, status: &str) -> BTreeSet<String> {
    let page = emulator
        .list_datastores(&format!("datastoreStatus={status}&maxResults=50"))
        .await;
    assert_eq!(
        page.status, 200,
        "listing filtered to {status:?}: expected HTTP 200, got {page:?}"
    );
    listed_ids(&page)
}

/// The ids the unfiltered listing serves.
async fn listed_unfiltered(emulator: &Emulator) -> BTreeSet<String> {
    let page = emulator.list_datastores("maxResults=50").await;
    assert_eq!(
        page.status, 200,
        "unfiltered listing: expected HTTP 200, got {page:?}"
    );
    listed_ids(&page)
}

/// The data store ARN the tagging routes name a data store by.
fn datastore_arn(datastore_id: &str) -> String {
    format!("arn:aws:medical-imaging:{DEFAULT_TEST_REGION}:{ACCOUNT}:datastore/{datastore_id}")
}

/// One row of the lifecycle table.
#[derive(Debug)]
struct WindowCase {
    case: &'static str,
    /// How many other data stores share the emulator.
    neighbours: usize,
    /// The name the data store was created with, `None` for a store created without one.
    datastore_name: Option<&'static str>,
    /// Whether the data store ever HELD an image set (retired before the store's own
    /// delete, which the delete requires).
    held_content: bool,
}

/// RULE (above): once the deletion the clock owes has landed, the data store is reported
/// in `DELETED` — by `GetDatastore`, by the unfiltered listing and by the
/// `datastoreStatus=DELETED` filter — until the clock reaps the record, and is gone from
/// all three afterwards; the stores around it are untouched throughout.
#[tokio::test]
async fn a_finished_deletion_is_observable_in_deleted_before_it_is_gone() {
    let cases = vec![
        WindowCase {
            case: "a lone data store",
            neighbours: 0,
            datastore_name: Some("window.lone"),
            held_content: false,
        },
        WindowCase {
            case: "a nameless data store among neighbours",
            neighbours: 2,
            datastore_name: None,
            held_content: false,
        },
        WindowCase {
            case: "a data store that held an image set",
            neighbours: 1,
            datastore_name: Some("window.held#1"),
            held_content: true,
        },
    ];

    for case in &cases {
        let name = case.case;
        let emulator = Emulator::new();
        let victim = match case.datastore_name {
            Some(datastore_name) => emulator.active_datastore(datastore_name, "window-named").await,
            None => {
                let created = emulator
                    .create_datastore(json!({ "clientToken": "window-nameless" }))
                    .await;
                assert_eq!(
                    created.status, 200,
                    "case {name:?}: expected HTTP 200 from CreateDatastore, got {created:?}"
                );
                let datastore_id = created.string(&["datastoreId"]);
                // Settled before the case starts: a delete of a store whose creation
                // is still in flight is refused, and this case is about the other end
                // of the lifecycle.
                land_the_clock(&emulator, &datastore_id);
                let settled = emulator.get_datastore(&datastore_id).await;
                assert_eq!(
                    settled.string(&["datastoreProperties", "datastoreStatus"]),
                    "ACTIVE",
                    "case {name:?}: expected the nameless store to settle ACTIVE, got {settled:?}"
                );
                datastore_id
            }
        };

        let mut survivors = BTreeSet::new();
        for index in 0..case.neighbours {
            survivors.insert(
                emulator
                    .active_datastore(
                        &format!("window.survivor-{index}"),
                        &format!("window-survivor-{index}"),
                    )
                    .await,
            );
        }

        if case.held_content {
            // Content the data store HELD, retired before the store's own delete: a
            // data store still holding an image set cannot be deleted at all.
            let image_set_id = seed_image_set(&emulator, &victim, 1);
            let retired = delete_image_set(&emulator, &victim, &image_set_id).await;
            assert_eq!(
                retired.status, 200,
                "case {name:?}: expected HTTP 200 from DeleteImageSet, got {retired:?}"
            );
            land_the_clock(&emulator, &victim);
        }

        let started = Instant::now();
        let deleted = emulator.delete_datastore(&victim).await;
        assert_eq!(
            deleted.status, 200,
            "case {name:?}: expected HTTP 200 from DeleteDatastore, got {deleted:?}"
        );
        assert_eq!(
            deleted.string(&["datastoreStatus"]),
            "DELETING",
            "case {name:?}: expected DELETING from DeleteDatastore, got {deleted:?}"
        );

        // Walk the phases the clock takes the record through. The deadline is a
        // generous multiple of the two spans together, so a stuck record fails the
        // case rather than hanging it.
        let deadline = started
            + 20 * (DATASTORE_DELETION_DELAY + DATASTORE_DELETED_RETENTION)
            + Duration::from_secs(1);
        let mut phase = Phase::Deleting;
        let mut seen: Vec<Phase> = Vec::new();
        let mut deleted_updated_at: Option<Value> = None;
        let mut checked_the_listing = false;
        while phase != Phase::Gone {
            assert!(
                Instant::now() < deadline,
                "case {name:?}: the record never reached {:?}, phases seen {seen:?}",
                Phase::Gone
            );
            let read = emulator.get_datastore(&victim).await;
            let observed = phase_of(name, &read);
            assert!(
                observed >= phase,
                "case {name:?}: the lifecycle moved backwards from {phase:?} to {observed:?}, phases seen {seen:?}"
            );
            if seen.last() != Some(&observed) {
                seen.push(observed);
            }
            phase = observed;

            if observed == Phase::Deleted {
                // What the window is FOR: the finished deletion is reported in
                // `DELETED`, whole, and selected by the filter for that status.
                assert_eq!(
                    read.string(&["datastoreProperties", "datastoreId"]),
                    victim,
                    "case {name:?}: expected the deleted store's own id back, got {read:?}"
                );
                assert_eq!(
                    read.string(&["datastoreProperties", "datastoreArn"]),
                    datastore_arn(&victim),
                    "case {name:?}: expected the data store's ARN, got {read:?}"
                );
                assert!(
                    !read
                        .string(&["datastoreProperties", "datastoreName"])
                        .is_empty(),
                    "case {name:?}: expected a reported name, got {read:?}"
                );
                // The deletion is dated at the instant it fell DUE, so what a second
                // reader sees does not move with when it happened to read.
                let updated_at = read.field(&["datastoreProperties", "updatedAt"]).clone();
                match &deleted_updated_at {
                    None => deleted_updated_at = Some(updated_at),
                    Some(first) => assert_eq!(
                        &updated_at, first,
                        "case {name:?}: expected updatedAt to stay {first:?} across reads, got {updated_at:?}"
                    ),
                }
                if !checked_the_listing {
                    checked_the_listing = true;
                    let filtered = listed_in_status(&emulator, "DELETED").await;
                    assert!(
                        filtered.contains(&victim),
                        "case {name:?}: expected datastoreStatus=DELETED to select {victim:?}, got {filtered:?}"
                    );
                    let unfiltered = listed_unfiltered(&emulator).await;
                    assert!(
                        unfiltered.contains(&victim),
                        "case {name:?}: expected the unfiltered listing to serve {victim:?}, got {unfiltered:?}"
                    );
                    let as_active = listed_in_status(&emulator, "ACTIVE").await;
                    assert_eq!(
                        as_active, survivors,
                        "case {name:?}: expected datastoreStatus=ACTIVE to select only the survivors, got {as_active:?}"
                    );
                }
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }

        assert!(
            checked_the_listing,
            "case {name:?}: the record was never observed in DELETED, phases seen {seen:?}"
        );
        assert_eq!(
            seen,
            vec![Phase::Deleting, Phase::Deleted, Phase::Gone],
            "case {name:?}: expected the record to be reported DELETING, then DELETED, then gone"
        );

        // Reaped: gone from the listing under every status, and from the unfiltered one.
        for status in ["CREATING", "CREATE_FAILED", "ACTIVE", "DELETING", "DELETED"] {
            let filtered = listed_in_status(&emulator, status).await;
            assert!(
                !filtered.contains(&victim),
                "case {name:?}: expected datastoreStatus={status} not to serve the reaped {victim:?}, got {filtered:?}"
            );
        }
        let remaining = listed_unfiltered(&emulator).await;
        assert_eq!(
            remaining, survivors,
            "case {name:?}: expected only the survivors to remain, got {remaining:?}"
        );
        for survivor in &survivors {
            let read = emulator.get_datastore(survivor).await;
            assert_eq!(
                read.string(&["datastoreProperties", "datastoreStatus"]),
                "ACTIVE",
                "case {name:?}: expected the neighbour {survivor:?} to stay ACTIVE, got {read:?}"
            );
        }
    }
}

/// One row of the tombstone table: an operation that ACTS on a data store, or on
/// something it held.
#[derive(Debug)]
struct ReachCase {
    case: &'static str,
    method: &'static str,
    /// The route, with `{datastoreId}` standing for the tombstone's id and `{unheld}`
    /// for a well-formed id of a resource inside it. The routes are the ones the
    /// sibling suites drive for each operation (`OPERATIONS.md`).
    uri: &'static str,
    body: Option<Value>,
}

/// RULE (above), the other half: a record in `DELETED` is reportable and NOT reachable —
/// every operation that acts on the data store, or on anything it held, is answered
/// exactly as it is for an id nothing ever minted, while `GetDatastore` keeps reporting
/// the status.
///
/// The rows are every route in this service that names a data store and does more than
/// report its status (`OPERATIONS.md`), including the tagging routes, which name it by
/// ARN rather than by id.
#[tokio::test]
async fn a_deleted_record_is_reported_but_reached_by_nothing() {
    let emulator = Emulator::new();
    let victim = emulator
        .active_datastore("tombstone.store", "tombstone-token")
        .await;
    let image_set_id = seed_image_set(&emulator, &victim, 1);
    let retired = delete_image_set(&emulator, &victim, &image_set_id).await;
    assert_eq!(
        retired.status, 200,
        "case \"retire the image set\": expected HTTP 200, got {retired:?}"
    );
    land_the_clock(&emulator, &victim);

    let neighbour = emulator
        .active_datastore("tombstone.neighbour", "tombstone-neighbour")
        .await;

    hold_in_deleted(&emulator, &victim).await;

    let cases = vec![
        ReachCase {
            case: "DeleteDatastore again",
            method: "DELETE",
            uri: "/datastore/{datastoreId}",
            body: None,
        },
        ReachCase {
            case: "StartDICOMImportJob",
            method: "POST",
            uri: "/startDICOMImportJob/datastore/{datastoreId}",
            body: Some(json!({
                "clientToken": "tombstone-import",
                "dataAccessRoleArn": "arn:aws:iam::000000000000:role/ImportRole",
                "inputS3Uri": "s3://imaging-in/",
                "outputS3Uri": "s3://imaging-out/",
            })),
        },
        ReachCase {
            case: "ListDICOMImportJobs",
            method: "GET",
            uri: "/listDICOMImportJobs/datastore/{datastoreId}",
            body: None,
        },
        ReachCase {
            case: "GetDICOMImportJob",
            method: "GET",
            uri: "/getDICOMImportJob/datastore/{datastoreId}/job/{unheld}",
            body: None,
        },
        ReachCase {
            case: "SearchImageSets",
            method: "POST",
            uri: "/datastore/{datastoreId}/searchImageSets",
            body: Some(json!({})),
        },
        ReachCase {
            case: "GetImageSet",
            method: "POST",
            uri: "/datastore/{datastoreId}/imageSet/{unheld}/getImageSet",
            body: None,
        },
        ReachCase {
            case: "GetImageSetMetadata",
            method: "POST",
            uri: "/datastore/{datastoreId}/imageSet/{unheld}/getImageSetMetadata",
            body: None,
        },
        ReachCase {
            case: "ListImageSetVersions",
            method: "POST",
            uri: "/datastore/{datastoreId}/imageSet/{unheld}/listImageSetVersions",
            body: None,
        },
        ReachCase {
            case: "GetImageFrame",
            method: "POST",
            uri: "/datastore/{datastoreId}/imageSet/{unheld}/getImageFrame",
            body: Some(json!({ "imageFrameId": "0".repeat(32) })),
        },
        ReachCase {
            case: "DeleteImageSet",
            method: "POST",
            uri: "/datastore/{datastoreId}/imageSet/{unheld}/deleteImageSet",
            body: None,
        },
        ReachCase {
            case: "CopyImageSet",
            method: "POST",
            uri: "/datastore/{datastoreId}/imageSet/{unheld}/copyImageSet",
            body: Some(json!({ "sourceImageSet": { "latestVersionId": "1" } })),
        },
    ];

    let arn = datastore_arn(&victim);
    // The tagging routes name the data store by ARN rather than by id, so they are sent
    // through the sibling suite's helpers, which spell the `@httpLabel` the one way.
    let tagging: Vec<(&str, Answer)> = vec![
        (
            "ListTagsForResource",
            list_tags(&emulator, &arn).await,
        ),
        (
            "TagResource",
            tag(&emulator, &arn, json!({ "team": "imaging" })).await,
        ),
        ("UntagResource", untag(&emulator, &arn, &["team"]).await),
    ];

    let mut answers: Vec<(&str, Answer)> = Vec::new();
    for case in &cases {
        let uri = case
            .uri
            .replace("{datastoreId}", &victim)
            .replace("{unheld}", &unheld_id());
        answers.push((case.case, emulator.call(case.method, &uri, case.body.clone()).await));
    }
    answers.extend(tagging);

    for (name, answer) in &answers {
        assert_eq!(
            answer.status, 404,
            "case {name:?}: expected HTTP 404 against a DELETED record, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.string(&["message"]),
            DATASTORE_NOT_FOUND,
            "case {name:?}: expected the service's constant not-found text, got {answer:?}"
        );
    }

    // Reporting it, all the while — and nothing the refused calls did moved it on.
    let read = emulator.get_datastore(&victim).await;
    assert_eq!(
        read.status, 200,
        "case \"report the tombstone\": expected HTTP 200, got {read:?}"
    );
    assert_eq!(
        read.string(&["datastoreProperties", "datastoreStatus"]),
        "DELETED",
        "case \"report the tombstone\": expected DELETED, got {read:?}"
    );
    let filtered = listed_in_status(&emulator, "DELETED").await;
    assert_eq!(
        filtered,
        BTreeSet::from([victim.clone()]),
        "case \"report the tombstone\": expected datastoreStatus=DELETED to select only {victim:?}, got {filtered:?}"
    );

    // And the data store beside it is reachable exactly as before: a tombstone is one
    // record's phase, not a mode the service is in.
    let beside = emulator.get_datastore(&neighbour).await;
    assert_eq!(
        beside.string(&["datastoreProperties", "datastoreStatus"]),
        "ACTIVE",
        "case \"the neighbour\": expected ACTIVE, got {beside:?}"
    );
    let jobs = emulator
        .call(
            "GET",
            &format!("/listDICOMImportJobs/datastore/{neighbour}"),
            None,
        )
        .await;
    assert_eq!(
        jobs.status, 200,
        "case \"the neighbour\": expected ListDICOMImportJobs to be served, got {jobs:?}"
    );

    // A tombstone frees the place it held: creating a data store while one is in
    // `DELETED` is admitted, so the record counts against no quota.
    let replacement = emulator
        .create_datastore(json!({
            "datastoreName": "tombstone.replacement",
            "clientToken": "tombstone-replacement",
        }))
        .await;
    assert_eq!(
        replacement.status, 200,
        "case \"create beside a tombstone\": expected HTTP 200, got {replacement:?}"
    );
}

/// Make every change the clock owes this data store and the image sets it holds due AT
/// ONCE, so the next request materialises them — a case that is not about a span does
/// not wait one out.
fn land_the_clock(emulator: &Emulator, datastore_id: &str) {
    let now = SystemTime::now();
    with_datastore(emulator, datastore_id, |datastore| {
        if datastore.status_due_at.is_some() {
            datastore.status_due_at = Some(now);
        }
        for image_set in datastore.image_sets.values_mut() {
            if let Some(pending) = image_set.pending.as_mut() {
                pending.due_at = now;
            }
        }
    });
}

/// Bring `datastore_id` to its `DELETED` record and HOLD it there: the deletion is made
/// due at once, one reporting call lands it, and the reaping that would end the window is
/// pushed out past the end of the case.
async fn hold_in_deleted(emulator: &Emulator, datastore_id: &str) {
    let deleted = emulator.delete_datastore(datastore_id).await;
    assert_eq!(
        deleted.status, 200,
        "holding {datastore_id} in DELETED: expected HTTP 200 from DeleteDatastore, got {deleted:?}"
    );
    land_the_clock(emulator, datastore_id);
    let landed = emulator.get_datastore(datastore_id).await;
    assert_eq!(
        landed.string(&["datastoreProperties", "datastoreStatus"]),
        "DELETED",
        "holding {datastore_id} in DELETED: expected the clock to land DELETED, got {landed:?}"
    );
    let held_until = SystemTime::now() + HELD;
    with_datastore(emulator, datastore_id, |datastore| {
        datastore.status_due_at = Some(held_until);
    });
}

/// Reach one persisted data store through the crate's public `state` module — the
/// seeding seam the image-set suites use — so a case about what an answer SAYS is not
/// also a case about how long a span is. Nothing is ASSERTED through this handle: every
/// assertion reads the wire.
fn with_datastore(
    emulator: &Emulator,
    datastore_id: &str,
    change: impl FnOnce(&mut service_com_amazonaws_medicalimaging_ahigatewayservice::state::Datastore),
) {
    let state = service_state(emulator.engine()).expect("the medical-imaging state store");
    let mut store = state.lock();
    let datastore = store
        .datastores
        .get_mut(datastore_id)
        .expect("the data store under test");
    change(datastore);
}

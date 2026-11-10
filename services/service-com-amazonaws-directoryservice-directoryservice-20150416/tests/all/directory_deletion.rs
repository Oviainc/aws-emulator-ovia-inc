// SPDX-License-Identifier: CC0-1.0

//! The teardown `DeleteDirectory` starts, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the harness is the sibling suite's, reached as
//! `crate::directory_lifecycle::<item>`). Nothing a RESPONSE claims is read from the state
//! struct, so an answer that is not computed from persisted state cannot pass.
//!
//! The rule under test, asserted over a TABLE of directories rather than one recorded call:
//! a delete does not take the directory away while the caller waits. It moves the directory
//! OUT of the stage it was serving in and into the teardown the modelled `DirectoryStage`
//! enum declares — `Deleting`, then `Deleted` — and `DescribeDirectories` keeps describing
//! it, from the same persisted record, until the removal the clock owes falls due. Only
//! then is the directory one the account no longer holds: a reporting call that takes the
//! id as a FILTER narrows to an empty page, and every other call naming it is refused.
//!
//! The rows differ along what the two creates persist — create operation, directory type,
//! size, edition and network type — and every pass keeps a control directory in the same
//! store, so a teardown is shown to be per-directory rather than per-store.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_directoryservice_directoryservice_20150416::state::{
    STAGE_DELETED, STAGE_DELETING, TRANSITION_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{
    call, create_directory, descriptions, minimal_create, ok, service, string, PASSWORD,
    PAST_ONE_SPAN, STAGE_ACTIVE,
};
use crate::harness::PAST_THE_TEARDOWN;
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// Comfortably past the FIRST step of the teardown — the move onto the stage it settles on
/// — and comfortably short of the second, so a case that waits this long observes the
/// settled stage while the record is still there.
///
/// DERIVED from the span the service publishes rather than restated: each step of the
/// teardown is one [`TRANSITION_SPAN`], so this sits between them with margin on each side.
const INSIDE_THE_SETTLED_STAGE: Duration =
    TRANSITION_SPAN.saturating_add(Duration::from_millis(200));

/// One row of the directory table: how the directory is created.
struct Row {
    case: &'static str,
    operation: &'static str,
    body: Value,
}

/// Create the row's directory through whichever create operation it names.
async fn create(router: &MultiServiceRouter, row: &Row) -> String {
    match row.operation {
        "CreateMicrosoftAD" => create_microsoft_ad(router, row.body.clone(), row.case).await,
        _ => create_directory(router, row.body.clone(), row.case).await,
    }
}

/// The table: rows differing along everything the two creates persist, so the rule is not
/// asserted on one shape of directory.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a minimal Simple AD directory",
            operation: "CreateDirectory",
            body: minimal_create("gone-simple.example.com"),
        },
        Row {
            case: "a large Simple AD directory with a dual-stack network",
            operation: "CreateDirectory",
            body: json!({
                "Name": "gone-large.example.com",
                "Password": PASSWORD,
                "Size": "Large",
                "NetworkType": "Dual-stack",
                "ShortName": "GONE"
            }),
        },
        Row {
            case: "a minimal Managed Microsoft AD directory",
            operation: "CreateMicrosoftAD",
            body: minimal_microsoft_ad("gone-corp.example.com"),
        },
        Row {
            case: "a Standard-edition Managed Microsoft AD directory",
            operation: "CreateMicrosoftAD",
            body: json!({
                "Name": "gone-standard.example.com",
                "Password": PASSWORD,
                "Edition": "Standard",
                "VpcSettings": {
                    "VpcId": "vpc-0a1b2c3d",
                    "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"]
                }
            }),
        },
    ]
}

/// The descriptions `DescribeDirectories` reports for the one id `directory_id`, which is
/// an empty page for an id the account no longer holds.
async fn page_for(router: &MultiServiceRouter, directory_id: &str, case: &str) -> Vec<Value> {
    let answer = ok(
        router,
        "DescribeDirectories",
        json!({ "DirectoryIds": [directory_id] }),
        case,
    )
    .await;
    descriptions(&answer, case)
}

/// The `Stage` `DescribeDirectories` reports for `directory_id`, or `None` once the account
/// no longer holds the directory at all.
async fn reported_stage(
    router: &MultiServiceRouter,
    directory_id: &str,
    case: &str,
) -> Option<String> {
    let page = page_for(router, directory_id, case).await;
    assert!(
        page.len() <= 1,
        "case {case:?}: expected at most one description for {directory_id}, got {page:?}"
    );
    page.first().map(|described| string(described, "Stage", case))
}

/// `DeleteDirectory`, asserting it answers 200 with the id it was asked about.
async fn delete(router: &MultiServiceRouter, directory_id: &str, case: &str) {
    let answer = ok(
        router,
        "DeleteDirectory",
        json!({ "DirectoryId": directory_id }),
        case,
    )
    .await;
    assert_eq!(
        answer["DirectoryId"],
        json!(directory_id),
        "case {case:?}: expected the delete to report the id it was asked about, got {answer}"
    );
}

#[tokio::test]
async fn a_deleted_directory_is_described_through_its_teardown_and_then_is_gone() {
    let router = service();
    let rows = rows();

    // The control directory is created first and never deleted, so every assertion below
    // is about the row's own directory rather than about the store emptying.
    let control = create_directory(
        &router,
        minimal_create("control.example.com"),
        "the control directory",
    )
    .await;
    let mut ids: Vec<(&Row, String)> = Vec::new();
    for row in &rows {
        ids.push((row, create(&router, row).await));
    }
    tokio::time::sleep(PAST_ONE_SPAN).await;

    for (row, id) in &ids {
        let case = row.case;
        // The directory is in service before the delete, so the stage the delete moves it
        // out of is a stage it really was in.
        assert_eq!(
            reported_stage(&router, id, case).await.as_deref(),
            Some(STAGE_ACTIVE),
            "case {case:?}: expected the directory to be in service before the delete"
        );
        // A tag, so the record the teardown keeps describing is one with content of its
        // own: nothing of the description is invented while the directory is being torn
        // down.
        ok(
            &router,
            "AddTagsToResource",
            json!({ "ResourceId": id, "Tags": [{ "Key": "owner", "Value": "platform" }] }),
            case,
        )
        .await;
        let before = page_for(&router, id, case).await;

        delete(&router, id, case).await;

        // The delete answered, and the directory is STILL described — out of `Active` and
        // into the first stage of the teardown, with everything else the record holds
        // reported exactly as it was.
        let during = page_for(&router, id, case).await;
        assert_eq!(
            during.len(),
            1,
            "case {case:?}: expected the deleted directory to still be described, got \
             {during:?}"
        );
        let described = &during[0];
        assert_eq!(
            described["Stage"],
            json!(STAGE_DELETING),
            "case {case:?}: expected the teardown's first stage, got {described}"
        );
        for member in ["DirectoryId", "Name", "Type", "LaunchTime"] {
            assert_eq!(
                described[member], before[0][member],
                "case {case:?}: expected {member} to be reported from the same record, got {}",
                described[member]
            );
        }
        // The tags are still the resource's own while it is being torn down: the record is
        // there, so what hangs off it is reported from there.
        let tags = ok(
            &router,
            "ListTagsForResource",
            json!({ "ResourceId": id }),
            case,
        )
        .await;
        assert_eq!(
            tags["Tags"],
            json!([{ "Key": "owner", "Value": "platform" }]),
            "case {case:?}: expected the directory's own tags during the teardown, got {tags}"
        );

        // One span on, the teardown has settled on the stage the modelled enum ends at,
        // and the record is still there to report it.
        tokio::time::sleep(INSIDE_THE_SETTLED_STAGE).await;
        assert_eq!(
            reported_stage(&router, id, case).await.as_deref(),
            Some(STAGE_DELETED),
            "case {case:?}: expected the teardown to settle on the enum's terminal stage"
        );

        // And once the removal the delete owes falls due the account no longer holds the
        // directory: the id FILTERS the reporting call to an empty page, and every call
        // whose subject is the directory is refused.
        tokio::time::sleep(PAST_THE_TEARDOWN).await;
        assert_eq!(
            reported_stage(&router, id, case).await,
            None,
            "case {case:?}: expected the record to have left the store"
        );
        for (operation, body) in [
            ("ListTagsForResource", json!({ "ResourceId": id })),
            ("CreateAlias", json!({ "DirectoryId": id, "Alias": "revived" })),
            (
                "AddTagsToResource",
                json!({ "ResourceId": id, "Tags": [{ "Key": "env", "Value": "dev" }] }),
            ),
        ] {
            let (status, parsed) = call(&router, operation, body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected {operation} on a removed directory to be refused, \
                 got {status} with {parsed}"
            );
        }
    }

    // The control directory went through none of it: a teardown is the named directory's.
    assert_eq!(
        reported_stage(&router, &control, "the control directory")
            .await
            .as_deref(),
        Some(STAGE_ACTIVE),
        "case \"the control directory\": expected the untouched directory to still be in \
         service"
    );
}

#[tokio::test]
async fn a_repeated_delete_answers_the_same_id_and_does_not_restart_the_teardown() {
    let router = service();
    let id = create_directory(&router, minimal_create("twice.example.com"), "repeated").await;
    let kept = create_directory(&router, minimal_create("kept.example.com"), "repeated").await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    delete(&router, &id, "repeated: the first delete").await;
    // The second delete asks for the state the directory is already heading for: it answers
    // the same id, and the teardown keeps the instant the FIRST delete started it from — so
    // the record still leaves the store within one teardown of that first call rather than
    // of this one.
    delete(&router, &id, "repeated: the second delete").await;
    assert_eq!(
        reported_stage(&router, &id, "repeated").await.as_deref(),
        Some(STAGE_DELETING),
        "case \"repeated\": expected the repeated delete to leave the teardown where it was"
    );

    tokio::time::sleep(PAST_THE_TEARDOWN).await;
    assert_eq!(
        reported_stage(&router, &id, "repeated").await,
        None,
        "case \"repeated\": expected the record to leave the store on the first delete's \
         own clock"
    );
    // A delete of an id the account no longer holds is refused, which is how a client tells
    // a delete it started from an id that was never there.
    let (status, parsed) = call(&router, "DeleteDirectory", json!({ "DirectoryId": &id })).await;
    assert_eq!(
        status, 400,
        "case \"repeated\": expected a delete of the removed id to be refused, got {status} \
         with {parsed}"
    );
    assert_eq!(
        reported_stage(&router, &kept, "repeated").await.as_deref(),
        Some(STAGE_ACTIVE),
        "case \"repeated\": expected the other directory to be untouched"
    );
}

#[tokio::test]
async fn what_describes_nothing_without_the_directory_goes_with_its_record() {
    let router = service();
    let case = "the cascade";
    let doomed = create_microsoft_ad(&router, minimal_microsoft_ad("doomed.example.com"), case).await;
    let kept = create_microsoft_ad(&router, minimal_microsoft_ad("kept.example.com"), case).await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // A snapshot OF each directory, so the one taken of the doomed directory is a record
    // that describes nothing once its directory's record is gone.
    let mut snapshots = Vec::new();
    for (directory_id, name) in [(&doomed, "doomed-nightly"), (&kept, "kept-nightly")] {
        let answer = ok(
            &router,
            "CreateSnapshot",
            json!({ "DirectoryId": directory_id, "Name": name }),
            case,
        )
        .await;
        snapshots.push(string(&answer, "SnapshotId", case));
    }

    delete(&router, &doomed, case).await;

    // While the directory is being torn down its record is still there, so the snapshot
    // taken of it is still described: the cascade follows the RECORD leaving the store, not
    // the delete being accepted.
    let during = ok(
        &router,
        "DescribeSnapshots",
        json!({ "SnapshotIds": [&snapshots[0]] }),
        case,
    )
    .await;
    assert_eq!(
        during["Snapshots"]
            .as_array()
            .map(|infos| infos.len())
            .unwrap_or_default(),
        1,
        "case {case:?}: expected the snapshot to be described during the teardown, got \
         {during}"
    );

    tokio::time::sleep(PAST_THE_TEARDOWN).await;
    // The record is gone, and with it the snapshot taken of that directory — while the
    // other directory's snapshot is untouched.
    let (status, parsed) = call(
        &router,
        "DescribeSnapshots",
        json!({ "SnapshotIds": [&snapshots[0]] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a snapshot of the removed directory to resolve to nothing, \
         got {status} with {parsed}"
    );
    let surviving = ok(
        &router,
        "DescribeSnapshots",
        json!({ "SnapshotIds": [&snapshots[1]] }),
        case,
    )
    .await;
    assert_eq!(
        surviving["Snapshots"]
            .as_array()
            .and_then(|infos| infos.first())
            .map(|info| info["SnapshotId"].clone()),
        Some(json!(&snapshots[1])),
        "case {case:?}: expected the other directory's snapshot to survive, got {surviving}"
    );
}

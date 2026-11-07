// SPDX-License-Identifier: CC0-1.0

//! The Directory Service snapshot slice, driven end to end over HTTP.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router. Nothing reaches into the state struct, so a
//! response that is not computed from persisted state cannot pass.
//!
//! The cases assert RULES over sets of inputs rather than one recorded call: the stored
//! snapshot record over a table of `CreateSnapshot` requests differing in name
//! (absent, plain, punctuated, at the published 128-character bound) and in which
//! directory they are taken of; the `DirectoryId`/`SnapshotIds` filters over a table of
//! filter forms including the documented empty list; pagination over a table of page
//! sizes spanning 1, several, exactly the record count and past it; the clock-owed
//! `Creating` -> `Completed` transition; the per-directory manual snapshot quota at its
//! bound and one past it; and the not-found and invalid-token refusals, each with the
//! modelled error code.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_directoryservice_directoryservice_20150416::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix: the service shape's name, `DirectoryService_20150416`.
const TARGET_PREFIX: &str = "DirectoryService_20150416";

/// A password satisfying the modelled `Password` pattern, so the create requests these
/// cases need are well formed. It is never asserted on.
const PASSWORD: &str = "Passw0rd!2345";

/// The first value of the modelled `SnapshotStatus` enum: where a snapshot sits until
/// the transition the create owes falls due.
const STATUS_CREATING: &str = "Creating";

/// The `Completed` value of the modelled `SnapshotStatus` enum.
const STATUS_COMPLETED: &str = "Completed";

/// The `Manual` value of the modelled `SnapshotType` enum: a snapshot a caller asked for.
const TYPE_MANUAL: &str = "Manual";

/// Directory Service's default quota "Manual snapshots: 5" per directory.
const MANUAL_SNAPSHOT_QUOTA: usize = 5;

/// The maximum length of the modelled `SnapshotName`, from its `@length` trait
/// (`min: 0, max: 128`).
const SNAPSHOT_NAME_MAX: usize = 128;

/// Comfortably longer than one clock-owed transition span, with margin for a loaded
/// test machine.
const PAST_ONE_SPAN: Duration = Duration::from_millis(900);

/// A well-formed `DirectoryId` (`^d-[0-9a-f]{10}$`) that is never created.
const ABSENT_DIRECTORY_ID: &str = "d-1dca5812e8";

/// A well-formed `SnapshotId` (`^s-[0-9a-f]{10}$`) that is never created.
const ABSENT_SNAPSHOT_ID: &str = "s-9f8e7d6c5b";

/// A router with just this service mounted on a fresh engine.
fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The cases exercise the service, not SigV4: requests carry a credential scope so
    // the router can name the target service, but no real signature.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One `awsJson1_1` call, returning the HTTP status and the parsed response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "ds.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDDSTEST/20260101/us-east-1/ds/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=00",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    (status, parsed)
}

/// A call that must succeed, returning its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
    );
    parsed
}

/// A call that must be refused with HTTP 400 and the given modelled error code.
async fn refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    expected_code: &str,
    case: &str,
) {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains(expected_code),
        "case {case:?}: expected {expected_code} from {operation}, got {parsed}"
    );
}

/// A required string member of a response body.
fn string(body: &Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member}, got {body}"))
        .to_string()
}

/// The modelled `SnapshotId` pattern, `^s-[0-9a-f]{10}$`, as an assertion.
fn assert_snapshot_id(id: &str, case: &str) {
    let suffix = id.strip_prefix("s-").unwrap_or_default();
    assert!(
        suffix.len() == 10
            && suffix
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "case {case:?}: expected a SnapshotId matching ^s-[0-9a-f]{{10}}$, got {id:?}"
    );
}

/// `CreateDirectory` for `name`, returning the minted directory id.
async fn create_directory(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let answer = ok(
        router,
        "CreateDirectory",
        json!({ "Name": name, "Password": PASSWORD, "Size": "Small" }),
        case,
    )
    .await;
    string(&answer, "DirectoryId", case)
}

/// `CreateSnapshot` of `directory_id`, with `name` when given, returning the minted
/// snapshot id.
async fn create_snapshot(
    router: &MultiServiceRouter,
    directory_id: &str,
    name: Option<&str>,
    case: &str,
) -> String {
    let mut body = json!({ "DirectoryId": directory_id });
    if let Some(name) = name {
        body["Name"] = json!(name);
    }
    let answer = ok(router, "CreateSnapshot", body, case).await;
    let id = string(&answer, "SnapshotId", case);
    assert_snapshot_id(&id, case);
    id
}

/// The `Snapshots` list of a `DescribeSnapshots` answer.
fn snapshots(body: &Value, case: &str) -> Vec<Value> {
    body["Snapshots"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected Snapshots, got {body}"))
        .clone()
}

/// `DescribeSnapshots` with `body`, returning its `Snapshots` list.
async fn describe(router: &MultiServiceRouter, body: Value, case: &str) -> Vec<Value> {
    let answer = ok(router, "DescribeSnapshots", body, case).await;
    snapshots(&answer, case)
}

/// The `SnapshotId`s of a `Snapshots` list, in the order reported.
fn reported_ids(found: &[Value], case: &str) -> Vec<String> {
    found
        .iter()
        .map(|snapshot| string(snapshot, "SnapshotId", case))
        .collect()
}

/// One row of the create table: what the request carries and what the stored record
/// must then report.
struct CreateCase {
    name: &'static str,
    requested_name: Option<String>,
}

#[tokio::test]
async fn a_created_snapshot_is_reported_from_persisted_state() {
    let router = service();
    let directory = create_directory(&router, "corp.example.com", "the snapshotted directory").await;

    // The rows differ along what `CreateSnapshot` persists: whether a `Name` was given,
    // and the forms the modelled `SnapshotName` admits — its pattern
    // `^([a-zA-Z0-9_])[\\a-zA-Z0-9_@#%*+=:?./!\s-]*$` allows punctuation and whitespace
    // after the first character, and its `@length` trait allows up to 128 of them.
    let cases = vec![
        CreateCase {
            name: "no name",
            requested_name: None,
        },
        CreateCase {
            name: "plain name",
            requested_name: Some("nightly".to_string()),
        },
        CreateCase {
            name: "punctuated name with whitespace",
            requested_name: Some("pre-upgrade snapshot #2 @ 03:00".to_string()),
        },
        CreateCase {
            name: "name at the published length bound",
            requested_name: Some("n".repeat(SNAPSHOT_NAME_MAX)),
        },
    ];

    let mut ids: Vec<String> = Vec::new();
    for case in &cases {
        let id = create_snapshot(
            &router,
            &directory,
            case.requested_name.as_deref(),
            case.name,
        )
        .await;
        ids.push(id);
    }

    // Distinct creates mint distinct ids.
    let distinct: BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "expected {} distinct snapshot ids, got {ids:?}",
        ids.len()
    );

    for (case, id) in cases.iter().zip(&ids) {
        let found = describe(&router, json!({ "SnapshotIds": [id] }), case.name).await;
        assert_eq!(
            found.len(),
            1,
            "case {:?}: expected exactly one snapshot for {id}, got {found:?}",
            case.name
        );
        let snapshot = &found[0];
        assert_eq!(
            snapshot["SnapshotId"],
            json!(id),
            "case {:?}: expected SnapshotId {id}, got {snapshot}",
            case.name
        );
        assert_eq!(
            snapshot["DirectoryId"],
            json!(directory),
            "case {:?}: expected DirectoryId {directory}, got {snapshot}",
            case.name
        );
        assert_eq!(
            snapshot["Type"],
            json!(TYPE_MANUAL),
            "case {:?}: expected a {TYPE_MANUAL} snapshot, got {snapshot}",
            case.name
        );
        match case.requested_name.as_deref() {
            Some(requested) => assert_eq!(
                snapshot["Name"],
                json!(requested),
                "case {:?}: expected Name {requested:?}, got {snapshot}",
                case.name
            ),
            None => assert!(
                snapshot["Name"].is_null(),
                "case {:?}: expected no Name on a create that gave none, got {snapshot}",
                case.name
            ),
        }
        // "The date and time that the snapshot was taken" is reported as the modelled
        // timestamp, so it is a number of epoch seconds and it is not the epoch itself.
        let start_time = snapshot["StartTime"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {:?}: expected a numeric StartTime, got {snapshot}",
                case.name
            )
        });
        assert!(
            start_time > 1_600_000_000.0,
            "case {:?}: expected StartTime to be a real epoch instant, got {start_time}",
            case.name
        );
    }

    // A read does not change what a later caller sees: the same listing twice.
    let first = reported_ids(&describe(&router, json!({}), "read twice").await, "read twice");
    let second = reported_ids(&describe(&router, json!({}), "read twice").await, "read twice");
    assert_eq!(
        first, second,
        "expected DescribeSnapshots to be a read: got {first:?} then {second:?}"
    );
    assert_eq!(
        first, ids,
        "expected every created snapshot in creation order, got {first:?}"
    );
}

#[tokio::test]
async fn a_snapshot_reaches_completed_once_the_clock_owes_it() {
    let router = service();
    let directory = create_directory(&router, "corp.example.com", "transition").await;
    let snapshot_id = create_snapshot(&router, &directory, Some("transition"), "transition").await;

    // The create answers while the snapshot is still being taken, so a caller acting
    // straight afterwards observes the transient status.
    let found = describe(&router, json!({ "SnapshotIds": [&snapshot_id] }), "transition").await;
    assert_eq!(
        found[0]["Status"],
        json!(STATUS_CREATING),
        "case {:?}: expected {STATUS_CREATING} straight after the create, got {:?}",
        "transition",
        found[0]
    );

    tokio::time::sleep(PAST_ONE_SPAN).await;

    let found = describe(&router, json!({ "SnapshotIds": [&snapshot_id] }), "transition").await;
    assert_eq!(
        found[0]["Status"],
        json!(STATUS_COMPLETED),
        "case {:?}: expected {STATUS_COMPLETED} once the owed transition fell due, got {:?}",
        "transition",
        found[0]
    );
}

/// One row of the filter table: a `DescribeSnapshots` request body built from the two
/// directories' ids, and which of the created snapshots it must report.
struct FilterCase {
    name: &'static str,
    /// Index of the directory to filter on, if any: 0 or 1.
    directory: Option<usize>,
    /// Indices into the created snapshot list, or `None` for no `SnapshotIds` member;
    /// `Some(vec![])` is the documented empty list.
    snapshots: Option<Vec<usize>>,
    /// Indices of the snapshots the answer must carry, in order.
    expect: Vec<usize>,
}

#[tokio::test]
async fn the_directory_and_snapshot_filters_select_from_persisted_state() {
    let router = service();
    let first = create_directory(&router, "corp.example.com", "filters").await;
    let second = create_directory(&router, "sales.example.org", "filters").await;

    // Three snapshots of the first directory and one of the second, so every filter row
    // below selects a different subset.
    let created = [
        create_snapshot(&router, &first, Some("one"), "filters").await,
        create_snapshot(&router, &first, Some("two"), "filters").await,
        create_snapshot(&router, &second, Some("three"), "filters").await,
        create_snapshot(&router, &first, Some("four"), "filters").await,
    ];
    let directories = [first.clone(), second.clone()];

    let cases = vec![
        FilterCase {
            name: "no filter at all",
            directory: None,
            snapshots: None,
            expect: vec![0, 1, 2, 3],
        },
        FilterCase {
            // `SnapshotIds`' documentation: "If this member is null or empty, all
            // snapshots are returned using the Limit and NextToken members."
            name: "the documented empty SnapshotIds list",
            directory: None,
            snapshots: Some(vec![]),
            expect: vec![0, 1, 2, 3],
        },
        FilterCase {
            name: "one directory with three snapshots",
            directory: Some(0),
            snapshots: None,
            expect: vec![0, 1, 3],
        },
        FilterCase {
            name: "one directory with one snapshot",
            directory: Some(1),
            snapshots: None,
            expect: vec![2],
        },
        FilterCase {
            name: "two snapshot ids, out of creation order",
            directory: None,
            snapshots: Some(vec![3, 0]),
            expect: vec![0, 3],
        },
        FilterCase {
            name: "one snapshot id",
            directory: None,
            snapshots: Some(vec![2]),
            expect: vec![2],
        },
        FilterCase {
            name: "both filters, agreeing",
            directory: Some(0),
            snapshots: Some(vec![0, 1]),
            expect: vec![0, 1],
        },
        FilterCase {
            name: "both filters, disagreeing",
            directory: Some(1),
            snapshots: Some(vec![0]),
            expect: vec![],
        },
    ];

    for case in &cases {
        let mut body = json!({});
        if let Some(index) = case.directory {
            body["DirectoryId"] = json!(directories[index]);
        }
        if let Some(indices) = case.snapshots.as_ref() {
            body["SnapshotIds"] = json!(indices
                .iter()
                .map(|index| created[*index].clone())
                .collect::<Vec<String>>());
        }
        let found = describe(&router, body, case.name).await;
        let expected: Vec<String> = case
            .expect
            .iter()
            .map(|index| created[*index].clone())
            .collect();
        assert_eq!(
            reported_ids(&found, case.name),
            expected,
            "case {:?}: expected {expected:?}, got {found:?}",
            case.name
        );
    }
}

#[tokio::test]
async fn pagination_walks_every_snapshot_exactly_once() {
    let router = service();
    let directory = create_directory(&router, "corp.example.com", "pagination").await;
    let mut created: Vec<String> = Vec::new();
    for index in 0..MANUAL_SNAPSHOT_QUOTA {
        created.push(create_snapshot(&router, &directory, Some(&format!("snap_{index}")), "pagination").await);
    }

    // Page sizes spanning 1, several, exactly the record count and past it, plus the
    // documented zero: `Limit`'s documentation says "If this value is zero, the maximum
    // number of items is specified by the limitations of the operation."
    for limit in [0usize, 1, 2, created.len(), created.len() + 3] {
        let case = format!("Limit {limit}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = json!({ "Limit": limit });
            if let Some(token) = next_token.as_ref() {
                body["NextToken"] = json!(token);
            }
            let answer = ok(&router, "DescribeSnapshots", body, &case).await;
            let page = snapshots(&answer, &case);
            if limit > 0 {
                assert!(
                    page.len() <= limit,
                    "case {case:?}: expected at most {limit} snapshots per page, got {}",
                    page.len()
                );
            }
            walked.extend(reported_ids(&page, &case));
            pages += 1;
            assert!(
                pages <= created.len() + 2,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
            match answer["NextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, created,
            "case {case:?}: expected every snapshot once in creation order, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn absent_ids_and_foreign_tokens_are_refused_with_the_modelled_error() {
    let router = service();
    // TWO directories with TWO snapshots each, so a `Limit: 1` page of either listing is
    // always truncated and always hands back a token to replay below.
    let directory = create_directory(&router, "corp.example.com", "refusals").await;
    let other = create_directory(&router, "sales.example.org", "refusals").await;
    let snapshot_id = create_snapshot(&router, &directory, Some("refusals_one"), "refusals").await;
    create_snapshot(&router, &other, Some("refusals_two"), "refusals").await;

    // A well-formed id naming nothing stored is reported rather than served as an empty
    // page, under the family the member it arrived in belongs to: a `DirectoryId` the
    // listing is narrowed by, or a snapshot id that is no longer a usable filter value.
    let not_found = vec![
        (
            "DescribeSnapshots with an absent DirectoryId",
            "DescribeSnapshots",
            json!({ "DirectoryId": ABSENT_DIRECTORY_ID }),
            "EntityDoesNotExistException",
        ),
        (
            "DescribeSnapshots with an absent SnapshotId",
            "DescribeSnapshots",
            json!({ "SnapshotIds": [ABSENT_SNAPSHOT_ID] }),
            "InvalidParameterException",
        ),
        (
            "DescribeSnapshots with one present and one absent SnapshotId",
            "DescribeSnapshots",
            json!({ "SnapshotIds": [&snapshot_id, ABSENT_SNAPSHOT_ID] }),
            "InvalidParameterException",
        ),
        (
            "CreateSnapshot of an absent directory",
            "CreateSnapshot",
            json!({ "DirectoryId": ABSENT_DIRECTORY_ID }),
            "EntityDoesNotExistException",
        ),
    ];
    for (case, operation, body, expected) in not_found {
        refused(&router, operation, body, expected, case).await;
    }

    // A token this service did not issue for THIS listing is not a page of it: a token
    // nobody issued, an empty one, one issued by another paginated operation of the same
    // service, and one this listing issued under a different filter.
    let foreign_token = {
        let answer = ok(
            &router,
            "DescribeDirectories",
            json!({ "Limit": 1 }),
            "a token from another listing",
        )
        .await;
        string(&answer, "NextToken", "a token from another listing")
    };
    let own_token_other_filter = {
        let answer = ok(
            &router,
            "DescribeSnapshots",
            json!({ "Limit": 1 }),
            "a token replayed under another filter",
        )
        .await;
        string(&answer, "NextToken", "a token replayed under another filter")
    };
    let tokens = vec![
        (
            "a token this service never issued",
            json!({ "NextToken": "not-a-token" }),
        ),
        ("an empty token", json!({ "NextToken": "" })),
        (
            "a token issued by another listing",
            json!({ "NextToken": foreign_token }),
        ),
        (
            "a token replayed under another filter",
            json!({ "DirectoryId": directory, "NextToken": own_token_other_filter }),
        ),
    ];
    for (case, body) in tokens {
        refused(
            &router,
            "DescribeSnapshots",
            body,
            "InvalidNextTokenException",
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn deleting_a_directory_deletes_the_snapshots_taken_of_it() {
    let router = service();
    let first = create_directory(&router, "corp.example.com", "delete").await;
    let second = create_directory(&router, "sales.example.org", "delete").await;
    let doomed = [
        create_snapshot(&router, &first, Some("doomed_one"), "delete").await,
        create_snapshot(&router, &first, Some("doomed_two"), "delete").await,
    ];
    let kept = [
        create_snapshot(&router, &second, Some("kept_one"), "delete").await,
        create_snapshot(&router, &second, Some("kept_two"), "delete").await,
    ];

    ok(
        &router,
        "DeleteDirectory",
        json!({ "DirectoryId": first }),
        "delete",
    )
    .await;

    // A snapshot is a snapshot OF a directory, so it does not outlive it.
    let remaining = reported_ids(&describe(&router, json!({}), "delete").await, "delete");
    assert_eq!(
        remaining,
        kept.to_vec(),
        "expected only the surviving directory's snapshots, got {remaining:?}"
    );
    for id in &doomed {
        refused(
            &router,
            "DescribeSnapshots",
            json!({ "SnapshotIds": [id] }),
            "InvalidParameterException",
            "a snapshot of a deleted directory",
        )
        .await;
    }
    refused(
        &router,
        "DescribeSnapshots",
        json!({ "DirectoryId": first }),
        "EntityDoesNotExistException",
        "the deleted directory",
    )
    .await;
}

#[tokio::test]
async fn an_oversized_snapshot_id_filter_is_refused() {
    let router = service();
    let directory = create_directory(&router, "corp.example.com", "oversized filter").await;
    let snapshot_id = create_snapshot(&router, &directory, Some("present"), "oversized filter").await;

    // This emulator's own guard on the caller-sized `SnapshotIds` list (100 entries),
    // checked BEFORE the list reaches the store: an account cannot hold more snapshots
    // than that, so a longer list can only repeat ids. Rows at the bound, and one past
    // it, each padded with well-formed `^s-[0-9a-f]{10}$` ids that name nothing.
    const MAX_SNAPSHOT_IDS_PER_REQUEST: usize = 100;
    let padding = |count: usize| -> Vec<String> {
        (0..count).map(|index| format!("s-{index:010x}")).collect()
    };

    // At the bound the list is resolved, so it is the absent ids that are reported.
    let mut at_bound = vec![snapshot_id.clone()];
    at_bound.extend(padding(MAX_SNAPSHOT_IDS_PER_REQUEST - 1));
    refused(
        &router,
        "DescribeSnapshots",
        json!({ "SnapshotIds": at_bound }),
        "InvalidParameterException",
        "SnapshotIds at the guard",
    )
    .await;

    // One past it the list is refused before anything is resolved.
    refused(
        &router,
        "DescribeSnapshots",
        json!({ "SnapshotIds": padding(MAX_SNAPSHOT_IDS_PER_REQUEST + 1) }),
        "InvalidParameterException",
        "SnapshotIds one past the guard",
    )
    .await;
}

#[tokio::test]
async fn the_per_directory_manual_snapshot_quota_is_enforced_at_its_bound() {
    let router = service();
    let first = create_directory(&router, "corp.example.com", "quota").await;
    let second = create_directory(&router, "sales.example.org", "quota").await;

    // Every snapshot up to the published bound is accepted.
    for index in 0..MANUAL_SNAPSHOT_QUOTA {
        let case = format!("snapshot {} of {MANUAL_SNAPSHOT_QUOTA}", index + 1);
        create_snapshot(&router, &first, Some(&format!("snap_{index}")), &case).await;
    }

    // One past it is refused with the modelled quota error.
    refused(
        &router,
        "CreateSnapshot",
        json!({ "DirectoryId": first, "Name": "one_too_many" }),
        "SnapshotLimitExceededException",
        "one past the quota",
    )
    .await;

    // The quota is per directory, so another directory is unaffected.
    create_snapshot(&router, &second, Some("unaffected"), "another directory").await;

    let held = describe(
        &router,
        json!({ "DirectoryId": first }),
        "the quota-full directory",
    )
    .await;
    assert_eq!(
        held.len(),
        MANUAL_SNAPSHOT_QUOTA,
        "expected the refused create to persist nothing, got {held:?}"
    );
}

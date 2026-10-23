// SPDX-License-Identifier: CC0-1.0

//! `ListAccessPoints` end to end over HTTP, asserted against what the service
//! PERSISTED rather than against what any one request carried.
//!
//! The suite reuses `file_system_lifecycle`'s harness, so every case goes through the
//! same `MultiServiceRouter` the emulator boots: routing, the restJson1 wire shapes,
//! `@httpQuery` binding, the modelled response status and the modelled error envelopes.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. The listing reports exactly the access points persisted under the ONE file system
//!    the request names — never a sibling's — in one stable order, and every member of
//!    every description is read from the stored record, so it agrees member for member
//!    with what `GetAccessPoint` answers for the same access point.
//! 2. The listing is `@paginated`: it is cut into pages of at most `maxResults`, and
//!    `nextToken` resumes exactly where the previous page stopped, so no access point is
//!    repeated or skipped whatever the page size.
//! 3. `fileSystemId` accepts either form the model allows — the bare id or the file
//!    system's ARN — and both name the same parent.
//! 4. A parent this service does not hold is the modelled `ResourceNotFoundException`
//!    (404); a `fileSystemId` of neither modelled form, a `nextToken` the service never
//!    issued, and a `maxResults` outside the modelled `@range(1, 1000)` are each the
//!    modelled `ValidationException` (400).
//! 5. The listing is a report: calling it changes nothing a later caller sees.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::file_system_lifecycle::{
    assert_lifecycle_state, call, create_access_point, created_file_system_id, encoded,
    file_system_arn_prefix, is_minted_id, new_router, without, Answer, ACCOUNT,
};

/// The members `ListAccessPointsDescription` carries (smithy_query: shape
/// `ListAccessPointsDescription`). The list shape holds NO `clientToken` and NO `tags`,
/// so those two are the only members of the stored record a listing does not report.
const DESCRIPTION_MEMBERS: &[&str] = &[
    "accessPointArn",
    "accessPointId",
    "fileSystemId",
    "status",
    "ownerId",
    "posixUser",
    "rootDirectory",
    "name",
];

/// The published bounds of `ListAccessPointsRequest$maxResults`, which is
/// `@range(min: 1, max: 1000)` with `@default(100)` (smithy_query: operation
/// `ListAccessPoints`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 1000;

/// `ListAccessPoints` on `label` (a bare id or an ARN), optionally with a page size and
/// a resume cursor.
async fn list_access_points(
    router: &MultiServiceRouter,
    label: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> Answer {
    // `fileSystemId` and `nextToken` ride as `@httpQuery` parameters, so a value
    // carrying `:` or `/` is percent-encoded by the harness's single encoder.
    let mut query = vec![format!("fileSystemId={}", encoded(label))];
    if let Some(size) = max_results {
        query.push(format!("maxResults={size}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={}", encoded(token)));
    }
    call(
        router,
        "GET",
        &format!("/access-points?{}", query.join("&")),
        None,
    )
    .await
}

/// Every page of the listing of `label`, walked to exhaustion at page size
/// `max_results`; returns the access point ids in the order they were reported, and the
/// descriptions keyed by id.
async fn walk_listing(
    router: &MultiServiceRouter,
    case: &str,
    label: &str,
    max_results: Option<i64>,
    expected: usize,
) -> (Vec<String>, Vec<Value>) {
    let mut ids: Vec<String> = Vec::new();
    let mut descriptions: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let listed = list_access_points(router, label, max_results, next_token.as_deref()).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200, got {} / {:?}",
            listed.status, listed.body
        );
        let page = listed
            .field("accessPoints")
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: accessPoints is @required on the response, got {:?}",
                    listed.body
                )
            })
            .clone();
        if let Some(size) = max_results {
            assert!(
                page.len() as i64 <= size,
                "case {case:?}: a page holds at most maxResults={size} items, got {}",
                page.len()
            );
        }
        for item in &page {
            ids.push(
                item["accessPointId"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: accessPointId is @required, got {item:?}")
                    })
                    .to_string(),
            );
            descriptions.push(item.clone());
        }
        pages += 1;
        assert!(
            pages <= expected + 1,
            "case {case:?}: the listing did not terminate after {pages} pages"
        );
        match listed.field("nextToken").as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }
    (ids, descriptions)
}

/// `GetAccessPoint`, the reader rule 1 compares the listing against.
async fn get_access_point(router: &MultiServiceRouter, access_point_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/access-points/{access_point_id}"),
        None,
    )
    .await
}

/// The table of `CreateAccessPoint` bodies the listing is exercised over. The rows
/// differ along every optional member a description reports — the POSIX identity, the
/// root directory, and the `Name` tag the name is derived from — plus the row that sends
/// none of them, so a listing of them cannot be satisfied by one canned description.
fn access_point_table() -> Vec<(&'static str, Value)> {
    vec![
        ("no optional member", json!({})),
        (
            "a posix user",
            json!({ "posixUser": { "uid": 1000, "gid": 1000 } }),
        ),
        (
            "a posix user with secondary gids and a name tag",
            json!({
                "posixUser": { "uid": 0, "gid": 0, "secondaryGids": [7, 8] },
                "tags": [{ "key": "Name", "value": "listed-root" }],
            }),
        ),
        (
            "a root directory with creation permissions",
            json!({
                "rootDirectory": {
                    "path": "/team/data",
                    "creationPermissions": { "ownerUid": 42, "ownerGid": 43, "permissions": "0755" },
                },
            }),
        ),
        (
            "every optional member, and a tag the name is not taken from",
            json!({
                "posixUser": { "uid": 65534, "gid": 65534 },
                "rootDirectory": { "path": "/exports/shared" },
                "tags": [{ "key": "Name", "value": "fully-specified" }, { "key": "env", "value": "test" }],
            }),
        ),
    ]
}

/// Create `count` access points under `file_system_id`, drawing their bodies from
/// [`access_point_table`], and return their ids in creation order.
async fn populate(
    router: &MultiServiceRouter,
    file_system_id: &str,
    label: &str,
    count: usize,
) -> Vec<String> {
    let table = access_point_table();
    let mut ids = Vec::new();
    for index in 0..count {
        let (case, body) = &table[index % table.len()];
        let mut request = body.clone();
        request["clientToken"] = json!(format!("{label}-{index}"));
        let created = create_access_point(router, file_system_id, request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?} ({label}-{index}): expected 200 from CreateAccessPoint, got {} / {:?}",
            created.status, created.body
        );
        ids.push(created.text("accessPointId"));
    }
    ids
}

/// Rule 1 and rule 3: the listing reports exactly its parent's access points, in
/// creation order, member for member as `GetAccessPoint` reads them back — whichever
/// form of `fileSystemId` named the parent.
#[tokio::test]
async fn a_listing_reports_its_own_parents_access_points_member_for_member() {
    let router = new_router();
    // Three parents with different child counts, so one parent's listing can be seen
    // NOT to leak into another's, including the parent that holds none.
    let parents = [("busy", 5usize), ("quiet", 2), ("empty", 0)];
    let mut created: Vec<(String, Vec<String>)> = Vec::new();
    for (label, count) in parents {
        let file_system_id = created_file_system_id(
            &router,
            &format!("arn:aws:s3:::s3files-listing-{label}"),
            Some(&format!("listing-{label}")),
        )
        .await;
        let children = populate(&router, &file_system_id, label, count).await;
        created.push((file_system_id, children));
    }

    for ((file_system_id, children), (label, count)) in created.iter().zip(parents) {
        let file_system_arn = format!("{}{file_system_id}", file_system_arn_prefix());
        // Rule 3: the bare id and the ARN name the same parent.
        for form in [file_system_id.as_str(), file_system_arn.as_str()] {
            let case = format!("{label} ({count} access points) listed by {form:?}");
            let (ids, descriptions) =
                walk_listing(&router, &case, form, None, children.len()).await;
            assert_eq!(
                ids, *children,
                "case {case:?}: expected exactly this parent's access points in creation order, \
                 got {ids:?}"
            );

            for description in &descriptions {
                let id = description["accessPointId"].as_str().unwrap();
                let read = get_access_point(&router, id).await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: expected 200 from GetAccessPoint({id}), got {} / {:?}",
                    read.status, read.body
                );
                // Every member the list shape carries must equal the stored record the
                // reader answers from; `status` is compared separately because the clock
                // may settle it between the two calls.
                let listed_members: Vec<&str> = description
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .filter(|member| !DESCRIPTION_MEMBERS.contains(member))
                    .collect();
                assert!(
                    listed_members.is_empty(),
                    "case {case:?}: a description reports only the modelled members, got extras \
                     {listed_members:?}"
                );
                assert_eq!(
                    without(description, &["status"]),
                    without(
                        &Value::Object(
                            read.body
                                .as_object()
                                .unwrap()
                                .iter()
                                .filter(|(member, _)| DESCRIPTION_MEMBERS
                                    .contains(&member.as_str()))
                                .map(|(member, value)| (member.clone(), value.clone()))
                                .collect()
                        ),
                        &["status"]
                    ),
                    "case {case:?}: the listed description of {id} must match what \
                     GetAccessPoint answers"
                );
                assert_lifecycle_state(&case, description["status"].as_str().unwrap_or_default());
                assert!(
                    is_minted_id("fsap-", id),
                    "case {case:?}: expected a minted fsap-<17..40 hex> id, got {id:?}"
                );
                assert_eq!(
                    description["accessPointArn"].as_str().unwrap_or_default(),
                    format!("{file_system_arn}/access-point/{id}"),
                    "case {case:?}: an access point ARN nests under its file system's"
                );
                assert_eq!(
                    description["fileSystemId"].as_str().unwrap_or_default(),
                    file_system_id,
                    "case {case:?}: every listed access point names this parent"
                );
                assert_eq!(
                    description["ownerId"].as_str().unwrap_or_default(),
                    ACCOUNT,
                    "case {case:?}: ownerId"
                );
            }
        }
    }

    // The parent that holds none reports the @required list as empty, with no cursor.
    let (file_system_id, _) = &created[2];
    let empty = list_access_points(&router, file_system_id, None, None).await;
    assert_eq!(
        empty.field("accessPoints"),
        &json!([]),
        "case \"a parent with no access points\": expected an empty accessPoints, got {:?}",
        empty.body
    );
    assert!(
        empty.field("nextToken").is_null(),
        "case \"a parent with no access points\": a last page carries no nextToken, got {:?}",
        empty.field("nextToken")
    );
}

/// Rule 2: whatever the page size, the pages together report each of the parent's
/// access points exactly once, in the same order the unpaged listing reports them.
#[tokio::test]
async fn listing_pages_at_most_max_results_and_resumes_where_it_stopped() {
    let router = new_router();
    let file_system_id = created_file_system_id(
        &router,
        "arn:aws:s3:::s3files-listing-pages",
        Some("listing-pages"),
    )
    .await;
    let children = populate(&router, &file_system_id, "page", 7).await;

    // Every page size inside the modelled range, each published bound, and the absent
    // case that takes the documented @default(100).
    for page_size in [
        Some(MAX_RESULTS_MIN),
        Some(2),
        Some(3),
        Some(7),
        Some(100),
        Some(MAX_RESULTS_MAX),
        None,
    ] {
        let case = format!("maxResults={page_size:?}");
        let (ids, _) = walk_listing(&router, &case, &file_system_id, page_size, children.len())
            .await;
        assert_eq!(
            ids, children,
            "case {case:?}: the pages together must report exactly this parent's access points, \
             in creation order"
        );
        let unique: BTreeSet<&String> = ids.iter().collect();
        assert_eq!(
            unique.len(),
            ids.len(),
            "case {case:?}: no access point may appear on two pages, got {ids:?}"
        );
    }
}

/// Rule 4: every input the service cannot honour is answered with its modelled error,
/// over a table spanning the unknown parent, both malformed-id forms, a forged cursor
/// and each side of the `maxResults` range.
#[tokio::test]
async fn unhonourable_inputs_answer_the_modelled_errors() {
    let router = new_router();
    let file_system_id = created_file_system_id(
        &router,
        "arn:aws:s3:::s3files-listing-errors",
        Some("listing-errors"),
    )
    .await;
    populate(&router, &file_system_id, "error", 2).await;

    // A well-formed id, and a well-formed ARN, that this service holds no record for:
    // the listing is scoped to one parent, so an absent parent is not an empty page.
    let unknown_id = "fs-0123456789abcdef0";
    let unknown_arn = format!("{}{unknown_id}", file_system_arn_prefix());
    for label in [unknown_id.to_string(), unknown_arn] {
        let answer = list_access_points(&router, &label, None, None).await;
        assert_eq!(
            answer.status, 404,
            "case \"an unknown parent {label}\": expected 404, got {} / {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "ResourceNotFoundException",
            "case \"an unknown parent {label}\": expected the modelled \
             ResourceNotFoundException, got {:?}",
            answer.error_type
        );
    }

    // A `fileSystemId` of neither modelled form, a cursor this service never issued, and
    // a page size outside the modelled range: each a 400 ValidationException.
    let refusals: Vec<(String, Answer)> = vec![
        (
            "a fileSystemId of neither modelled form".to_string(),
            list_access_points(&router, "not-a-file-system", None, None).await,
        ),
        (
            "a fileSystemId whose id is too short for the pattern".to_string(),
            list_access_points(&router, "fs-0123", None, None).await,
        ),
        (
            "a forged nextToken".to_string(),
            list_access_points(&router, &file_system_id, None, Some("not-a-token")).await,
        ),
        (
            format!("maxResults below the published minimum {MAX_RESULTS_MIN}"),
            list_access_points(&router, &file_system_id, Some(MAX_RESULTS_MIN - 1), None).await,
        ),
        (
            format!("maxResults above the published maximum {MAX_RESULTS_MAX}"),
            list_access_points(&router, &file_system_id, Some(MAX_RESULTS_MAX + 1), None).await,
        ),
    ];
    for (case, answer) in refusals {
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {:?}",
            answer.error_type
        );
    }
}

/// Rule 5: the listing reports, so what a later caller sees is what it would have seen
/// had nobody listed — the same access points, read the same way, however often the
/// listing ran.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let router = new_router();
    let file_system_id = created_file_system_id(
        &router,
        "arn:aws:s3:::s3files-listing-readonly",
        Some("listing-readonly"),
    )
    .await;
    let children = populate(&router, &file_system_id, "readonly", 4).await;

    // Past the provisioning span, so the clock owes no further transition and two reads
    // must agree member for member, `status` included.
    tokio::time::sleep(std::time::Duration::from_millis(900)).await;

    let before = list_access_points(&router, &file_system_id, Some(2), None).await;
    let reads_before: Vec<Value> = {
        let mut bodies = Vec::new();
        for id in &children {
            bodies.push(get_access_point(&router, id).await.body);
        }
        bodies
    };

    // Several more listings, at several page sizes, including a walk to exhaustion.
    for page_size in [Some(1), Some(3), None] {
        walk_listing(
            &router,
            &format!("repeat at maxResults={page_size:?}"),
            &file_system_id,
            page_size,
            children.len(),
        )
        .await;
    }

    let after = list_access_points(&router, &file_system_id, Some(2), None).await;
    assert_eq!(
        after.body, before.body,
        "case \"repeated listings\": the same request must answer the same page, got {:?} then {:?}",
        before.body, after.body
    );
    for (id, expected) in children.iter().zip(reads_before) {
        let read = get_access_point(&router, id).await;
        assert_eq!(
            read.body, expected,
            "case \"a read after repeated listings\": {id} must be unchanged by having been listed"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the run-group slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes. The HTTP harness is the one
//! `private_workflow_runs` owns; only the run-group helpers live here.
//!
//! The rules under test:
//!
//! * a run group is a resource of its own, stored once: whatever `CreateRunGroup` carried
//!   is what a later `GetRunGroup` reports, each member taken from that one record and
//!   nothing invented for a member the request left out;
//!   * each response carries exactly the members its own output shape models — the create
//!     answers the ARN, the id and the tags, and the read answers the whole record;
//!   * the group's id is the decimal the modelled `RunGroupId` describes and its ARN names
//!     it, both minted per group;
//!   * a replayed `requestId` answers with the group the first call created rather than
//!     minting a second one, and changes nothing about it;
//! * an id no group holds is the modelled 404, however the id is shaped;
//! * `ListRunGroups` reports every persisted group in creation order, each item's members
//!   read from the same record `GetRunGroup` reports and nothing its own shape does not
//!   model; `maxResults` bounds a page and only a page with more behind it carries the
//!   `nextToken` that resumes exactly there; `name` narrows the listing to the groups
//!   carrying exactly that name; and a token this listing never issued is refused;
//! * a run may join a group this emulator holds: the run records the group's canonical id,
//!   `ListRuns` narrows on it, and an id no group holds is the modelled 404 there too.

use serde_json::{json, Map, Value};
use service_com_amazonaws_omics_omics::state::{FIRST_RESOURCE_ID, MAX_TAGS};
use smithy_server_runtime::MultiServiceRouter;

use crate::private_workflow_runs::{
    assert_absent, assert_arn, assert_members, assert_timestamp, call, created_id, emulator,
    error_code, get_run, items, list_runs, start_run, start_run_body, workflow_for_runs,
};

/// Every member of `CreateRunGroupRequest` that caps a resource the group's runs may use.
/// Source: the `maxCpus`, `maxRuns`, `maxDuration` and `maxGpus` members of
/// `CreateRunGroupRequest`.
const CAP_MEMBERS: &[&str] = &["maxCpus", "maxRuns", "maxDuration", "maxGpus"];

/// The published range every one of [`CAP_MEMBERS`] shares (1..100000).
/// Source: the same four members of `CreateRunGroupRequest`.
const CAP_RANGE: (i64, i64) = (1, 100_000);

/// The published bounds of the modelled `RunGroupId` length (1..18).
/// Source: the `id` member of `CreateRunGroupResponse`.
const RUN_GROUP_ID_WIDTH: (usize, usize) = (1, 18);

/// The resource an omics run group ARN names.
const RUN_GROUP_ARN_RESOURCE: &str = "runGroup";

/// Members `CreateRunGroupResponse` does NOT model, so the create answers none of them
/// however much the request carried: they are read back through `GetRunGroup`.
/// Source: `CreateRunGroupResponse` -> arn | id | tags.
const NOT_ON_THE_CREATE_RESPONSE: &[&str] = &[
    "name",
    "maxCpus",
    "maxRuns",
    "maxDuration",
    "maxGpus",
    "creationTime",
];

/// `POST /runGroup`.
async fn create_group(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/runGroup", Some(body)).await
}

/// `GET /runGroup/{id}`.
async fn get_group(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/runGroup/{id}"), None).await
}

/// The `id` member of a 201 create response, after asserting the status and the id's shape.
fn created_group_id(case: &str, status: u16, body: &Value) -> String {
    let id = created_id(case, status, body);
    let (min, max) = RUN_GROUP_ID_WIDTH;
    assert!(
        id.len() >= min && id.len() <= max && id.bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: expected a {min}..{max} digit decimal id, got {id:?}"
    );
    let value: u64 = id
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: id {id:?} does not parse as a number"));
    assert!(
        value >= FIRST_RESOURCE_ID,
        "case {case:?}: expected an id at or above {FIRST_RESOURCE_ID}, got {value}"
    );
    id
}

/// A tag map with `count` entries, each key and value inside the modelled `TagKey`/
/// `TagValue` lengths.
fn tag_map(count: usize) -> Value {
    let mut tags = Map::new();
    for n in 0..count {
        tags.insert(format!("key{n}"), Value::String(format!("value{n}")));
    }
    Value::Object(tags)
}

/// A one-member object: the body of a row that names exactly one cap.
fn member_object(member: &str, value: Value) -> Value {
    let mut map = Map::new();
    map.insert(member.to_string(), value);
    Value::Object(map)
}

/// A row of the create/read-back table: a request body, the members `GetRunGroup` must
/// report for it, and the members it may not carry.
struct GroupCase {
    /// How the row is reported in a failure, and the group's `requestId`.
    case: String,
    /// The request body, without the `requestId` the row's `case` fills in.
    request: Value,
    /// Every member the read-back must carry with exactly this value.
    expected: Value,
    /// Members the read-back must NOT carry: nothing is invented for what the request
    /// left out.
    absent: Vec<&'static str>,
}

/// The table: one row per way a create can differ, so the rule is asserted over the class
/// and not over one request. The rows vary along which members are present, which cap is
/// named, the cap's value across the whole published range, the number of tags and the
/// characters of the name.
fn group_cases() -> Vec<GroupCase> {
    let (low, high) = CAP_RANGE;
    let mut cases = vec![
        GroupCase {
            case: "only the required requestId".to_string(),
            request: json!({}),
            expected: json!({}),
            absent: vec![
                "name",
                "maxCpus",
                "maxRuns",
                "maxDuration",
                "maxGpus",
                "tags",
            ],
        },
        GroupCase {
            case: "a named group with no caps".to_string(),
            request: json!({ "name": "autumn cohort" }),
            expected: json!({ "name": "autumn cohort" }),
            absent: vec!["maxCpus", "maxRuns", "maxDuration", "maxGpus", "tags"],
        },
        GroupCase {
            // The modelled `RunGroupName` pattern admits letters, marks, separators,
            // symbols, numbers and punctuation, so a name is not limited to ASCII words.
            case: "a name of letters, symbols, digits and punctuation".to_string(),
            request: json!({ "name": "grupo de ejecución ✦ 42 (prod)" }),
            expected: json!({ "name": "grupo de ejecución ✦ 42 (prod)" }),
            absent: vec!["maxCpus", "maxRuns", "maxDuration", "maxGpus", "tags"],
        },
        GroupCase {
            case: "every cap at once".to_string(),
            request: json!({
                "name": "fully capped",
                "maxCpus": 64,
                "maxRuns": 8,
                "maxDuration": 2_880,
                "maxGpus": 2,
            }),
            expected: json!({
                "name": "fully capped",
                "maxCpus": 64,
                "maxRuns": 8,
                "maxDuration": 2_880,
                "maxGpus": 2,
            }),
            absent: vec!["tags"],
        },
        GroupCase {
            case: "tags and no other member".to_string(),
            request: json!({ "tags": tag_map(2) }),
            expected: json!({ "tags": tag_map(2) }),
            absent: vec!["name", "maxCpus", "maxRuns", "maxDuration", "maxGpus"],
        },
        GroupCase {
            case: "one tag beside a cap".to_string(),
            request: json!({ "maxCpus": 16, "tags": tag_map(1) }),
            expected: json!({ "maxCpus": 16, "tags": tag_map(1) }),
            absent: vec!["name", "maxRuns", "maxDuration", "maxGpus"],
        },
        GroupCase {
            case: "tags at the documented bound".to_string(),
            request: json!({ "name": "many tags", "tags": tag_map(MAX_TAGS) }),
            expected: json!({ "name": "many tags", "tags": tag_map(MAX_TAGS) }),
            absent: vec!["maxCpus", "maxRuns", "maxDuration", "maxGpus"],
        },
    ];
    // One row per cap member at each published bound of its range, so the whole catalog of
    // caps is exercised and each is reported from the record rather than from whichever
    // member the first row happened to name.
    for member in CAP_MEMBERS {
        for value in [low, high] {
            let absent = CAP_MEMBERS
                .iter()
                .filter(|other| *other != member)
                .copied()
                .chain(["name", "tags"])
                .collect();
            cases.push(GroupCase {
                case: format!("{member} at {value}"),
                request: member_object(member, json!(value)),
                expected: member_object(member, json!(value)),
                absent,
            });
        }
    }
    cases
}

/// The request body for a row: its own members plus the `requestId` the row's `case`
/// names it by.
fn request_for(case: &GroupCase) -> Value {
    let mut body = case.request.as_object().cloned().unwrap_or_default();
    body.insert(
        "requestId".to_string(),
        Value::String(format!("rg-{}", case.case)),
    );
    Value::Object(body)
}

#[tokio::test]
async fn a_created_run_group_reads_back_from_state() {
    let router = emulator();
    let cases = group_cases();

    // Create them all first, so each read-back also proves the store keeps the rows apart
    // rather than answering with the most recent one.
    let mut created = Vec::new();
    let mut ids = std::collections::BTreeSet::new();
    for group in &cases {
        let request = request_for(group);
        let (status, body) = create_group(&router, &request).await;
        let id = created_group_id(&group.case, status, &body);
        let arn = body.get("arn").and_then(Value::as_str).unwrap_or_default();
        assert_arn(&group.case, arn, RUN_GROUP_ARN_RESOURCE, &id);
        // The create answers exactly the members its own output shape models: the tags the
        // request carried, and nothing that is only readable through `GetRunGroup`.
        assert_eq!(
            body.get("tags"),
            request.get("tags"),
            "case {:?}: the create must answer with the tags the request carried, got {body:?}",
            group.case
        );
        assert_absent(&group.case, NOT_ON_THE_CREATE_RESPONSE, &body);
        assert!(
            ids.insert(id.clone()),
            "case {:?}: id {id} was already handed out",
            group.case
        );
        created.push((id, arn.to_string()));
    }

    for (group, (id, arn)) in cases.iter().zip(&created) {
        let (status, body) = get_group(&router, id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetRunGroup, got {body:?}",
            group.case
        );
        assert_members(&group.case, "GetRunGroup", &group.expected, &body);
        assert_absent(&group.case, &group.absent, &body);
        assert_members(
            &group.case,
            "GetRunGroup",
            &json!({ "id": id, "arn": arn }),
            &body,
        );
        let creation_time = assert_timestamp(&group.case, "creationTime", &body);
        // A read reports, it does not restamp: the second read answers the same record.
        let (status, again) = get_group(&router, id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from the second GetRunGroup, got {again:?}",
            group.case
        );
        assert_eq!(
            again, body,
            "case {:?}: two reads of one group must answer identically, got {again:?} \
             after {body:?}",
            group.case
        );
        assert!(
            creation_time > 0.0,
            "case {:?}: expected a creationTime after the epoch, got {creation_time}",
            group.case
        );
    }
}

#[tokio::test]
async fn a_replayed_request_id_returns_the_same_run_group() {
    let router = emulator();
    // The rule generalises over the token's text and over how much of the rest of the
    // request changes on the replay: the token alone decides.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "identical replay",
            json!({ "requestId": "group-1", "name": "alpha", "maxCpus": 8 }),
            json!({ "requestId": "group-1", "name": "alpha", "maxCpus": 8 }),
        ),
        (
            "replay with different members",
            json!({ "requestId": "group-2", "name": "beta", "maxRuns": 2 }),
            json!({ "requestId": "group-2", "name": "beta-renamed", "maxRuns": 9, "maxGpus": 4 }),
        ),
        (
            "replay of a token that is all punctuation",
            json!({ "requestId": "::group-3::", "tags": tag_map(1) }),
            json!({ "requestId": "::group-3::", "tags": tag_map(3) }),
        ),
    ];

    let mut first = Vec::new();
    for (case, body, _) in &rows {
        let (status, response) = create_group(&router, body).await;
        let id = created_group_id(case, status, &response);
        let (status, stored) = get_group(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetRunGroup, got {stored:?}"
        );
        first.push((id, response, stored));
    }

    for ((case, _, replay), (expected_id, expected_response, expected_stored)) in
        rows.iter().zip(&first)
    {
        let (status, body) = create_group(&router, replay).await;
        let id = created_group_id(case, status, &body);
        assert_eq!(
            &id, expected_id,
            "case {case:?}: a replayed requestId must answer with the original group id, \
             got {body:?}"
        );
        assert_eq!(
            &body, expected_response,
            "case {case:?}: a replayed requestId must answer exactly as the first call did, \
             got {body:?}"
        );
        // And the replay changed nothing about the stored group, however different its
        // other members were.
        let (status, stored) = get_group(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetRunGroup after the replay, got {stored:?}"
        );
        assert_eq!(
            &stored, expected_stored,
            "case {case:?}: a replayed requestId must leave the group as it was, got \
             {stored:?}"
        );
    }

    // Two creates that differ only in their `requestId` are two groups.
    let mut minted = Vec::new();
    for case in ["token-a", "token-b"] {
        let (status, body) =
            create_group(&router, &json!({ "requestId": case, "name": "same name" })).await;
        minted.push(created_group_id(case, status, &body));
    }
    assert_ne!(
        minted[0], minted[1],
        "two creates carrying different requestIds must be two groups, got {} twice",
        minted[0]
    );
}

#[tokio::test]
async fn an_unknown_run_group_is_not_found() {
    let router = emulator();
    // One real group exists, so the 404s below are about the id, not about an empty
    // registry.
    let (status, body) = create_group(&router, &json!({ "requestId": "the only group" })).await;
    let existing = created_group_id("the only group", status, &body);

    // The rule generalises over the id's width and value, across the whole modelled
    // `RunGroupId` length (1..18): anything the registry does not hold is not found,
    // however it is shaped — including a value too wide to be a number this emulator mints.
    let (min, max) = RUN_GROUP_ID_WIDTH;
    let ids = [
        "0".repeat(min),
        "9".repeat(min),
        "1234567".to_string(),
        "1".repeat(max),
        "9".repeat(max),
    ];
    for id in &ids {
        assert_ne!(
            id, &existing,
            "the fixture must not collide with the real id"
        );
        let (status, body) = get_group(&router, id).await;
        assert_eq!(
            status, 404,
            "case {id:?}: expected 404 from GetRunGroup, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {id:?}: expected ResourceNotFoundException, got {body:?}"
        );
    }

    // The group that does exist is still readable, under its own spelling and under any
    // other decimal naming the same number.
    for id in [existing.clone(), format!("0{existing}")] {
        let (status, body) = get_group(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {id:?}: expected 200 from GetRunGroup, got {body:?}"
        );
        assert_members(
            "the only group",
            "GetRunGroup",
            &json!({ "id": existing }),
            &body,
        );
    }
}

#[tokio::test]
async fn a_cap_outside_its_published_range_is_refused() {
    let router = emulator();
    let (low, high) = CAP_RANGE;
    // Each cap carries the same published range, so each is accepted at both bounds and
    // refused one step outside either.
    for member in CAP_MEMBERS {
        for (value, expect_ok) in [
            (low - 1, false),
            (low, true),
            (high, true),
            (high + 1, false),
        ] {
            let case = format!("{member} at {value}");
            let mut request = member_object(member, json!(value));
            if let Value::Object(map) = &mut request {
                map.insert(
                    "requestId".to_string(),
                    Value::String(format!("range-{case}")),
                );
            }
            let (status, body) = create_group(&router, &request).await;
            if expect_ok {
                assert_eq!(status, 201, "case {case:?}: expected 201, got {body:?}");
                let id = created_group_id(&case, status, &body);
                let (status, stored) = get_group(&router, &id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from GetRunGroup, got {stored:?}"
                );
                assert_members(
                    &case,
                    "GetRunGroup",
                    &member_object(member, json!(value)),
                    &stored,
                );
            } else {
                assert_eq!(status, 400, "case {case:?}: expected 400, got {body:?}");
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {body:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn too_many_tags_on_a_run_group_are_refused() {
    let router = emulator();
    // The bound is MAX_TAGS, documented on the `tags` member, so the row at the bound is
    // accepted and the row one past it is refused.
    for (case, count, expect_ok) in [
        ("at the tag bound", MAX_TAGS, true),
        ("one past the tag bound", MAX_TAGS + 1, false),
    ] {
        let (status, body) = create_group(
            &router,
            &json!({ "requestId": format!("tags-{count}"), "tags": tag_map(count) }),
        )
        .await;
        if expect_ok {
            assert_eq!(status, 201, "case {case:?}: expected 201, got {body:?}");
        } else {
            assert_eq!(status, 400, "case {case:?}: expected 400, got {body:?}");
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_run_joins_a_run_group_this_emulator_holds() {
    let router = emulator();
    let workflow_id = workflow_for_runs(&router, "grouped").await;

    let mut groups = Vec::new();
    for case in ["cpu capped", "gpu capped"] {
        let (status, body) = create_group(
            &router,
            &json!({ "requestId": format!("join-{case}"), "name": case, "maxCpus": 32 }),
        )
        .await;
        groups.push(created_group_id(case, status, &body));
    }

    // The rule generalises over the group named and over how the `^[0-9]+$` id is spelled:
    // the run records the GROUP's canonical id either way, and a run that names no group
    // carries none.
    let rows: Vec<(&str, Option<String>, Option<String>)> = vec![
        (
            "the first group",
            Some(groups[0].clone()),
            Some(groups[0].clone()),
        ),
        (
            "the second group",
            Some(groups[1].clone()),
            Some(groups[1].clone()),
        ),
        (
            "the first group spelled with a leading zero",
            Some(format!("0{}", groups[0])),
            Some(groups[0].clone()),
        ),
        ("no group at all", None, None),
    ];

    let mut runs = Vec::new();
    for (case, requested, expected) in &rows {
        let extra = match requested {
            Some(id) => json!({ "runGroupId": id }),
            None => json!({}),
        };
        let body = start_run_body(&workflow_id, &format!("grouped-{case}"), &extra);
        let (status, response) = start_run(&router, &body).await;
        let run_id = created_id(case, status, &response);
        let (status, run) = get_run(&router, &run_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetRun, got {run:?}"
        );
        assert_eq!(
            run.get("runGroupId").and_then(Value::as_str),
            expected.as_deref(),
            "case {case:?}: expected runGroupId {expected:?}, got {run:?}"
        );
        runs.push((case, run_id, expected.clone()));
    }

    // `ListRuns` narrows on the group: every run that joined it, and no other.
    for group in &groups {
        let (status, body) = list_runs(&router, &format!("?runGroupId={group}")).await;
        let page = items(group, status, &body);
        let mut listed: Vec<String> = page
            .iter()
            .map(|item| {
                item.get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("<no id>")
                    .to_string()
            })
            .collect();
        listed.sort();
        let mut expected: Vec<String> = runs
            .iter()
            .filter(|(_, _, joined)| joined.as_deref() == Some(group.as_str()))
            .map(|(_, run_id, _)| run_id.clone())
            .collect();
        expected.sort();
        assert_eq!(
            listed, expected,
            "group {group}: expected exactly the runs that joined it, got {body:?}"
        );
    }

    // An id no group holds is the modelled 404 wherever a run names one, rather than an
    // empty page or a run bound to nothing.
    let unheld = "4242424";
    let body = start_run_body(&workflow_id, "grouped-unheld", &json!({ "runGroupId": unheld }));
    let (status, response) = start_run(&router, &body).await;
    assert_eq!(
        status, 404,
        "an unheld runGroupId must be 404 from StartRun, got {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "an unheld runGroupId must be ResourceNotFoundException from StartRun, got {response:?}"
    );
    let (status, response) = list_runs(&router, &format!("?runGroupId={unheld}")).await;
    assert_eq!(
        status, 404,
        "an unheld runGroupId must be 404 from ListRuns, got {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "an unheld runGroupId must be ResourceNotFoundException from ListRuns, got {response:?}"
    );
}

// ---------------------------------------------------------------------------
// ListRunGroups
// ---------------------------------------------------------------------------

/// `GET /runGroup{query}`.
async fn list_groups(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/runGroup{query}"), None).await
}

/// Every member `RunGroupListItem` models, so each is asserted against the one record
/// `GetRunGroup` reports rather than against whichever member a single row happened to
/// carry.
/// Source: `smithy_query(query="shape",shape="RunGroupListItem")` -> arn | id | name |
/// maxCpus | maxRuns | maxDuration | creationTime | maxGpus.
const LIST_ITEM_MEMBERS: &[&str] = &[
    "arn",
    "id",
    "name",
    "maxCpus",
    "maxRuns",
    "maxDuration",
    "creationTime",
    "maxGpus",
];

/// Members `RunGroupListItem` does NOT model, so a listed item carries none of them
/// however much the group holds.
/// Source: the same shape, which models no `tags` member.
const NOT_ON_A_LIST_ITEM: &[&str] = &["tags"];

/// The `id` of each item of a listing page, in the order the page carried them.
fn listed_ids(page: &[Value]) -> Vec<String> {
    page.iter()
        .map(|item| {
            item.get("id")
                .and_then(Value::as_str)
                .unwrap_or("<no id>")
                .to_string()
        })
        .collect()
}

/// Walk `ListRunGroups` from the first page to the last at `page_size`, answering every
/// id it reported in order. Polls the token the listing itself issued, so what is
/// asserted is that the pages concatenate to the whole collection.
async fn walk_listing(
    router: &MultiServiceRouter,
    case: &str,
    query: &str,
    page_size: usize,
    total: usize,
) -> Vec<String> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let paged = match &token {
            Some(token) => format!("?maxResults={page_size}&startingToken={token}{query}"),
            None => format!("?maxResults={page_size}{query}"),
        };
        let (status, body) = list_groups(router, &paged).await;
        let page = items(case, status, &body);
        assert!(
            page.len() <= page_size,
            "case {case:?}: a page of at most {page_size} carried {} items",
            page.len()
        );
        seen.extend(listed_ids(&page));
        pages += 1;
        assert!(
            pages <= total + 1,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
        match body.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    seen
}

#[tokio::test]
async fn list_run_groups_reports_every_persisted_group_and_pages_over_it() {
    let router = emulator();

    // An empty registry is an empty page, not a refusal and not a token.
    let (status, body) = list_groups(&router, "").await;
    let page = items("an empty registry", status, &body);
    assert!(
        page.is_empty(),
        "an empty registry must list no groups, got {body:?}"
    );
    assert!(
        body.get("nextToken").is_none(),
        "an empty page must carry no nextToken, got {body:?}"
    );

    // The same table the create/read-back case uses, so the listing is asserted over the
    // whole class of groups: named and unnamed, uncapped, capped on each member at each
    // published bound, and tagged.
    let cases = group_cases();
    let mut created = Vec::new();
    for group in &cases {
        let (status, body) = create_group(&router, &request_for(group)).await;
        created.push(created_group_id(&group.case, status, &body));
    }

    // The rule generalises over the page size: the pages concatenate to the whole
    // registry in creation order, whatever size is asked for, and only a page with more
    // behind it carries a nextToken.
    for page_size in [1usize, 2, 3, 7, cases.len(), 100] {
        let case = format!("page size {page_size}");
        let seen = walk_listing(&router, &case, "", page_size, cases.len()).await;
        assert_eq!(
            seen, created,
            "case {case:?}: expected every run group in creation order, got {seen:?}"
        );
    }

    // A page that exactly exhausts the registry has nothing behind it, so it carries no
    // token to resume from.
    let (status, body) = list_groups(&router, &format!("?maxResults={}", cases.len())).await;
    let page = items("an exact fit", status, &body);
    assert_eq!(
        page.len(),
        cases.len(),
        "an exact fit must carry every group, got {body:?}"
    );
    assert!(
        body.get("nextToken").is_none(),
        "a page with nothing behind it must carry no nextToken, got {body:?}"
    );

    // Every member of every item comes from the ONE record the reader reports, and the
    // item carries nothing its own shape does not model.
    let (status, body) = list_groups(&router, "").await;
    let page = items("the whole registry", status, &body);
    for (group, (id, item)) in cases.iter().zip(created.iter().zip(&page)) {
        let (status, stored) = get_group(&router, id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetRunGroup, got {stored:?}",
            group.case
        );
        for member in LIST_ITEM_MEMBERS {
            assert_eq!(
                item.get(*member),
                stored.get(*member),
                "case {:?}: the listed {member} must be the one GetRunGroup reports, got \
                 {:?} against {stored:?}",
                group.case,
                item.get(*member)
            );
        }
        assert_absent(&group.case, NOT_ON_A_LIST_ITEM, item);
    }

    // A token this operation never issued is a bad request, not a page — however it is
    // shaped.
    for token in ["not-a-token", "rg_", "run_6a6f", "wf_zz"] {
        let (status, body) = list_groups(&router, &format!("?startingToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {token:?}: an unissued startingToken must be refused, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {token:?}: expected ValidationException, got {body:?}"
        );
    }

    // And the token this listing DID issue resumes only this listing: handed to another
    // collection's, it is refused rather than read as a resume point into it.
    let (status, body) = list_groups(&router, "?maxResults=1").await;
    items("the first page", status, &body);
    let group_token = body
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a ListRunGroups nextToken, got {body:?}"))
        .to_string();
    let (status, body) = list_groups(&router, &format!("?startingToken={group_token}")).await;
    assert_eq!(
        status, 200,
        "a ListRunGroups token must resume ListRunGroups, got {body:?}"
    );
    let (status, body) = list_runs(&router, &format!("?startingToken={group_token}")).await;
    assert_eq!(
        status, 400,
        "a ListRunGroups token must not resume ListRuns, got {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "expected ValidationException from ListRuns, got {body:?}"
    );
}

#[tokio::test]
async fn list_run_groups_honours_its_name_filter() {
    let router = emulator();

    // Rows differing along what the filter keys on: a name two groups share, a name one
    // group carries, a name that is a prefix of another, a name differing only in case,
    // and a group carrying no name at all. Every name here is query-safe, so the request
    // asserts the filter and not a URI encoding; the whole modelled `RunGroupName`
    // character class is exercised by the create/read-back table above.
    let names: Vec<Option<&str>> = vec![
        Some("alpha"),
        Some("beta"),
        Some("alpha"),
        None,
        Some("alphabet"),
        Some("Alpha"),
        Some("gamma.2-x"),
    ];

    let mut created: Vec<(Option<&str>, String)> = Vec::new();
    for (n, name) in names.iter().enumerate() {
        let case = format!("group {n} named {name:?}");
        let mut body = Map::new();
        body.insert("requestId".to_string(), Value::String(format!("filter-{n}")));
        if let Some(name) = name {
            body.insert("name".to_string(), Value::String((*name).to_string()));
        }
        let (status, response) = create_group(&router, &Value::Object(body)).await;
        created.push((*name, created_group_id(&case, status, &response)));
    }

    // One row per name asked for, plus a name nothing carries: the filter answers exactly
    // the groups whose stored name equals it, in creation order, and matches no other
    // spelling of it.
    let asked = [
        "alpha",
        "beta",
        "alphabet",
        "Alpha",
        "gamma.2-x",
        "alph",
        "delta",
    ];
    for name in asked {
        let expected: Vec<String> = created
            .iter()
            .filter(|(stored, _)| *stored == Some(name))
            .map(|(_, id)| id.clone())
            .collect();
        let (status, body) = list_groups(&router, &format!("?name={name}")).await;
        let page = items(name, status, &body);
        assert_eq!(
            listed_ids(&page),
            expected,
            "case {name:?}: expected exactly the groups carrying that name, got {body:?}"
        );
        // The filter survives paging: one group per page concatenates to the same answer.
        let seen = walk_listing(
            &router,
            &format!("{name} one per page"),
            &format!("&name={name}"),
            1,
            created.len(),
        )
        .await;
        assert_eq!(
            seen, expected,
            "case {name:?}: a filtered listing must page over the same groups, got {seen:?}"
        );
    }

    // Unfiltered, every group is still there — the filter narrowed the answer without
    // changing what a later caller sees.
    let (status, body) = list_groups(&router, "").await;
    let page = items("unfiltered", status, &body);
    assert_eq!(
        listed_ids(&page),
        created
            .iter()
            .map(|(_, id)| id.clone())
            .collect::<Vec<String>>(),
        "an unfiltered listing must report every group in creation order, got {body:?}"
    );
}

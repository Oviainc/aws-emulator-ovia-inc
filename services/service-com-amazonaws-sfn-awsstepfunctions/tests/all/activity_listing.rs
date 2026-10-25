// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListActivities`, driven over this service's own wire
//! protocol (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`)
//! through the production `MultiServiceRouter`, so what is asserted is what a signed
//! AWS SDK request observes.
//!
//! The rules under test:
//!
//! 1. the listing REPORTS the persisted activity records: every activity
//!    `CreateActivity` stored in the region this endpoint serves appears exactly once,
//!    with the ARN, name and creation instant `DescribeActivity` reports for it, and
//!    nothing that was never stored or has since been deleted appears;
//! 2. paging walks ONE total order: whatever `maxResults` a caller chooses, a page
//!    carries at most that many items, `nextToken` is present exactly when items
//!    remain, and following it visits every record exactly once with no repeat and no
//!    omission;
//! 3. a pagination token this service did not issue is `InvalidToken` (400);
//! 4. the listing is a report: calling it changes nothing a later caller sees.
//!
//! The harness (`Emulator`, `string`) is the one `state_machine_lifecycle` builds;
//! `tests/all.rs` is this binary's crate root, so it is reached through
//! `crate::state_machine_lifecycle`.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_sfn_awsstepfunctions::state::DEFAULT_PAGE_SIZE;

use crate::state_machine_lifecycle::{
    create_machine, string, Emulator, ACCOUNT, HELLO_DEFINITION, REGION,
};

/// The largest `maxResults` the model admits (`maxResults` is `integer` with range
/// 0..1000, per this crate's `OPERATIONS.md` entry for `ListActivities`).
const MAX_PAGE_SIZE: i64 = 1000;

/// The ARN an activity of `name` carries in the region every request here signs for.
fn activity_arn(name: &str) -> String {
    format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:{name}")
}

/// One `ListActivities` page, required to succeed.
async fn list(emulator: &Emulator, request: &Value) -> Value {
    emulator.ok("ListActivities", request).await
}

/// The `activities` array of one answer.
fn items(body: &Value) -> &Vec<Value> {
    body.get("activities")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an 'activities' array, got {body:?}"))
}

/// Walk every page with `max_results` (absent when `None`) and return the items in
/// the order the pages reported them.
async fn walk(emulator: &Emulator, case: &str, max_results: Option<i64>) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // Bounded so a token that never clears fails loudly instead of looping forever.
    for _ in 0..64 {
        let mut request = json!({});
        if let Some(max) = max_results {
            request["maxResults"] = Value::from(max);
        }
        if let Some(token) = &token {
            request["nextToken"] = Value::String(token.clone());
        }
        let body = list(emulator, &request).await;
        let page = items(&body);
        if let Some(max) = max_results.filter(|max| *max > 0) {
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= max,
                "case {case:?}: a page carries at most maxResults={max} items, got {body:?}"
            );
        }
        collected.extend(page.iter().cloned());
        token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return collected;
        }
    }
    panic!("case {case:?}: paging did not terminate after 64 pages");
}

/// `DescribeActivity` for `arn`.
async fn describe(emulator: &Emulator, arn: &str) -> Value {
    emulator
        .ok("DescribeActivity", &json!({ "activityArn": arn }))
        .await
}

// ---------------------------------------------------------------------------
// The listing reports the stored records
// ---------------------------------------------------------------------------

/// Every listed item carries exactly what `DescribeActivity` reports for the same
/// activity, over a table whose rows differ along the NAME's form — and an empty
/// store lists nothing at all.
#[tokio::test]
async fn list_activities_reports_the_stored_records() {
    let emulator = Emulator::new();

    let empty = list(&emulator, &json!({})).await;
    assert!(
        items(&empty).is_empty(),
        "expected an empty store to list nothing, got {empty:?}"
    );
    assert_eq!(
        empty.get("nextToken").and_then(Value::as_str),
        None,
        "expected no nextToken when nothing is truncated, got {empty:?}"
    );

    // Rows differ along the form of the name the record is keyed by: the shortest and
    // longest the modelled `Name` (len 1..80) admits, ASCII and non-ASCII, and names
    // that are prefixes of one another.
    let longest = "z".repeat(80);
    let names = vec![
        "a",
        longest.as_str(),
        "order-worker_v2",
        "worker",
        "worker-2",
        "wörker-日本",
    ];
    let mut created: BTreeMap<String, Value> = BTreeMap::new();
    for name in &names {
        let answer = emulator
            .ok("CreateActivity", &json!({ "name": name }))
            .await;
        created.insert((*name).to_string(), answer);
    }

    let listed = list(&emulator, &json!({})).await;
    let reported: BTreeMap<String, Value> = items(&listed)
        .iter()
        .map(|item| (string(item, "name"), item.clone()))
        .collect();
    let reported_names: Vec<String> = reported.keys().cloned().collect();
    let mut expected_names: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    expected_names.sort();
    assert_eq!(
        reported_names, expected_names,
        "expected every created activity listed exactly once, got {reported_names:?}"
    );

    for name in &names {
        let case = *name;
        let item = reported
            .get(*name)
            .unwrap_or_else(|| panic!("case {case:?}: expected the activity listed, got {listed:?}"));
        let expected_arn = activity_arn(name);
        assert_eq!(
            string(item, "activityArn"),
            expected_arn,
            "case {case:?}: expected the ARN {expected_arn}, got {item:?}"
        );
        // The listed item is the stored record, so it agrees with the create's answer
        // and with the describe's — member by member, not merely in shape.
        let create_answer = &created[*name];
        assert_eq!(
            item.get("creationDate"),
            create_answer.get("creationDate"),
            "case {case:?}: expected the creationDate the create answered, got {item:?}"
        );
        let described = describe(&emulator, &expected_arn).await;
        for member in ["activityArn", "name", "creationDate"] {
            assert_eq!(
                item.get(member),
                described.get(member),
                "case {case:?}: expected the listed {member} to match the describe's \
                 ({described:?}), got {item:?}"
            );
        }
        // The listing reports the list item's members and nothing else: an activity's
        // encryption settings belong to `DescribeActivity`'s output shape.
        assert_eq!(
            item.get("encryptionConfiguration"),
            None,
            "case {case:?}: expected no encryptionConfiguration on a list item, got {item:?}"
        );
    }
}

/// A deleted activity leaves the listing, and a state machine of the same name never
/// enters it: the listing is of ACTIVITY records, keyed by their own ARNs.
#[tokio::test]
async fn list_activities_follows_the_store() {
    let emulator = Emulator::new();
    for name in ["kept", "doomed"] {
        emulator
            .ok("CreateActivity", &json!({ "name": name }))
            .await;
    }
    // A state machine of a third name, and of one of the activity names: neither is
    // an activity.
    create_machine(&emulator, "kept", HELLO_DEFINITION).await;
    create_machine(&emulator, "machine-only", HELLO_DEFINITION).await;

    let before = walk(&emulator, "before the delete", None).await;
    let names: Vec<String> = before.iter().map(|item| string(item, "name")).collect();
    assert_eq!(
        names,
        vec!["doomed".to_string(), "kept".to_string()],
        "expected only the two activities, got {names:?}"
    );

    emulator
        .ok(
            "DeleteActivity",
            &json!({ "activityArn": activity_arn("doomed") }),
        )
        .await;

    let after = walk(&emulator, "after the delete", None).await;
    let names: Vec<String> = after.iter().map(|item| string(item, "name")).collect();
    assert_eq!(
        names,
        vec!["kept".to_string()],
        "expected the deleted activity gone from the listing, got {names:?}"
    );
}

/// A regional endpoint lists only its own region's activities: the region is a field
/// of the ARN each record is keyed by, so two endpoints over ONE store report
/// disjoint listings.
#[tokio::test]
async fn list_activities_is_scoped_to_the_endpoint_region() {
    let emulator = Emulator::new();
    let elsewhere = Emulator::in_region_on(&emulator, "eu-west-1");

    for name in ["here-a", "here-b"] {
        emulator
            .ok("CreateActivity", &json!({ "name": name }))
            .await;
    }
    for name in ["there-a", "shared"] {
        elsewhere
            .ok("CreateActivity", &json!({ "name": name }))
            .await;
    }
    // The same NAME in both regions is two records, each listed by its own endpoint.
    emulator
        .ok("CreateActivity", &json!({ "name": "shared" }))
        .await;

    let cases: Vec<(&str, &Emulator, &str, Vec<&str>)> = vec![
        (
            "the endpoint the activities were created against",
            &emulator,
            REGION,
            vec!["here-a", "here-b", "shared"],
        ),
        (
            "another region's endpoint over the same store",
            &elsewhere,
            "eu-west-1",
            vec!["shared", "there-a"],
        ),
    ];
    for (case, client, region, expected) in cases {
        let listed = walk(client, case, None).await;
        let names: Vec<String> = listed.iter().map(|item| string(item, "name")).collect();
        let expected_names: Vec<String> =
            expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            names, expected_names,
            "case {case:?}: expected {expected_names:?}, got {names:?}"
        );
        for item in &listed {
            let arn = string(item, "activityArn");
            let prefix = format!("arn:aws:states:{region}:{ACCOUNT}:activity:");
            assert!(
                arn.starts_with(&prefix),
                "case {case:?}: expected every listed ARN under {prefix}, got {arn:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// Paging walks ONE total order: for every page size, following `nextToken` visits
/// each record exactly once in the same order the unpaged listing reports, and
/// `nextToken` is present exactly when items remain.
#[tokio::test]
async fn list_activities_pages_the_same_total_order() {
    let emulator = Emulator::new();
    let total = 7;
    for index in 0..total {
        emulator
            .ok("CreateActivity", &json!({ "name": format!("worker-{index}") }))
            .await;
    }

    let whole = walk(&emulator, "the whole listing", None).await;
    let order: Vec<String> = whole.iter().map(|item| string(item, "name")).collect();
    assert_eq!(
        order.len(),
        total,
        "expected every activity in the unpaged listing, got {order:?}"
    );

    // Rows: the page sizes the rule generalises over — one item per page, a size that
    // does not divide the total, exactly the total, more than the total, the model's
    // published maximum, and `0`, which `maxResults` documents as "uses the default".
    let cases: Vec<(&str, Option<i64>)> = vec![
        ("absent, so the default page size", None),
        ("one item per page", Some(1)),
        ("a page size that does not divide the total", Some(3)),
        ("a page size equal to the total", Some(total as i64)),
        ("a page size past the total", Some(total as i64 + 1)),
        ("the largest page size the model admits", Some(MAX_PAGE_SIZE)),
        ("zero, which means the default", Some(0)),
    ];
    for (case, max_results) in cases {
        let paged = walk(&emulator, case, max_results).await;
        let names: Vec<String> = paged.iter().map(|item| string(item, "name")).collect();
        assert_eq!(
            names, order,
            "case {case:?}: expected paging to walk the one total order {order:?}, got {names:?}"
        );
    }

    // `nextToken` is present exactly when items remain: with one item per page the
    // first page is truncated and the last is not.
    let first = list(&emulator, &json!({ "maxResults": 1 })).await;
    assert!(
        first.get("nextToken").and_then(Value::as_str).is_some(),
        "expected a nextToken while items remain, got {first:?}"
    );
    let last = list(&emulator, &json!({ "maxResults": total })).await;
    assert_eq!(
        last.get("nextToken").and_then(Value::as_str),
        None,
        "expected no nextToken once every item is reported, got {last:?}"
    );

    // The default page size is an upper limit of the operation's own, so a store
    // smaller than it is never truncated.
    assert!(
        total < DEFAULT_PAGE_SIZE,
        "this suite assumes a store smaller than the default page size ({DEFAULT_PAGE_SIZE})"
    );
    let defaulted = list(&emulator, &json!({})).await;
    assert_eq!(
        defaulted.get("nextToken").and_then(Value::as_str),
        None,
        "expected no nextToken when the default page holds everything, got {defaulted:?}"
    );
}

/// A page size past the modelled maximum is refused by the request walk, and a
/// pagination token this service did not issue is `InvalidToken` (400).
#[tokio::test]
async fn list_activities_refuses_a_page_it_cannot_serve() {
    let emulator = Emulator::new();
    for index in 0..3 {
        emulator
            .ok("CreateActivity", &json!({ "name": format!("worker-{index}") }))
            .await;
    }

    // One row per class of text that is not a token this service issued.
    let tokens = [
        ("text that is not a token at all", json!("not-a-token")),
        ("a token of the wrong shape", json!("page:2")),
        ("a negative offset", json!("-1")),
        ("a fractional offset", json!("1.5")),
        ("a token with trailing space", json!("1 ")),
        (
            "the longest token the model admits",
            json!("x".repeat(1024)),
        ),
    ];
    for (case, token) in tokens {
        let (status, code) = emulator
            .err("ListActivities", &json!({ "nextToken": token }))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidToken"),
            "case {case:?}: expected a 400 InvalidToken for {token:?}, got {status} {code}"
        );
    }

    // Each published bound of `maxResults`, and one past it.
    for (case, max_results, expect_ok) in [
        ("the minimum the model admits", 0, true),
        ("the maximum the model admits", MAX_PAGE_SIZE, true),
        ("one past the maximum", MAX_PAGE_SIZE + 1, false),
        ("one below the minimum", -1, false),
    ] {
        let request = json!({ "maxResults": max_results });
        if expect_ok {
            let body = list(&emulator, &request).await;
            assert_eq!(
                items(&body).len(),
                3,
                "case {case:?}: expected the whole store on one page, got {body:?}"
            );
        } else {
            let (status, code) = emulator.err("ListActivities", &request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected maxResults={max_results} refused, got {status} {code}"
            );
        }
    }
}

/// The listing is a REPORT: calling it, however it is paged, changes nothing a later
/// caller sees.
#[tokio::test]
async fn listing_activities_changes_nothing() {
    let emulator = Emulator::new();
    for index in 0..4 {
        emulator
            .ok("CreateActivity", &json!({ "name": format!("worker-{index}") }))
            .await;
    }

    let before = list(&emulator, &json!({})).await;
    for max_results in [None, Some(1), Some(2), Some(MAX_PAGE_SIZE)] {
        walk(&emulator, "a repeated walk", max_results).await;
    }
    let after = list(&emulator, &json!({})).await;
    assert_eq!(
        before, after,
        "expected the listing unchanged by reading it, got {before:?} then {after:?}"
    );

    // And every record is still readable through its own describe.
    for index in 0..4 {
        let arn = activity_arn(&format!("worker-{index}"));
        let described = describe(&emulator, &arn).await;
        assert_eq!(
            string(&described, "activityArn"),
            arn,
            "expected {arn} still stored, got {described:?}"
        );
    }
}

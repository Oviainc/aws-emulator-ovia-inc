// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListMilestones`.
//!
//! The rules pinned here, each over a table of inputs rather than one recorded request:
//!
//! 1. `ListMilestones` REPORTS the milestones recorded against the workload the path
//!    names, in `MilestoneNumber` order, each as a `MilestoneSummary` of the workload as
//!    it stood when the milestone was recorded; paging enumerates every one of them
//!    exactly once and the last page omits `NextToken`.
//! 2. The path `WorkloadId` is resolved, not validated: an id naming no stored workload
//!    is `404 ResourceNotFoundException`, never an empty page.
//! 3. A `NextToken` this service did not issue is `400 ValidationException`.
//! 4. Listing is reporting only: it changes nothing a later caller sees.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::workload_harness::{created_id, minimal_create, Answer, Client};

/// A well-formed `WorkloadId` (32 lowercase hex) that no workload carries.
const ABSENT_HEX_ID: &str = "00000000000000000000000000000000";

/// The largest page this service serves, and the page size a request that omits
/// `MaxResults` gets — the modelled `MaxResults` `@range(min: 1, max: 50)` maximum,
/// taken from the emulator's single source of truth rather than a literal.
const MAX_RESULTS_CEILING: i32 = service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::MAX_RESULTS_CEILING;

async fn create_milestone(client: &Client, workload_id: &str, name: &str, token: &str) -> Answer {
    client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/milestones"),
            Some(json!({ "MilestoneName": name, "ClientRequestToken": token })),
        )
        .await
}

async fn get_milestone(client: &Client, workload_id: &str, milestone_number: i64) -> Answer {
    client
        .send(
            "GET",
            &format!("/workloads/{workload_id}/milestones/{milestone_number}"),
            None,
        )
        .await
}

async fn list_milestones(client: &Client, workload_id: &str, body: Value) -> Answer {
    client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/milestonesSummaries"),
            Some(body),
        )
        .await
}

/// The `MilestoneSummaries` of a successful answer.
fn summaries(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListMilestones 200, got {answer:?}"
    );
    answer
        .body
        .get("MilestoneSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: MilestoneSummaries must be present, got {answer:?}"))
}

/// A workload with `count` milestones recorded against it, named `<prefix>-<n>`.
async fn workload_with_milestones(client: &Client, prefix: &str, count: i64) -> String {
    let workload_id = created_id(
        &client
            .create(minimal_create(prefix, &format!("tok-{prefix}")))
            .await,
        prefix,
    );
    for number in 1..=count {
        let answer = create_milestone(
            client,
            &workload_id,
            &format!("{prefix}-{number}"),
            &format!("tok-{prefix}-{number}"),
        )
        .await;
        assert_eq!(
            answer.body.get("MilestoneNumber").and_then(Value::as_i64),
            Some(number),
            "case {prefix:?}: milestone {number} must be recorded, got {answer:?}"
        );
    }
    workload_id
}

/// Rule 1: paging enumerates every recorded milestone exactly once, in `MilestoneNumber`
/// order, and the last page omits `NextToken`. Checked exhaustively over every
/// (milestone count, page size) pair in a bounded range, so the invariant is pinned at
/// both ends of the range and wherever the page boundary lands.
#[tokio::test]
async fn paging_enumerates_every_milestone_in_number_order_exactly_once() {
    for milestone_count in 1i64..=5 {
        let client = Client::new();
        let workload_id =
            workload_with_milestones(&client, &format!("pager-{milestone_count}"), milestone_count)
                .await;
        let expected: Vec<i64> = (1..=milestone_count).collect();

        for page_size in 1i64..=(milestone_count + 2) {
            let case = format!("{milestone_count} milestones, MaxResults {page_size}");
            let mut seen: Vec<i64> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut pages = 0i64;
            loop {
                pages += 1;
                assert!(
                    pages <= milestone_count + 2,
                    "case {case:?}: paging did not terminate after {pages} pages"
                );
                let mut request = json!({ "MaxResults": page_size });
                if let Some(token) = &next_token {
                    request["NextToken"] = json!(token);
                }
                let answer = list_milestones(&client, &workload_id, request).await;
                assert_eq!(
                    answer.body.get("WorkloadId").and_then(Value::as_str),
                    Some(workload_id.as_str()),
                    "case {case:?}: the answer must name the workload listed, got {answer:?}"
                );
                let page = summaries(&answer, &case);
                assert!(
                    i64::try_from(page.len()).expect("a page fits in an i64") <= page_size,
                    "case {case:?}: a page must not exceed MaxResults, got {} entries",
                    page.len()
                );
                for summary in &page {
                    // `MilestoneSummary` members — smithy_query(query="shape",
                    // shape="MilestoneSummary"): every one is filled from the record.
                    for name in [
                        "MilestoneNumber",
                        "MilestoneName",
                        "RecordedAt",
                        "WorkloadSummary",
                    ] {
                        assert!(
                            summary.get(name).is_some(),
                            "case {case:?}: MilestoneSummary must carry {name}, got {summary:?}"
                        );
                    }
                    seen.push(
                        summary
                            .get("MilestoneNumber")
                            .and_then(Value::as_i64)
                            .unwrap_or_default(),
                    );
                }
                next_token = answer
                    .body
                    .get("NextToken")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                match &next_token {
                    None => break,
                    Some(token) => assert!(
                        token
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || "+/=_-".contains(c)),
                        "case {case:?}: NextToken must match ^[A-Za-z0-9+/=_-]+$, got {token:?}"
                    ),
                }
            }

            let unique: BTreeSet<i64> = seen.iter().copied().collect();
            assert_eq!(
                unique.len(),
                seen.len(),
                "case {case:?}: paging returned a milestone twice: {seen:?}"
            );
            assert_eq!(
                seen, expected,
                "case {case:?}: paging must enumerate every milestone exactly once in \
                 MilestoneNumber order, got {seen:?}"
            );
        }
    }
}

/// Rule 1, continued: a request that omits `MaxResults` asks for a full page, and the
/// page size is the modelled maximum. Checked on each published bound of the modelled
/// `@range(min: 1, max: 50)` and one past it, where the boundary validation refuses the
/// request with a 400 before the handler runs.
#[tokio::test]
async fn max_results_is_honoured_inside_its_modelled_range_and_refused_outside_it() {
    let client = Client::new();
    let total = 4i64;
    let workload_id = workload_with_milestones(&client, "bounds", total).await;

    for (case, request, expected_len) in [
        ("omitted", json!({}), total),
        ("min", json!({ "MaxResults": 1 }), 1),
        ("under-total", json!({ "MaxResults": total - 1 }), total - 1),
        ("at-total", json!({ "MaxResults": total }), total),
        ("over-total", json!({ "MaxResults": total + 1 }), total),
        (
            "max",
            json!({ "MaxResults": MAX_RESULTS_CEILING }),
            total,
        ),
    ] {
        let answer = list_milestones(&client, &workload_id, request).await;
        let page = summaries(&answer, case);
        assert_eq!(
            i64::try_from(page.len()).expect("a page fits in an i64"),
            expected_len,
            "case {case:?}: expected {expected_len} summaries, got {answer:?}"
        );
        let has_token = answer.body.get("NextToken").is_some();
        assert_eq!(
            has_token,
            expected_len < total,
            "case {case:?}: NextToken must be present only while milestones remain, got {answer:?}"
        );
    }

    // One past each published bound of `MaxResults` @range(min: 1, max: 50).
    for (case, max_results) in [("below-min", 0), ("above-max", MAX_RESULTS_CEILING + 1)] {
        let answer =
            list_milestones(&client, &workload_id, json!({ "MaxResults": max_results })).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a MaxResults outside its modelled range must be a 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
    }
}

/// Rule 1, continued: a summary reports the milestone's own members and the workload AS
/// RECORDED — a later `UpdateWorkload` changes what `GetWorkload` reports and leaves
/// every listed summary alone. Rows differ along the member edited.
#[tokio::test]
async fn a_summary_reports_the_workload_as_it_stood_when_the_milestone_was_recorded() {
    let client = Client::new();
    let workload_id = created_id(
        &client.create(minimal_create("snapshot-host", "tok-snap")).await,
        "snapshot-host",
    );
    let recorded = create_milestone(&client, &workload_id, "before-the-edit", "tok-snap-1").await;
    assert_eq!(
        recorded.status, 200,
        "case \"record\": CreateMilestone must succeed, got {recorded:?}"
    );

    for (case, member, value) in [
        ("name", "WorkloadName", json!("snapshot-host-renamed")),
        ("description", "Description", json!("edited after the milestone")),
        ("environment", "Environment", json!("PREPRODUCTION")),
    ] {
        let updated = client
            .update(&workload_id, json!({ member: value.clone() }))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: UpdateWorkload must succeed, got {updated:?}"
        );

        let answer = list_milestones(&client, &workload_id, json!({})).await;
        let page = summaries(&answer, case);
        assert_eq!(
            page.len(),
            1,
            "case {case:?}: the workload holds exactly one milestone, got {answer:?}"
        );
        let summary = &page[0];
        assert_eq!(
            summary.get("MilestoneName").and_then(Value::as_str),
            Some("before-the-edit"),
            "case {case:?}: the summary must report the recorded name, got {summary:?}"
        );
        assert_eq!(
            summary.get("MilestoneNumber").and_then(Value::as_i64),
            Some(1),
            "case {case:?}: the summary must report the recorded number, got {summary:?}"
        );
        // `WorkloadSummary` is a projection of the workload AS RECORDED, so the edit
        // above must not reach it; the live workload does take the edit.
        let workload_summary = summary
            .get("WorkloadSummary")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            workload_summary.get("WorkloadName").and_then(Value::as_str),
            Some("snapshot-host"),
            "case {case:?}: the recorded workload must not take the later edit, got {summary:?}"
        );
        assert_eq!(
            workload_summary.get("WorkloadId").and_then(Value::as_str),
            Some(workload_id.as_str()),
            "case {case:?}: the recorded workload must name its id, got {summary:?}"
        );
        let live = client.get(&workload_id).await;
        assert_eq!(
            live.body
                .get("Workload")
                .and_then(|workload| workload.get(member))
                .cloned()
                .unwrap_or(Value::Null),
            value,
            "case {case:?}: the live workload must take the edit, got {live:?}"
        );
    }

    // The listed summary agrees with what `GetMilestone` reads back for the same
    // milestone: one record, two projections.
    let fetched = get_milestone(&client, &workload_id, 1).await;
    let milestone = fetched
        .body
        .get("Milestone")
        .cloned()
        .unwrap_or(Value::Null);
    let listed = summaries(&list_milestones(&client, &workload_id, json!({})).await, "agree")[0]
        .clone();
    for name in ["MilestoneNumber", "MilestoneName", "RecordedAt"] {
        assert_eq!(
            listed.get(name),
            milestone.get(name),
            "case \"agree\": {name} must agree between ListMilestones and GetMilestone, \
             got {listed:?} and {milestone:?}"
        );
    }
}

/// Rule 1, continued: a workload nobody recorded a milestone against lists an EMPTY page
/// — the workload exists, so the answer is a 200 with no summaries and no `NextToken`.
#[tokio::test]
async fn a_workload_with_no_milestones_lists_an_empty_page() {
    let client = Client::new();
    let workload_id = created_id(
        &client.create(minimal_create("no-milestones", "tok-none")).await,
        "no-milestones",
    );

    for (case, request) in [
        ("omitted", json!({})),
        ("max-results", json!({ "MaxResults": 1 })),
    ] {
        let answer = list_milestones(&client, &workload_id, request).await;
        let page = summaries(&answer, case);
        assert!(
            page.is_empty(),
            "case {case:?}: a workload with no milestones lists nothing, got {answer:?}"
        );
        assert!(
            answer.body.get("NextToken").is_none(),
            "case {case:?}: an empty page must omit NextToken, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("WorkloadId").and_then(Value::as_str),
            Some(workload_id.as_str()),
            "case {case:?}: the answer must name the workload listed, got {answer:?}"
        );
    }
}

/// Rule 2: the path `WorkloadId` is RESOLVED. A value naming no stored workload is the
/// lookup's 404 — never an empty page — and the rows differ along the id's syntax,
/// because resolution, not input validation, owns a path id.
#[tokio::test]
async fn listing_the_milestones_of_an_unknown_workload_is_a_not_found() {
    let client = Client::new();
    // A stored workload WITH milestones exists, so every row below is a genuine miss.
    workload_with_milestones(&client, "present", 2).await;

    for (case, workload_id) in [
        ("absent-but-well-formed", ABSENT_HEX_ID),
        ("uppercase-hex", "0123456789ABCDEF0123456789ABCDEF"),
        ("too-short", "0123456789abcdef"),
        ("too-long", "0123456789abcdef0123456789abcdef0"),
        ("not-hex", "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"),
    ] {
        let answer = list_milestones(&client, workload_id, json!({})).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: an unresolvable WorkloadId must be a 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("ResourceId").and_then(Value::as_str),
            Some(workload_id),
            "case {case:?}: the 404 must carry the id as sent, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("ResourceType").and_then(Value::as_str),
            Some("WORKLOAD"),
            "case {case:?}: expected ResourceType WORKLOAD, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Message").and_then(Value::as_str),
            Some(
                format!(
                    "[ResourceNotFound] No workload with ID {workload_id} was found in account {}.",
                    engine::emulator_account_id()
                )
                .as_str()
            ),
            "case {case:?}: expected the shared id/account not-found message, got {answer:?}"
        );
    }
}

/// Rule 3: a `NextToken` this service did not issue is a bad request, not an empty page.
/// The rows differ along how the token is malformed — including one that IS a token this
/// service issues, for `ListWorkloads`, whose cursor names no milestone.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused_with_400() {
    let client = Client::new();
    let workload_id = workload_with_milestones(&client, "tokens", 3).await;

    // A token for the OTHER paginated operation: same encoding, different cursor. It
    // needs a second workload to exist, so that `ListWorkloads` has a page to resume.
    created_id(
        &client.create(minimal_create("tokens-sibling", "tok-sibling")).await,
        "tokens-sibling",
    );
    let other = client.list(json!({ "MaxResults": 1 })).await;
    let workload_token = other
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("ListWorkloads must page when more than one workload exists")
        .to_string();

    for (case, token) in [
        ("not-a-cursor", "not-a-cursor".to_string()),
        ("hex-but-not-json", hex::encode("not json at all")),
        ("json-without-a-cursor", hex::encode("{}")),
        ("cursor-of-the-wrong-shape", hex::encode(r#"{"m":"one"}"#)),
        ("another-operations-token", workload_token),
    ] {
        let answer = list_milestones(&client, &workload_id, json!({ "NextToken": token })).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: an unissued NextToken must be a 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
    }

    // And a token this operation DID issue resumes the enumeration.
    let first = list_milestones(&client, &workload_id, json!({ "MaxResults": 1 })).await;
    let token = first
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let resumed = list_milestones(
        &client,
        &workload_id,
        json!({ "MaxResults": 1, "NextToken": token }),
    )
    .await;
    let page = summaries(&resumed, "resumed");
    assert_eq!(
        page.first().and_then(|s| s.get("MilestoneNumber")).and_then(Value::as_i64),
        Some(2),
        "case \"resumed\": a token this operation issued must resume just past the page it \
         ended, got {resumed:?}"
    );
}

/// Rule 4: listing is REPORTING. Repeated listings answer the same thing, every
/// milestone still reads back through `GetMilestone`, and the next `CreateMilestone`
/// takes the number it would have taken had nobody listed.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let client = Client::new();
    let recorded = 3i64;
    let workload_id = workload_with_milestones(&client, "reporting", recorded).await;

    let mut answers: Vec<Value> = Vec::new();
    for round in 0..3 {
        let case = format!("round-{round}");
        // Page over the whole set with a page size of one, so every cursor is exercised.
        let mut numbers: Vec<i64> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut request = json!({ "MaxResults": 1 });
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let answer = list_milestones(&client, &workload_id, request).await;
            for summary in summaries(&answer, &case) {
                numbers.push(
                    summary
                        .get("MilestoneNumber")
                        .and_then(Value::as_i64)
                        .unwrap_or_default(),
                );
            }
            next_token = answer
                .body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            numbers,
            (1..=recorded).collect::<Vec<i64>>(),
            "case {case:?}: every listing must report the same milestones, got {numbers:?}"
        );
        answers.push(
            list_milestones(&client, &workload_id, json!({}))
                .await
                .body,
        );
    }
    assert!(
        answers.windows(2).all(|pair| pair[0] == pair[1]),
        "case \"repeat\": two listings of the same state must answer identically, got {answers:?}"
    );

    for number in 1..=recorded {
        let fetched = get_milestone(&client, &workload_id, number).await;
        assert_eq!(
            fetched.status, 200,
            "case \"after-listing\": milestone {number} must still resolve, got {fetched:?}"
        );
    }

    let next = create_milestone(
        &client,
        &workload_id,
        "after-the-listings",
        "tok-after-listings",
    )
    .await;
    assert_eq!(
        next.body.get("MilestoneNumber").and_then(Value::as_i64),
        Some(recorded + 1),
        "case \"after-listing\": listing must not consume a milestone number, got {next:?}"
    );
}

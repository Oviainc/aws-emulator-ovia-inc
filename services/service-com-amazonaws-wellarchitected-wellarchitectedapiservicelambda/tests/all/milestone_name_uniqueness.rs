// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the uniqueness of a `MilestoneName` within its workload.
//!
//! The rule these pin, over a TABLE of spellings rather than one recorded request:
//! "Milestone names must be unique within a workload" (`smithy_query(query="shape",
//! shape="MilestoneName")`), decided by the same loose comparison this service applies to
//! every name it meters for uniqueness — capitalization and whitespace are ignored — so a
//! spelling that differs from a recorded milestone's only in either is a modelled 409
//! `ConflictException` scoped to that workload, it consumes no milestone number, and the
//! SAME name is still free in a different workload.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use serde_json::{json, Value};

use super::workload_harness::{created_id, minimal_create, Answer, Client};

/// `MilestoneName` is `@length(min: 3, max: 100)` (`smithy_query(query="shape",
/// shape="MilestoneName")`), so every spelling a row submits stays inside that bound.
const MILESTONE_NAME_RANGE: (usize, usize) = (3, 100);

fn member<'a>(answer: &'a Answer, name: &str) -> &'a str {
    answer.body.get(name).and_then(Value::as_str).unwrap_or("")
}

async fn create_milestone(client: &Client, workload_id: &str, name: &str, token: &str) -> Answer {
    client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/milestones"),
            Some(json!({ "MilestoneName": name, "ClientRequestToken": token })),
        )
        .await
}

async fn list_milestones(client: &Client, workload_id: &str) -> Vec<Value> {
    let answer = client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/milestonesSummaries"),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "expected ListMilestones 200, got {answer:?}"
    );
    answer
        .body
        .get("MilestoneSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

async fn workload(client: &Client, name: &str) -> String {
    created_id(
        &client.create(minimal_create(name, &format!("tok-{name}"))).await,
        name,
    )
}

/// The refusal a duplicate name gets: 409 `ConflictException`, `ResourceType`
/// `MILESTONE`, the `ResourceId` of the milestone already holding the name, and a message
/// naming the name AS SUBMITTED by the refused request, the workload the uniqueness is
/// scoped to and the comparison rule.
fn assert_name_conflict(
    case: &str,
    answer: &Answer,
    submitted_name: &str,
    workload_id: &str,
    stored_number: i64,
) {
    assert_eq!(
        answer.status, 409,
        "case {case:?}: expected HTTP 409, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ConflictException",
        "case {case:?}: expected ConflictException, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceType"),
        "MILESTONE",
        "case {case:?}: expected ResourceType MILESTONE, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceId"),
        format!("{workload_id}.{stored_number}"),
        "case {case:?}: ResourceId must name the milestone holding the name, got {answer:?}"
    );
    assert_eq!(
        member(answer, "Message"),
        format!(
            "[Conflict] Milestone with name {submitted_name} already exists under workload \
             {workload_id}. Capitalization and whitespace are ignored in milestone names."
        ),
        "case {case:?}: the refusal must quote the submitted name, name its workload and \
         the comparison rule, got {answer:?}"
    );
}

/// Every spelling that differs from a recorded milestone's name only in capitalization or
/// whitespace is refused the same way, and the refusals consume no milestone number: the
/// next genuinely different name is numbered 2. The rows differ along the stored
/// spelling (plain, with inner spaces, mixed case) and, for each, along how the duplicate
/// is respelled (identical, upper, lower, padded, whitespace removed, tab-separated).
#[tokio::test]
async fn a_milestone_name_is_unique_within_its_workload_ignoring_case_and_whitespace() {
    let client = Client::new();

    for (index, stored_name) in ["ms-plain", "ms with inner spaces", "MiXeD Case Name"]
        .iter()
        .enumerate()
    {
        let case_host = format!("host-{index}");
        let workload_id = workload(&client, &case_host).await;

        let first = create_milestone(
            &client,
            &workload_id,
            stored_name,
            &format!("tok-first-{index}"),
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {stored_name:?}: the first CreateMilestone must succeed, got {first:?}"
        );
        assert_eq!(
            first.body.get("MilestoneNumber").and_then(Value::as_i64),
            Some(1),
            "case {stored_name:?}: the first milestone of a workload is number 1, got {first:?}"
        );

        for (spelling, submitted) in [
            ("identical", (*stored_name).to_string()),
            ("upper", stored_name.to_uppercase()),
            ("lower", stored_name.to_lowercase()),
            ("padded", format!("   {stored_name} ")),
            ("whitespace-removed", stored_name.replace(' ', "")),
            ("tab-separated", stored_name.replace(' ', "\t")),
        ] {
            let case = format!("{stored_name:?}/{spelling}");
            assert!(
                (MILESTONE_NAME_RANGE.0..=MILESTONE_NAME_RANGE.1).contains(&submitted.len()),
                "case {case:?}: the row must stay inside \
                 MilestoneName @length{MILESTONE_NAME_RANGE:?}, got {submitted:?}"
            );
            let refused = create_milestone(
                &client,
                &workload_id,
                &submitted,
                &format!("tok-dup-{index}-{spelling}"),
            )
            .await;
            assert_name_conflict(&case, &refused, &submitted, &workload_id, 1);
        }

        // A refused create records nothing, so the number it would have taken is still
        // free: the next ACCEPTED milestone of this workload is number 2.
        let accepted = create_milestone(
            &client,
            &workload_id,
            &format!("{stored_name}-second"),
            &format!("tok-second-{index}"),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {stored_name:?}: a genuinely different name must be accepted, got {accepted:?}"
        );
        assert_eq!(
            accepted.body.get("MilestoneNumber").and_then(Value::as_i64),
            Some(2),
            "case {stored_name:?}: a refused create must consume no milestone number, \
             got {accepted:?}"
        );

        let listed = list_milestones(&client, &workload_id).await;
        assert_eq!(
            listed.len(),
            2,
            "case {stored_name:?}: only the two accepted milestones must be recorded, \
             got {listed:?}"
        );
    }
}

/// The uniqueness is scoped to ONE workload: the same spelling, and every spelling that
/// collides with it, is free in another workload. The rows differ along the spelling the
/// second workload submits.
#[tokio::test]
async fn the_same_milestone_name_is_free_in_another_workload() {
    let client = Client::new();
    const STORED_NAME: &str = "Shared Milestone Name";

    let first_workload = workload(&client, "scope-first").await;
    let recorded = create_milestone(&client, &first_workload, STORED_NAME, "tok-scope-first").await;
    assert_eq!(
        recorded.status, 200,
        "the first CreateMilestone must succeed, got {recorded:?}"
    );

    for (index, (spelling, submitted)) in [
        ("identical", STORED_NAME.to_string()),
        ("upper", STORED_NAME.to_uppercase()),
        ("whitespace-removed", STORED_NAME.replace(' ', "")),
    ]
    .into_iter()
    .enumerate()
    {
        let other = workload(&client, &format!("scope-other-{index}")).await;
        let accepted = create_milestone(
            &client,
            &other,
            &submitted,
            &format!("tok-scope-other-{index}"),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {spelling:?}: a name recorded in another workload must be free here, \
             got {accepted:?}"
        );
        assert_eq!(
            accepted.body.get("MilestoneNumber").and_then(Value::as_i64),
            Some(1),
            "case {spelling:?}: it is this workload's first milestone, got {accepted:?}"
        );

        // …and inside THAT workload it is now taken, whichever spelling it was stored
        // under: the refusal quotes the name THIS request submitted.
        let refused = create_milestone(
            &client,
            &other,
            STORED_NAME,
            &format!("tok-scope-other-dup-{index}"),
        )
        .await;
        assert_name_conflict(
            &format!("{spelling}/repeat"),
            &refused,
            STORED_NAME,
            &other,
            1,
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the pillar order a workload is reviewed in, across the
//! paths that WRITE it: `CreateWorkload` and `UpdateWorkload`.
//!
//! The rule pinned here, over a table of inputs rather than one recorded request: a
//! workload is always reviewed in SOME pillar order, so a request that names no order —
//! whether by omitting `PillarPriorities` or by carrying an empty list — leaves the
//! workload in the order it already has (the framework's own order for a workload being
//! created), and a request that names one is stored exactly as it named it. Every reader
//! of the workload reports that same order, including a milestone recorded afterwards.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use serde_json::{json, Value};

use super::workload_harness::{created_id, minimal_create, Answer, Client};

/// The pillar order the framework itself is reviewed in, by `PillarId`
/// (`smithy_query(query="shape", shape="PillarId")`) — the order a workload that names
/// none is created with. Spelled out here rather than read back out of the code under
/// test.
fn framework_pillar_order() -> Value {
    json!([
        "operationalExcellence",
        "security",
        "reliability",
        "performance",
        "costOptimization",
        "sustainability"
    ])
}

/// Every framework pillar, in the reverse of the framework's own order: an order a
/// request names that no default could have produced.
fn reversed_pillar_order() -> Value {
    json!([
        "sustainability",
        "costOptimization",
        "performance",
        "reliability",
        "security",
        "operationalExcellence"
    ])
}

fn workload_of(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {answer:?}"
    );
    answer
        .body
        .get("Workload")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: Workload must be present, got {answer:?}"))
}

/// The `PillarPriorities` of a projected workload, as sent on the wire.
fn pillar_priorities(workload: &Value) -> Value {
    workload
        .get("PillarPriorities")
        .cloned()
        .unwrap_or(Value::Null)
}

/// `CreateMilestone`: `POST /workloads/{WorkloadId}/milestones`.
async fn create_milestone(client: &Client, workload_id: &str, name: &str) -> Answer {
    client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/milestones"),
            Some(json!({ "MilestoneName": name, "ClientRequestToken": format!("tok-{name}") })),
        )
        .await
}

/// `GetMilestone`: `GET /workloads/{WorkloadId}/milestones/{MilestoneNumber}`.
async fn get_milestone(client: &Client, workload_id: &str, milestone_number: i64) -> Answer {
    client
        .send(
            "GET",
            &format!("/workloads/{workload_id}/milestones/{milestone_number}"),
            None,
        )
        .await
}

/// A request that names no pillar order leaves the workload in the order it already has;
/// one that names an order is stored as named. Rows differ along the order the workload
/// was CREATED with (none, one pillar, two pillars, every pillar reversed) and what the
/// update names (the member absent, an explicitly empty list, one pillar, every pillar
/// reversed), and each is read back through `GetWorkload` and through a milestone
/// recorded after the update.
#[tokio::test]
async fn an_update_leaves_a_workload_reviewed_in_some_pillar_order() {
    // (case, PillarPriorities the create names, PillarPriorities the update names,
    //  the order every reader must then report)
    let cases: Vec<(&str, Option<Value>, Option<Value>, Value)> = vec![
        // The case the report cites: an empty list over a workload created with no
        // order of its own.
        (
            "empty-over-default",
            None,
            Some(json!([])),
            framework_pillar_order(),
        ),
        (
            "empty-over-one-pillar",
            Some(json!(["security"])),
            Some(json!([])),
            json!(["security"]),
        ),
        (
            "empty-over-every-pillar-reversed",
            Some(reversed_pillar_order()),
            Some(json!([])),
            reversed_pillar_order(),
        ),
        // An absent member is the same request as far as the pillar order goes: it names
        // none, so the stored order stands.
        (
            "absent-over-two-pillars",
            Some(json!(["sustainability", "security"])),
            None,
            json!(["sustainability", "security"]),
        ),
        ("absent-over-default", None, None, framework_pillar_order()),
        (
            "two-pillars-over-default",
            None,
            Some(json!(["costOptimization", "security"])),
            json!(["costOptimization", "security"]),
        ),
        (
            "one-pillar-over-one-pillar",
            Some(json!(["security"])),
            Some(json!(["sustainability"])),
            json!(["sustainability"]),
        ),
        (
            "every-pillar-reversed-over-one-pillar",
            Some(json!(["security"])),
            Some(reversed_pillar_order()),
            reversed_pillar_order(),
        ),
    ];

    for (case, created, updated, expected) in cases {
        let client = Client::new();
        let mut request = minimal_create(&format!("pillar-{case}"), &format!("tok-{case}"));
        if let Some(created) = created.clone() {
            request["PillarPriorities"] = created;
        }
        let workload_id = created_id(&client.create(request).await, case);

        // The update always writes a member, so the pillar order is the only thing the
        // rows differ in: `Notes` keeps every row a real update even where it names no
        // order.
        let mut body = json!({ "Notes": "touched" });
        if let Some(updated) = updated.clone() {
            body["PillarPriorities"] = updated;
        }
        let answered = workload_of(&client.update(&workload_id, body).await, case);
        assert_eq!(
            pillar_priorities(&answered),
            expected,
            "case {case:?}: UpdateWorkload must answer PillarPriorities {expected:?}, got {:?}",
            pillar_priorities(&answered)
        );

        let fetched = workload_of(&client.get(&workload_id).await, case);
        assert_eq!(
            pillar_priorities(&fetched),
            expected,
            "case {case:?}: GetWorkload must report PillarPriorities {expected:?}, got {:?}",
            pillar_priorities(&fetched)
        );
        assert_ne!(
            pillar_priorities(&fetched),
            json!([]),
            "case {case:?}: a workload is always reviewed in some pillar order, got {fetched:?}"
        );

        let recorded = create_milestone(&client, &workload_id, &format!("ms-{case}")).await;
        let milestone_number = recorded
            .body
            .get("MilestoneNumber")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| {
                panic!("case {case:?}: CreateMilestone must answer a number, got {recorded:?}")
            });
        let milestone = get_milestone(&client, &workload_id, milestone_number).await;
        assert_eq!(
            milestone.status, 200,
            "case {case:?}: expected GetMilestone 200, got {milestone:?}"
        );
        let snapshot = milestone
            .body
            .get("Milestone")
            .and_then(|milestone| milestone.get("Workload"))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            pillar_priorities(&snapshot),
            expected,
            "case {case:?}: the recorded milestone must report PillarPriorities {expected:?}, got {snapshot:?}"
        );
    }
}

/// Naming no pillar order is IDEMPOTENT: however many updates carry an empty list, the
/// workload stays in the order it was in, and an order named in between is what the next
/// empty list preserves. Rows differ along how many empty-list updates run and what the
/// order was set to before them.
#[tokio::test]
async fn repeated_updates_naming_no_order_never_empty_the_pillar_order() {
    let cases: Vec<(&str, Option<Value>, usize, Value)> = vec![
        ("default-once", None, 1, framework_pillar_order()),
        ("default-three-times", None, 3, framework_pillar_order()),
        (
            "named-twice",
            Some(json!(["reliability", "performance"])),
            2,
            json!(["reliability", "performance"]),
        ),
        (
            "every-pillar-reversed-five-times",
            Some(reversed_pillar_order()),
            5,
            reversed_pillar_order(),
        ),
    ];

    for (case, created, rounds, expected) in cases {
        let client = Client::new();
        let mut request = minimal_create(&format!("repeat-{case}"), &format!("tok-repeat-{case}"));
        if let Some(created) = created.clone() {
            request["PillarPriorities"] = created;
        }
        let workload_id = created_id(&client.create(request).await, case);

        for round in 0..rounds {
            let answered = workload_of(
                &client
                    .update(&workload_id, json!({ "PillarPriorities": [] }))
                    .await,
                case,
            );
            assert_eq!(
                pillar_priorities(&answered),
                expected,
                "case {case:?}: round {round} must leave PillarPriorities {expected:?}, got {:?}",
                pillar_priorities(&answered)
            );
        }

        // An order named after the empty updates still takes effect, and the next empty
        // update preserves THAT order rather than reverting to the framework's.
        let named = json!(["costOptimization"]);
        let answered = workload_of(
            &client
                .update(&workload_id, json!({ "PillarPriorities": named.clone() }))
                .await,
            case,
        );
        assert_eq!(
            pillar_priorities(&answered),
            named,
            "case {case:?}: a named order must be stored as named, got {:?}",
            pillar_priorities(&answered)
        );
        let answered = workload_of(
            &client
                .update(&workload_id, json!({ "PillarPriorities": [] }))
                .await,
            case,
        );
        assert_eq!(
            pillar_priorities(&answered),
            named,
            "case {case:?}: an empty list must preserve the named order {named:?}, got {:?}",
            pillar_priorities(&answered)
        );
    }
}

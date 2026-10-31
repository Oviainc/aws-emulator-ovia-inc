// SPDX-License-Identifier: CC0-1.0

//! `UpdatePricingPlan` for a call that leaves the plan exactly as it stood, end to end
//! over the service's real `restJson1` wire through [`MultiServiceRouter::dispatch`] —
//! the same path the production router serves.
//!
//! RULE: `LastModifiedTime` is "the most recent time when the pricing plan was modified"
//! (`smithy_query(query="shape",shape="UpdatePricingPlanOutput")`), and the generation of
//! the plan a period-scoped read reports is the one the write that left it was made in —
//! so a write that leaves every member of the plan at the value state already holds
//! modified nothing: the instant stays where it was and no generation is frozen or
//! opened. A write that asks for any value the plan does not hold moves both, and the new
//! value reads back.
//!
//! `UpdatePricingPlanInput` makes every member but `Arn` optional
//! (`smithy_query(query="shape",shape="UpdatePricingPlanInput")`), so "changes nothing"
//! spans a request naming no member at all as well as one restating the values the plan
//! carries, member by member and both at once. The rows below differ along which member
//! the request names, how many of them it names, whether the value asked for is the one
//! the plan holds, and whether the plan holds a description at all — including the empty
//! string the member's length of 0 admits, which is a value and not an omission.
//!
//! `tests/all/pricing_rule_unchanged_update.rs` and
//! `tests/all/update_billing_group_no_op_generations.rs` own the same rule for the
//! sibling resources; `tests/all/pricing_plan_billing_periods.rs` owns the generations a
//! write that DID change the plan leaves behind, and owns the per-period read these rows
//! verify through.
//!
//! The wire cannot write in a PAST billing period — a write happens now — so the
//! generation rows write through the real wire and then backdate the stored instants,
//! which is the one thing the test cannot ask the service for.

use serde_json::{json, Value};

use crate::pricing_plan_billing_periods::{assert_members, element_in};
use crate::pricing_rule_association_periods::{period_start_millis, shift_period, Emulator};

/// `UpdatePricingPlan`, at the method its modelled HTTP binding names
/// (`smithy_query(query="operation",operation="UpdatePricingPlan")`: a `PUT`).
const UPDATE_PATH: &str = "/update-pricing-plan";

/// `CreatePricingPlan`, which gives these rows a plan to leave alone.
const CREATE_PATH: &str = "/create-pricing-plan";

/// The billing period the service calls current, read from the same clock it answers
/// from so an expectation holds whatever month the suite runs in.
fn current_period() -> String {
    service_com_amazonaws_billingconductor_awsbillingconductor::state::current_billing_period()
}

/// Wall-clock millis since the epoch, read from the same clock the service stamps
/// `LastModifiedTime` from, so a row can bracket the instant a call must report.
fn now_millis() -> i64 {
    service_com_amazonaws_billingconductor_awsbillingconductor::state::now_millis()
}

/// The `UpdatePricingPlan` members a row names beside `Arn`, built from the plan's
/// current name so a row can restate the value the plan already holds.
type Patch = fn(&str) -> Value;

/// One row of the table: the plan the row starts from, the update it makes, whether that
/// update asks for anything the plan does not already hold, and the plan it must leave.
struct Row {
    label: &'static str,
    /// `CreatePricingPlan` members beside `Name`.
    create: Value,
    patch: Patch,
    /// Does the request ask for any value the plan does not already hold?
    changes: bool,
    /// The members a read must report once the call is served.
    after: Value,
    /// The name the plan carries afterwards, when the row renames it.
    renamed_to: Option<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "no member named at all",
            create: json!({"Description": "as created"}),
            patch: |_| json!({}),
            changes: false,
            after: json!({"Description": "as created"}),
            renamed_to: None,
        },
        Row {
            label: "no member named at all, on a plan holding no description",
            create: json!({}),
            patch: |_| json!({}),
            changes: false,
            after: json!({"Description": null}),
            renamed_to: None,
        },
        Row {
            label: "the Name the plan already holds",
            create: json!({"Description": "as created"}),
            patch: |name| json!({"Name": name}),
            changes: false,
            after: json!({"Description": "as created"}),
            renamed_to: None,
        },
        Row {
            label: "the Description it already holds",
            create: json!({"Description": "as created"}),
            patch: |_| json!({"Description": "as created"}),
            changes: false,
            after: json!({"Description": "as created"}),
            renamed_to: None,
        },
        Row {
            label: "both members at the values it already holds, at once",
            create: json!({"Description": "as created"}),
            patch: |name| json!({"Name": name, "Description": "as created"}),
            changes: false,
            after: json!({"Description": "as created"}),
            renamed_to: None,
        },
        Row {
            // The member's modelled length is 0..1024, so the empty string is a value a
            // plan can hold, and restating it asks for what the plan already carries.
            label: "the empty Description a plan already holding it carries",
            create: json!({"Description": ""}),
            patch: |_| json!({"Description": ""}),
            changes: false,
            after: json!({"Description": ""}),
            renamed_to: None,
        },
        Row {
            label: "a new Description",
            create: json!({"Description": "as created"}),
            patch: |_| json!({"Description": "moved on"}),
            changes: true,
            after: json!({"Description": "moved on"}),
            renamed_to: None,
        },
        Row {
            label: "a Description emptied, which is a value and not an omission",
            create: json!({"Description": "as created"}),
            patch: |_| json!({"Description": ""}),
            changes: true,
            after: json!({"Description": ""}),
            renamed_to: None,
        },
        Row {
            label: "a Description given to a plan holding none",
            create: json!({}),
            patch: |_| json!({"Description": "added later"}),
            changes: true,
            after: json!({"Description": "added later"}),
            renamed_to: None,
        },
        Row {
            label: "a new Name",
            create: json!({"Description": "as created"}),
            patch: |_| json!({"Name": "PlanRenamed"}),
            changes: true,
            after: json!({"Description": "as created"}),
            renamed_to: Some("PlanRenamed"),
        },
        Row {
            label: "a new Name and a new Description at once",
            create: json!({"Description": "as created"}),
            patch: |_| json!({"Name": "PlanRenamedAgain", "Description": "moved on"}),
            changes: true,
            after: json!({"Description": "moved on"}),
            renamed_to: Some("PlanRenamedAgain"),
        },
    ]
}

/// `CreatePricingPlan` with the row's members beside `Name`, answering the ARN the
/// service minted.
async fn create_plan(em: &Emulator, case: &str, name: &str, create: &Value) -> String {
    let mut body = create.clone();
    body["Name"] = json!(name);
    let (status, answer) = em.post(CREATE_PATH, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: setup CreatePricingPlan expected 200, got {status} with {answer:?}"
    );
    answer["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: setup CreatePricingPlan reported no Arn"))
        .to_string()
}

/// Send the row's `UpdatePricingPlan` against `plan` and answer the status and body.
async fn update(em: &Emulator, plan: &str, patch: &Value) -> (u16, Value) {
    let mut body = patch.clone();
    body["Arn"] = json!(plan);
    em.put(UPDATE_PATH, &body).await
}

#[tokio::test]
async fn last_modified_time_moves_only_when_the_update_changed_the_plan() {
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.label;
        // One store per row, so a row that renames its plan cannot collide with the names
        // of the rows beside it.
        let em = Emulator::new();
        let name = format!("ClockPlan{index:03}");
        let plan = create_plan(&em, case, &name, &row.create).await;

        let created = element_in(&em, case, &plan, None)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the created plan is reported"));
        let creation_time = created["CreationTime"].as_i64().unwrap_or_default();
        let modified_before = created["LastModifiedTime"].as_i64().unwrap_or_default();
        // The clock the service stamps from has millisecond resolution, so a move is only
        // observable once the call lands at a later instant than the create.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        let before_call = now_millis();
        let (status, response) = update(&em, &plan, &(row.patch)(&name)).await;
        let after_call = now_millis();
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {response:?}"
        );
        let reported = response["LastModifiedTime"].as_i64().unwrap_or_default();

        let listed = element_in(&em, case, &plan, None)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the plan exists, so it is reported"));
        assert_eq!(
            listed["LastModifiedTime"].as_i64(),
            Some(reported),
            "case {case:?}: the answer and the stored plan must agree on LastModifiedTime, \
             got {listed:?}"
        );
        if row.changes {
            assert!(
                (before_call..=after_call).contains(&reported),
                "case {case:?}: expected a call that changed the plan to stamp \
                 LastModifiedTime inside its own window [{before_call}, {after_call}], got \
                 {reported}"
            );
        } else {
            assert_eq!(
                reported, modified_before,
                "case {case:?}: expected a call that changed nothing to leave \
                 LastModifiedTime at {modified_before}, got {reported}"
            );
        }

        // Whatever the call did to the instant, the plan reads back as the call left it:
        // the members it asked for, the ones it named nothing about, and the creation
        // instant no update is ever allowed to touch.
        assert_members(case, &listed, &row.after);
        assert_members(case, &response, &row.after);
        assert_eq!(
            listed["Name"].as_str(),
            Some(row.renamed_to.unwrap_or(&name)),
            "case {case:?}: expected the name the call left, got {listed:?}"
        );
        assert_eq!(
            listed["CreationTime"].as_i64(),
            Some(creation_time),
            "case {case:?}: the creation instant is never an update's to change, got {listed:?}"
        );
    }
}

/// One row of the generation table: how far back the plan's current generation started,
/// the update made NOW, and whether that update changes anything.
struct GenerationRow {
    label: &'static str,
    /// Months back from the current billing period that the plan was created in.
    months_back: i64,
    patch: Patch,
    changes: bool,
}

#[tokio::test]
async fn an_update_that_changed_nothing_opens_no_generation() {
    // Rows differ along how far back the plan's generation started (the month before, a
    // year, two years — crossing a year boundary) and along whether the call made now
    // asks for anything the plan does not hold.
    let rows = [
        GenerationRow {
            label: "no member named, a generation old one month",
            months_back: -1,
            patch: |_| json!({}),
            changes: false,
        },
        GenerationRow {
            label: "the Name it already holds, a generation old one year",
            months_back: -12,
            patch: |name| json!({"Name": name}),
            changes: false,
        },
        GenerationRow {
            label: "both values it already holds, a generation old two years",
            months_back: -24,
            patch: |name| json!({"Name": name, "Description": "as created"}),
            changes: false,
        },
        GenerationRow {
            label: "a new Description, a generation old one month",
            months_back: -1,
            patch: |_| json!({"Description": "moved on"}),
            changes: true,
        },
        GenerationRow {
            label: "a new Name, a generation old two years",
            months_back: -24,
            patch: |_| json!({"Name": "GenerationRenamed"}),
            changes: true,
        },
    ];

    let now = current_period();
    for (index, row) in rows.iter().enumerate() {
        let case = row.label;
        let em = Emulator::new();
        let created_in = shift_period(&now, row.months_back);
        let name = format!("GenerationPlan{index:03}");
        let plan = create_plan(&em, case, &name, &json!({"Description": "as created"})).await;
        // The plan as if it had been created — and so last written — in a billing period
        // that is already closed, which is the world in which a write made now either
        // opens a generation or does not.
        em.create_plan_in(&plan, &created_in);
        let generation_start = period_start_millis(&created_in);
        let generations_before = em.superseded_plan_generations(&plan);

        let (status, response) = update(&em, &plan, &(row.patch)(&name)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {response:?}"
        );
        let generations_after = em.superseded_plan_generations(&plan);

        // The period the plan was created in still reports it exactly as it was created:
        // the generation in force then is frozen whatever the write did.
        let earlier = element_in(&em, case, &plan, Some(&created_in))
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the plan existed then, so it is reported"));
        assert_members(
            case,
            &earlier,
            &json!({"Name": name, "Description": "as created"}),
        );
        assert_eq!(
            earlier["LastModifiedTime"].as_i64(),
            Some(generation_start),
            "case {case:?}: expected the closed period to report the generation's own \
             instant {generation_start}, got {earlier:?}"
        );

        // The current period is where the two cases part: a write that changed the plan
        // opens a generation there, spending a slot of the bounded version history, while
        // a write that changed nothing leaves the generation that was already in force —
        // so the current period still reports the instant of the write that really did
        // leave the plan as it stands.
        let current = element_in(&em, case, &plan, Some(&now))
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the plan exists now, so it is reported"));
        let reported = current["LastModifiedTime"].as_i64().unwrap_or_default();
        if row.changes {
            assert!(
                reported > generation_start,
                "case {case:?}: expected the write that changed the plan to open a \
                 generation past {generation_start}, got {reported}"
            );
            assert_eq!(
                generations_after,
                generations_before + 1,
                "case {case:?}: expected a write that changed the plan in a later billing \
                 period to freeze the generation it displaced, got {generations_before} \
                 superseded generations before and {generations_after} after"
            );
        } else {
            assert_eq!(
                reported, generation_start,
                "case {case:?}: expected a write that changed nothing to leave the \
                 generation that started at {generation_start} in force, got {reported}"
            );
            assert_eq!(
                generations_after, generations_before,
                "case {case:?}: expected a write that changed nothing to freeze no \
                 generation, got {generations_before} superseded generations before and \
                 {generations_after} after"
            );
            assert_members(
                case,
                &current,
                &json!({"Name": name, "Description": "as created"}),
            );
        }
        // A read that names no period asks about the current one, so the two agree.
        let unscoped = element_in(&em, case, &plan, None)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the plan exists now, so it is reported"));
        assert_eq!(
            unscoped["LastModifiedTime"].as_i64(),
            Some(reported),
            "case {case:?}: a read naming no period asks about {now:?}, got {unscoped:?}"
        );
    }
}

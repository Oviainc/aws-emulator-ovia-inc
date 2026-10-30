// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for AWS Deadline Cloud's budget plane.
//!
//! Three rules are under test here.
//!
//! 1. A member carrying `@timestampFormat("date-time")` travels as an RFC 3339 STRING on
//!    the restJson1 wire, in both directions — including a member reached through a union
//!    (`BudgetSchedule$fixed -> FixedBudgetSchedule$startTime/endTime`). Every form an SDK
//!    can put on the wire for such a member is accepted, and what comes back is a string.
//! 2. A budget request body that violates a modeled numeric `@range` or list `@length`
//!    bound is refused with HTTP 400 `ValidationException` whose message is the body
//!    validator's bracketed finding list — the bound as the model declares it and the
//!    value the request sent — and the refused request stores nothing. The request AT the
//!    bound succeeds.
//! 3. A budget's status moves `ACTIVE -> INACTIVE` once and never back, and the refused
//!    reactivation changes nothing.
//!
//! The harness (`Emulator`) is the one `farm_queue_job` already drives the service with,
//! so every call goes through routing, deserialization, constraint validation, the handler
//! and serialization, exactly as the router does in production.

use serde_json::{json, Value};

use crate::farm_queue_job::{error_code, id_of, string_at, Emulator};

/// `POST`/`GET` collection path of the budget resource, from
/// `smithy_query(query="operation", operation="CreateBudget")`.
fn budgets_path(farm_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/budgets")
}

/// Single-budget path, from `smithy_query(query="operation", operation="GetBudget")`.
fn budget_path(farm_id: &str, budget_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/budgets/{budget_id}")
}

/// One budget action at `threshold`: a legal `BudgetActionType` at the given
/// `ThresholdPercentage` (`smithy_query(query="shape", shape="BudgetActionType,BudgetActionToAdd")`).
fn action(threshold: f64) -> Value {
    json!({"type": "STOP_SCHEDULING_AND_COMPLETE_TASKS", "thresholdPercentage": threshold})
}

/// The one budget action every request that is not about actions carries.
fn one_action() -> Value {
    action(80.0)
}

/// A `CreateBudget` body, with every member the model requires.
fn create_body(display_name: &str, queue_id: &str, schedule: Value, actions: Value, limit: Value) -> Value {
    json!({
        "displayName": display_name,
        "description": "a budget",
        "usageTrackingResource": {"queueId": queue_id},
        "approximateDollarLimit": limit,
        "actions": actions,
        "schedule": schedule,
    })
}

/// A fixed schedule carrying the two `date-time` members as an SDK serializes them.
fn fixed_schedule(start: &str, end: &str) -> Value {
    json!({"fixed": {"startTime": start, "endTime": end}})
}

/// The `message` member of a refusal body.
fn message_of(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a message member in {body}"))
        .to_string()
}

/// True when `id` is `budget-` plus 32 lowercase hex characters, which is the modeled
/// `^budget-[0-9a-f]{32}$` pattern of `BudgetId`.
fn is_budget_id(id: &str) -> bool {
    match id.strip_prefix("budget-") {
        Some(suffix) => {
            suffix.len() == 32 && suffix.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        }
        None => false,
    }
}

/// A farm with one queue to track usage against.
async fn farm_with_queue(emu: &Emulator, label: &str) -> (String, String) {
    let farm_id = emu.farm(&format!("{label}-farm")).await;
    let queue_id = emu.queue(&farm_id, &format!("{label}-queue")).await;
    (farm_id, queue_id)
}

/// The `fixed` schedule of a budget read back, as `{startTime, endTime}`.
fn schedule_of(body: &Value) -> &Value {
    body.get("schedule")
        .and_then(|schedule| schedule.get("fixed"))
        .unwrap_or_else(|| panic!("expected schedule.fixed in {body}"))
}

/// Rule 1 — every RFC 3339 form an SDK puts on the wire for a `date-time` member is
/// accepted through the union, and comes back as a string naming the same instant.
///
/// The rows differ along what the rule generalises over: sub-second precision, and the
/// era of the instant (the epoch itself, a leap day, past the turn of the century).
/// `expected` is the instant as the `date-time` format renders it. A numeric UTC offset
/// is deliberately NOT a row: the framework's own date-time reader refuses one
/// ("Smithy does not support timezone offsets in RFC-3339 date times"), and no AWS SDK
/// serializes one, so the `Z` form is the whole of the wire contract.
#[tokio::test]
async fn a_date_time_schedule_round_trips_as_an_rfc3339_string() {
    // case, startTime sent, endTime sent, startTime expected back, endTime expected back
    let cases: [(&str, &str, &str, &str, &str); 5] = [
        (
            "utc zulu",
            "2026-01-01T00:00:00Z",
            "2026-01-02T00:00:00Z",
            "2026-01-01T00:00:00Z",
            "2026-01-02T00:00:00Z",
        ),
        (
            "sub-second precision",
            "2026-03-04T05:06:07.123Z",
            "2026-03-05T05:06:07.125Z",
            "2026-03-04T05:06:07.123Z",
            "2026-03-05T05:06:07.125Z",
        ),
        (
            "a leap day",
            "2028-02-29T12:00:00Z",
            "2028-03-01T12:00:00Z",
            "2028-02-29T12:00:00Z",
            "2028-03-01T12:00:00Z",
        ),
        (
            "past the turn of the century",
            "2099-12-31T23:59:59Z",
            "2100-01-01T00:00:00Z",
            "2099-12-31T23:59:59Z",
            "2100-01-01T00:00:00Z",
        ),
        (
            "the unix epoch itself",
            "1970-01-01T00:00:00Z",
            "1970-01-02T00:00:00Z",
            "1970-01-01T00:00:00Z",
            "1970-01-02T00:00:00Z",
        ),
    ];

    let emu = Emulator::new();
    let (farm_id, queue_id) = farm_with_queue(&emu, "schedule").await;

    for (case, start, end, expect_start, expect_end) in cases {
        let (status, body) = emu
            .post(
                &budgets_path(&farm_id),
                create_body(
                    case,
                    &queue_id,
                    fixed_schedule(start, end),
                    json!([one_action()]),
                    json!(10.0),
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBudget to answer 200, got {status} with {body}"
        );
        let budget_id = id_of(&body, "budgetId");
        assert!(
            is_budget_id(&budget_id),
            "case {case:?}: expected a ^budget-[0-9a-f]{{32}}$ id, got {budget_id:?}"
        );

        let (status, read) = emu.get(&budget_path(&farm_id, &budget_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetBudget to answer 200, got {status} with {read}"
        );
        let schedule = schedule_of(&read);
        assert_eq!(
            schedule.get("startTime").and_then(Value::as_str),
            Some(expect_start),
            "case {case:?}: expected startTime {expect_start:?} as a string, got {schedule}"
        );
        assert_eq!(
            schedule.get("endTime").and_then(Value::as_str),
            Some(expect_end),
            "case {case:?}: expected endTime {expect_end:?} as a string, got {schedule}"
        );
        // The same rule holds for this response's own `date-time` members.
        assert!(
            read.get("createdAt").and_then(Value::as_str).is_some(),
            "case {case:?}: expected createdAt as an RFC 3339 string, got {read}"
        );
    }
}

/// Rule 1, continued — what `CreateBudget` stored is what `GetBudget` and `ListBudgets`
/// report, member for member.
#[tokio::test]
async fn a_created_budget_reads_back_with_everything_it_was_given() {
    let emu = Emulator::new();
    let (farm_id, queue_id) = farm_with_queue(&emu, "roundtrip").await;

    let actions = json!([
        {"type": "STOP_SCHEDULING_AND_COMPLETE_TASKS", "thresholdPercentage": 50.0},
        {"type": "STOP_SCHEDULING_AND_CANCEL_TASKS", "thresholdPercentage": 90.0, "description": "hard stop"},
    ]);
    let (status, body) = emu
        .post(
            &budgets_path(&farm_id),
            create_body(
                "quarterly",
                &queue_id,
                fixed_schedule("2026-02-01T00:00:00Z", "2026-05-01T00:00:00Z"),
                actions.clone(),
                json!(250.5),
            ),
        )
        .await;
    assert_eq!(status, 200, "expected CreateBudget to answer 200, got {status} with {body}");
    let budget_id = id_of(&body, "budgetId");

    let (status, read) = emu.get(&budget_path(&farm_id, &budget_id)).await;
    assert_eq!(status, 200, "expected GetBudget to answer 200, got {status} with {read}");
    assert_eq!(
        string_at(&read, "displayName"),
        "quarterly",
        "expected the displayName that was created, got {read}"
    );
    assert_eq!(
        string_at(&read, "status"),
        "ACTIVE",
        "expected a new budget to be ACTIVE, got {read}"
    );
    assert_eq!(
        read.get("approximateDollarLimit").and_then(Value::as_f64),
        Some(250.5),
        "expected the dollar limit that was created, got {read}"
    );
    assert_eq!(
        read.get("usageTrackingResource")
            .and_then(|r| r.get("queueId"))
            .and_then(Value::as_str),
        Some(queue_id.as_str()),
        "expected the tracked queue that was created, got {read}"
    );
    assert_eq!(
        read.get("actions").and_then(Value::as_array).map(Vec::len),
        Some(2),
        "expected both budget actions, got {read}"
    );
    assert_eq!(
        read.get("usages")
            .and_then(|u| u.get("approximateDollarUsage"))
            .and_then(Value::as_f64),
        Some(0.0),
        "expected a new budget to have consumed nothing, got {read}"
    );
    assert!(
        read.get("updatedAt").is_none() && read.get("updatedBy").is_none(),
        "expected no updatedAt/updatedBy before an update, got {read}"
    );

    let (status, listed) = emu.get(&budgets_path(&farm_id)).await;
    assert_eq!(status, 200, "expected ListBudgets to answer 200, got {status} with {listed}");
    let summaries = listed
        .get("budgets")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a budgets array in {listed}"));
    assert_eq!(summaries.len(), 1, "expected exactly the one budget, got {listed}");
    assert_eq!(
        summaries[0].get("budgetId").and_then(Value::as_str),
        Some(budget_id.as_str()),
        "expected the created budget in the list, got {listed}"
    );
    assert!(
        summaries[0].get("createdAt").and_then(Value::as_str).is_some(),
        "expected a summary's createdAt as an RFC 3339 string, got {listed}"
    );
}

/// The budget-action list of `count` identical, legal actions.
fn actions(count: usize) -> Value {
    Value::Array((0..count).map(|_| one_action()).collect())
}

/// One budget action at `threshold`, legal in every other way.
fn action_at(threshold: f64) -> Value {
    json!([action(threshold)])
}

/// Rule 2 — a modeled numeric or element-count bound violated anywhere in a budget
/// request body is refused with the body validator's bracketed finding list, and the
/// refused request leaves no budget behind.
///
/// The rows span the class the rule generalises over: which member is at fault
/// (a list's element count, a top-level float, a float nested in a list element), which
/// side of the bound was crossed, and how far. The modeled bounds come from
/// `smithy_query(query="operation", operation="CreateBudget")` — `actions`
/// `length {min 0, max 10}`, `approximateDollarLimit` `range {min 0.01}` — and
/// `smithy_query(query="shape", shape="BudgetActionToAdd")` — `thresholdPercentage`
/// `range {min 0, max 100}`.
#[tokio::test]
async fn a_budget_body_outside_a_modeled_bound_is_refused_and_stores_nothing() {
    // case, actions, approximateDollarLimit, expected message
    let cases: [(&str, Value, Value, &str); 6] = [
        (
            "one action past the maximum",
            actions(11),
            json!(10.0),
            "[array is too long: must have at most 10 elements but instance has 11 elements, \
             format attribute \"float\" not supported]",
        ),
        (
            "far past the action maximum",
            actions(25),
            json!(10.0),
            "[array is too long: must have at most 10 elements but instance has 25 elements, \
             format attribute \"float\" not supported]",
        ),
        (
            "dollar limit just below the minimum",
            json!([one_action()]),
            json!(0.001),
            "[numeric instance is lower than the required minimum (minimum: 0.01, found: 0.001), \
             format attribute \"float\" not supported]",
        ),
        (
            "dollar limit of zero",
            json!([one_action()]),
            json!(0),
            "[numeric instance is lower than the required minimum (minimum: 0.01, found: 0), \
             format attribute \"float\" not supported]",
        ),
        (
            "threshold past the maximum",
            action_at(150.0),
            json!(10.0),
            "[numeric instance is greater than the required maximum (maximum: 100, found: 150), \
             format attribute \"float\" not supported]",
        ),
        (
            "threshold below the minimum",
            action_at(-1.0),
            json!(10.0),
            "[numeric instance is lower than the required minimum (minimum: 0, found: -1), \
             format attribute \"float\" not supported]",
        ),
    ];

    let emu = Emulator::new();
    let (farm_id, queue_id) = farm_with_queue(&emu, "bounds").await;

    for (case, actions, limit, expected) in cases {
        let (status, body) = emu
            .post(
                &budgets_path(&farm_id),
                create_body(
                    case,
                    &queue_id,
                    fixed_schedule("2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z"),
                    actions,
                    limit,
                ),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateBudget to answer 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected code ValidationException, got {body}"
        );
        assert_eq!(
            message_of(&body),
            expected,
            "case {case:?}: expected message {expected:?}, got {:?}",
            message_of(&body)
        );

        let (status, listed) = emu.get(&budgets_path(&farm_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListBudgets to answer 200, got {status} with {listed}"
        );
        let named = listed
            .get("budgets")
            .and_then(Value::as_array)
            .map(|budgets| {
                budgets
                    .iter()
                    .any(|b| b.get("displayName").and_then(Value::as_str) == Some(case))
            })
            .unwrap_or(false);
        assert!(
            !named,
            "case {case:?}: a refused CreateBudget must leave no budget behind, got {listed}"
        );
    }
}

/// Rule 2's other side — a request exactly AT a modeled bound is accepted.
///
/// The float rows are what makes this more than a mirror: a `float` bound compared after
/// widening the value to a wider type puts the modeled minimum itself just below the
/// bound, so `0.01` — the value the model names — would be refused.
#[tokio::test]
async fn a_budget_body_at_a_modeled_bound_is_accepted() {
    // case, actions, approximateDollarLimit
    let cases: [(&str, Value, Value); 5] = [
        ("actions at the maximum", actions(10), json!(10.0)),
        ("no actions at all", actions(0), json!(10.0)),
        ("dollar limit at the minimum", json!([one_action()]), json!(0.01)),
        ("threshold at the minimum", action_at(0.0), json!(10.0)),
        ("threshold at the maximum", action_at(100.0), json!(10.0)),
    ];

    let emu = Emulator::new();
    let (farm_id, queue_id) = farm_with_queue(&emu, "at-bounds").await;

    for (case, actions, limit) in cases {
        let (status, body) = emu
            .post(
                &budgets_path(&farm_id),
                create_body(
                    case,
                    &queue_id,
                    fixed_schedule("2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z"),
                    actions,
                    limit,
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBudget to answer 200, got {status} with {body}"
        );
        let budget_id = id_of(&body, "budgetId");
        let (status, read) = emu.get(&budget_path(&farm_id, &budget_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetBudget to answer 200, got {status} with {read}"
        );
    }
}

/// Rule 2 applies to the update path too: the same bounds, the same wording.
#[tokio::test]
async fn an_update_outside_a_modeled_bound_is_refused_and_changes_nothing() {
    let emu = Emulator::new();
    let (farm_id, queue_id) = farm_with_queue(&emu, "update-bounds").await;
    let (_, created) = emu
        .post(
            &budgets_path(&farm_id),
            create_body(
                "before",
                &queue_id,
                fixed_schedule("2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z"),
                json!([one_action()]),
                json!(10.0),
            ),
        )
        .await;
    let budget_id = id_of(&created, "budgetId");
    let path = budget_path(&farm_id, &budget_id);

    // case, update body, expected message
    let cases: [(&str, Value, &str); 3] = [
        (
            "dollar limit below the minimum",
            json!({"displayName": "after", "approximateDollarLimit": 0.005}),
            "[numeric instance is lower than the required minimum (minimum: 0.01, found: 0.005), \
             format attribute \"float\" not supported]",
        ),
        (
            "too many actions to add",
            json!({"displayName": "after", "actionsToAdd": actions(11)}),
            "[array is too long: must have at most 10 elements but instance has 11 elements, \
             format attribute \"float\" not supported]",
        ),
        (
            "too many actions to remove",
            json!({"displayName": "after", "actionsToRemove": actions(12)}),
            "[array is too long: must have at most 10 elements but instance has 12 elements, \
             format attribute \"float\" not supported]",
        ),
    ];

    for (case, body, expected) in cases {
        let (status, refused) = emu.call("PATCH", &path, Some(body), None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected UpdateBudget to answer 400, got {status} with {refused}"
        );
        assert_eq!(
            message_of(&refused),
            expected,
            "case {case:?}: expected message {expected:?}, got {:?}",
            message_of(&refused)
        );

        let (_, read) = emu.get(&path).await;
        assert_eq!(
            string_at(&read, "displayName"),
            "before",
            "case {case:?}: a refused UpdateBudget must change nothing, got {read}"
        );
    }
}

/// Rule 2's third side — the model bounds the action list a budget HOLDS
/// (`ResponseBudgetActionList` `length {max 10}`), not only each request's list, so no
/// sequence of updates can grow one budget's actions without limit and the refused update
/// changes nothing.
#[tokio::test]
async fn repeated_updates_cannot_grow_a_budget_past_its_action_maximum() {
    let emu = Emulator::new();
    let (farm_id, queue_id) = farm_with_queue(&emu, "action-cap").await;
    let (_, created) = emu
        .post(
            &budgets_path(&farm_id),
            create_body(
                "capped",
                &queue_id,
                fixed_schedule("2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z"),
                json!([action(10.0), action(20.0), action(30.0), action(40.0)]),
                json!(10.0),
            ),
        )
        .await;
    let budget_id = id_of(&created, "budgetId");
    let path = budget_path(&farm_id, &budget_id);

    /// How many actions the budget holds, read back over `GetBudget`.
    async fn held(emu: &Emulator, path: &str) -> usize {
        let (_, read) = emu.get(path).await;
        read.get("actions")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_else(|| panic!("expected an actions array in {read}"))
    }

    // Six more take it exactly to the modeled maximum.
    let (status, body) = emu
        .call(
            "PATCH",
            &path,
            Some(json!({"actionsToAdd": [
                action(50.0), action(60.0), action(70.0), action(75.0), action(85.0), action(95.0),
            ]})),
            None,
        )
        .await;
    assert_eq!(status, 200, "expected the sixth-to-tenth actions to be accepted, got {status} with {body}");
    assert_eq!(held(&emu, &path).await, 10, "expected the budget to hold 10 actions");

    // One more would pass it, so the whole request — including the rename — is refused.
    let (status, body) = emu
        .call(
            "PATCH",
            &path,
            Some(json!({"displayName": "grown", "actionsToAdd": action_at(45.0)})),
            None,
        )
        .await;
    assert_eq!(status, 400, "expected the eleventh action to be refused, got {status} with {body}");
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "expected code ValidationException, got {body}"
    );
    assert_eq!(
        message_of(&body),
        "[array is too long: must have at most 10 elements but instance has 11 elements, \
         format attribute \"float\" not supported]",
        "expected the stored-action-maximum refusal, got {:?}",
        message_of(&body)
    );
    assert_eq!(
        held(&emu, &path).await,
        10,
        "a refused UpdateBudget must leave the stored actions alone"
    );
    let (_, read) = emu.get(&path).await;
    assert_eq!(
        string_at(&read, "displayName"),
        "capped",
        "a refused UpdateBudget must change nothing else either, got {read}"
    );

    // At the maximum, a removal makes room for an addition in the same request.
    let (status, body) = emu
        .call(
            "PATCH",
            &path,
            Some(json!({"actionsToRemove": [action(10.0)], "actionsToAdd": action_at(45.0)})),
            None,
        )
        .await;
    assert_eq!(status, 200, "expected a replacement at the maximum to be accepted, got {status} with {body}");
    let (_, read) = emu.get(&path).await;
    let thresholds: Vec<f64> = read
        .get("actions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an actions array in {read}"))
        .iter()
        .filter_map(|a| a.get("thresholdPercentage").and_then(Value::as_f64))
        .collect();
    assert_eq!(thresholds.len(), 10, "expected the budget to still hold 10 actions, got {read}");
    assert!(
        thresholds.contains(&45.0) && !thresholds.contains(&10.0),
        "expected the removed action gone and the added one present, got {thresholds:?}"
    );
}

/// Rule 3 — `ACTIVE -> INACTIVE` happens once and never reverses.
///
/// The rows are every ordered pair of the two modeled `BudgetStatus` values
/// (`smithy_query(query="shape", shape="BudgetStatus")`), so the one refused transition
/// is asserted beside the three that are allowed.
#[tokio::test]
async fn the_budget_status_transition_is_one_way() {
    // case, status the budget is moved to first (None to leave it as created),
    // status the update then asks for, expected status afterwards, expected HTTP status
    let cases: [(&str, Option<&str>, &str, &str, u16); 4] = [
        ("active stays active", None, "ACTIVE", "ACTIVE", 200),
        ("active deactivates", None, "INACTIVE", "INACTIVE", 200),
        (
            "inactive stays inactive",
            Some("INACTIVE"),
            "INACTIVE",
            "INACTIVE",
            200,
        ),
        (
            "inactive cannot be reactivated",
            Some("INACTIVE"),
            "ACTIVE",
            "INACTIVE",
            400,
        ),
    ];

    let emu = Emulator::new();
    let (farm_id, queue_id) = farm_with_queue(&emu, "status").await;

    for (case, prepare, asked, expected_status, expected_http) in cases {
        let (_, created) = emu
            .post(
                &budgets_path(&farm_id),
                create_body(
                    case,
                    &queue_id,
                    fixed_schedule("2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z"),
                    json!([one_action()]),
                    json!(10.0),
                ),
            )
            .await;
        let budget_id = id_of(&created, "budgetId");
        let path = budget_path(&farm_id, &budget_id);

        let (_, read) = emu.get(&path).await;
        assert_eq!(
            string_at(&read, "status"),
            "ACTIVE",
            "case {case:?}: expected a new budget to be ACTIVE, got {read}"
        );

        if let Some(status) = prepare {
            let (code, body) = emu
                .call("PATCH", &path, Some(json!({"status": status})), None)
                .await;
            assert_eq!(
                code, 200,
                "case {case:?}: expected the move to {status} to answer 200, got {code} with {body}"
            );
        }

        // The update asks for `asked` and renames the budget in the same request, so a
        // refusal is observable as "nothing changed", not only as "the status held".
        let (code, body) = emu
            .call(
                "PATCH",
                &path,
                Some(json!({"status": asked, "displayName": "renamed"})),
                None,
            )
            .await;
        assert_eq!(
            code, expected_http,
            "case {case:?}: expected UpdateBudget to answer {expected_http}, got {code} with {body}"
        );
        if expected_http == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected code ValidationException, got {body}"
            );
            assert_eq!(
                message_of(&body),
                "Reactivating of INACTIVE budget is not allowed",
                "case {case:?}: expected the reactivation refusal, got {:?}",
                message_of(&body)
            );
        }

        let (_, read) = emu.get(&path).await;
        assert_eq!(
            string_at(&read, "status"),
            expected_status,
            "case {case:?}: expected status {expected_status:?} afterwards, got {read}"
        );
        let expected_name = if expected_http == 400 { case } else { "renamed" };
        assert_eq!(
            string_at(&read, "displayName"),
            expected_name,
            "case {case:?}: expected displayName {expected_name:?} afterwards, got {read}"
        );
    }
}

/// `ListBudgets`' `status` filter reports only the budgets in that status, and the farm's
/// budgets are reachable through no other farm.
#[tokio::test]
async fn budgets_are_listed_per_farm_and_filtered_by_status() {
    let emu = Emulator::new();
    let (farm_id, queue_id) = farm_with_queue(&emu, "listing").await;
    let (other_farm, _) = farm_with_queue(&emu, "other").await;

    let mut ids = Vec::new();
    for name in ["alpha", "beta", "gamma"] {
        let (_, created) = emu
            .post(
                &budgets_path(&farm_id),
                create_body(
                    name,
                    &queue_id,
                    fixed_schedule("2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z"),
                    json!([one_action()]),
                    json!(10.0),
                ),
            )
            .await;
        ids.push(id_of(&created, "budgetId"));
    }
    let (code, _) = emu
        .call(
            "PATCH",
            &budget_path(&farm_id, &ids[1]),
            Some(json!({"status": "INACTIVE"})),
            None,
        )
        .await;
    assert_eq!(code, 200, "expected the move to INACTIVE to answer 200, got {code}");

    // case, query, how many budgets the page holds
    let cases: [(&str, &str, usize); 3] = [
        ("every budget", "", 3),
        ("only the active ones", "?status=ACTIVE", 2),
        ("only the inactive ones", "?status=INACTIVE", 1),
    ];
    for (case, query, expected) in cases {
        let (status, listed) = emu
            .get(&format!("{}{query}", budgets_path(&farm_id)))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListBudgets to answer 200, got {status} with {listed}"
        );
        let count = listed
            .get("budgets")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        assert_eq!(
            count, expected,
            "case {case:?}: expected {expected} budgets, got {listed}"
        );
    }

    let (status, listed) = emu.get(&budgets_path(&other_farm)).await;
    assert_eq!(status, 200, "expected ListBudgets to answer 200, got {status} with {listed}");
    assert_eq!(
        listed.get("budgets").and_then(Value::as_array).map(Vec::len),
        Some(0),
        "expected another farm to hold none of these budgets, got {listed}"
    );
}

/// A budget is reached only through its own farm: every level of the path that names
/// something the service does not hold is a 404.
#[tokio::test]
async fn a_budget_is_reachable_only_through_its_own_farm() {
    let emu = Emulator::new();
    let (farm_id, queue_id) = farm_with_queue(&emu, "scoping").await;
    let (other_farm, _) = farm_with_queue(&emu, "neighbour").await;
    let (_, created) = emu
        .post(
            &budgets_path(&farm_id),
            create_body(
                "scoped",
                &queue_id,
                fixed_schedule("2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z"),
                json!([one_action()]),
                json!(10.0),
            ),
        )
        .await;
    let budget_id = id_of(&created, "budgetId");

    // A well-formed id this test never created: 32 hex characters, so the request passes
    // the boundary validator and the handler is what decides the outcome.
    let absent_budget = format!("budget-{}", "0".repeat(32));
    let absent_farm = format!("farm-{}", "1".repeat(32));

    // case, path
    let cases: [(&str, String); 3] = [
        (
            "a budget of another farm",
            budget_path(&other_farm, &budget_id),
        ),
        (
            "a budget nobody created",
            budget_path(&farm_id, &absent_budget),
        ),
        ("a farm nobody created", budget_path(&absent_farm, &budget_id)),
    ];
    for (case, path) in cases {
        let (status, body) = emu.get(&path).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected GetBudget to answer 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected code ResourceNotFoundException, got {body}"
        );
    }

    // The queue a budget tracks must be one of its own farm's queues, for the same reason.
    let (status, body) = emu
        .post(
            &budgets_path(&other_farm),
            create_body(
                "cross-farm queue",
                &queue_id,
                fixed_schedule("2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z"),
                json!([one_action()]),
                json!(10.0),
            ),
        )
        .await;
    assert_eq!(
        status, 404,
        "expected CreateBudget tracking another farm's queue to answer 404, got {status} with {body}"
    );
}

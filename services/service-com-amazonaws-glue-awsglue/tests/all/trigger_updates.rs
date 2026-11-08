// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateTrigger`, and the `GetTrigger` that reads back what it
//! wrote.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: `UpdateTrigger` replaces the stored trigger's DEFINITION with the
//! one the request's `TriggerUpdate` carries — "this object updates the previous trigger
//! definition by overwriting it completely" — so each of the five definition members the
//! shape declares takes the update's value and a member the update OMITS is left with no
//! value at all, while the members `TriggerUpdate` does not carry (the trigger's name, its
//! type, its state and the workflow it belongs to) stay where the create put them. The
//! answer is the resulting definition, which is also what a later read reports. A
//! replacement the service could not fire is refused without touching what is stored: the
//! trigger would initiate nothing, it would lose the member its type requires, its
//! predicate would not say whether all or any of several conditions must hold, its schedule
//! is no cron expression, or an action or condition names a job or crawler this account
//! does not hold.
//!
//! The suites are tables: each row is one input along the axis the rule generalises over
//! (the trigger's type, which definition members the update carries, how many actions and
//! conditions it carries, whether a member is replaced or dropped, the form of the schedule
//! expression), with each of the four modelled `TriggerType` values represented.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::TRIGGER_TYPE_VALUES;
use service_com_amazonaws_glue_awsglue::state::{
    messages, TRIGGER_STATE_ACTIVATED, TRIGGER_STATE_CREATED, TRIGGER_TYPE_ON_DEMAND,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_messages::assert_refusal;
use crate::wire::{call, call_in_region, emulator, error_code, error_message, ok};

/// The job a trigger's actions and conditions name. A trigger's action names a job of the
/// Region the call is served in, so each suite registers it first.
const JOB: &str = "trigger-update-job";

/// A second job, so an update can move an action onto another target.
const OTHER_JOB: &str = "trigger-update-other-job";

/// The crawler an action or a condition can name instead of a job.
const CRAWLER: &str = "trigger-update-crawler";

/// The workflow every trigger here belongs to: `TriggerUpdate` carries no `WorkflowName`,
/// so the membership is what an update must leave standing.
const WORKFLOW: &str = "trigger-update-workflow";

/// How a trigger write refuses a definition that names no action, as AWS words it.
/// RECORDED sentence, written out here rather than read from the service's own constant, so
/// a change to that wording is caught by this suite instead of following it.
const NO_ACTIONS_MESSAGE: &str = "Actions cannot be null or empty";

/// The five members `TriggerUpdate` declares as the definition it overwrites. Source: the
/// members of `com.amazonaws.glue#TriggerUpdate` (`smithy_query shape=TriggerUpdate`),
/// minus its `Name`, which the model states is "Reserved for future use".
const OVERWRITTEN_MEMBERS: [&str; 5] = [
    "Description",
    "Schedule",
    "Actions",
    "Predicate",
    "EventBatchingCondition",
];

/// Register the workflow, the jobs and the crawler the rows below name, so the targets a
/// create and an update refer to resolve.
async fn register_targets(router: &MultiServiceRouter) {
    ok(router, "CreateWorkflow", &json!({ "Name": WORKFLOW })).await;
    for job in [JOB, OTHER_JOB] {
        ok(
            router,
            "CreateJob",
            &json!({
                "Name": job,
                "Role": "arn:aws:iam::000000000000:role/sim2real-glue-job-role",
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
            }),
        )
        .await;
    }
    ok(
        router,
        "CreateCrawler",
        &json!({
            "Name": CRAWLER,
            "Role": "arn:aws:iam::000000000000:role/sim2real-crawler-role",
            "Targets": { "S3Targets": [{ "Path": "s3://sim2real-glue-crawler/data/" }] },
        }),
    )
    .await;
}

/// The trigger each row creates BEFORE updating it: every definition member its `Type`
/// allows, so a member the update drops is observably taken off rather than merely absent
/// from both records.
fn trigger_before_update(name: &str, trigger_type: &str, start_on_creation: bool) -> Value {
    let mut body = json!({
        "Name": name,
        "Type": trigger_type,
        "Description": "the trigger before the update",
        "WorkflowName": WORKFLOW,
        "Tags": { "owner": "data-platform" },
        "Actions": [
            {
                "JobName": JOB,
                "Arguments": { "--before": "true" },
                "Timeout": 60,
                "NotificationProperty": { "NotifyDelayAfter": 10 },
            },
            { "CrawlerName": CRAWLER },
        ],
    });
    match trigger_type {
        // "This field is required when the trigger type is SCHEDULED."
        "SCHEDULED" => body["Schedule"] = json!("cron(15 12 * * ? *)"),
        // "This field is required when the trigger type is CONDITIONAL." The predicate
        // waits on the job no action of this definition runs: a conditional trigger may not
        // name one job both in its `Predicate` and in its `Actions`.
        "CONDITIONAL" => {
            body["Predicate"] = json!({
                "Logical": "AND",
                "Conditions": [
                    { "LogicalOperator": "EQUALS", "JobName": OTHER_JOB, "State": "SUCCEEDED" },
                    {
                        "LogicalOperator": "EQUALS",
                        "CrawlerName": CRAWLER,
                        "CrawlState": "SUCCEEDED",
                    },
                ],
            });
        }
        // The batching condition "that must be met ... before EventBridge event trigger
        // fires", which only an event trigger waits on.
        "EVENT" => body["EventBatchingCondition"] = json!({ "BatchSize": 5, "BatchWindow": 120 }),
        _ => {}
    }
    // "Set to true to start SCHEDULED and CONDITIONAL triggers when created. True is not
    // supported for ON_DEMAND triggers."
    if start_on_creation {
        body["StartOnCreation"] = json!(true);
    }
    body
}

/// The `Trigger` structure `GetTrigger` reports for `name`.
async fn get_trigger(router: &MultiServiceRouter, name: &str) -> Value {
    let body = ok(router, "GetTrigger", &json!({ "Name": name })).await;
    body.get("Trigger")
        .cloned()
        .unwrap_or_else(|| panic!("GetTrigger must report a Trigger, got {body}"))
}

/// Create the fully-specified trigger of [`trigger_before_update`] and report what
/// `GetTrigger` reads back for it.
async fn create_trigger(
    router: &MultiServiceRouter,
    name: &str,
    trigger_type: &str,
    start_on_creation: bool,
) -> Value {
    ok(
        router,
        "CreateTrigger",
        &trigger_before_update(name, trigger_type, start_on_creation),
    )
    .await;
    get_trigger(router, name).await
}

/// The `Trigger` an overwrite of `before`'s definition by `update` must produce.
///
/// This is the rule, applied member by member rather than row by row: each definition
/// member takes the update's value, a member the update omits is left with none, and every
/// other member of the trigger — its name, type, state and workflow — is `before`'s.
fn overwritten(before: &Value, update: &Value) -> Value {
    let mut expected = before.clone();
    let members = expected.as_object_mut().expect("a Trigger is a structure");
    for member in OVERWRITTEN_MEMBERS {
        match update.get(member) {
            Some(value) => {
                members.insert(member.to_string(), value.clone());
            }
            None => {
                members.remove(member);
            }
        }
    }
    expected
}

/// Send one `UpdateTrigger` that must succeed and report the `Trigger` it answers with.
async fn update_trigger(router: &MultiServiceRouter, name: &str, update: &Value) -> Value {
    let body = ok(
        router,
        "UpdateTrigger",
        &json!({ "Name": name, "TriggerUpdate": update }),
    )
    .await;
    body.get("Trigger")
        .cloned()
        .unwrap_or_else(|| panic!("UpdateTrigger must answer the resulting Trigger, got {body}"))
}

// ---------------------------------------------------------------------------
// The overwrite
// ---------------------------------------------------------------------------

/// An update replaces every definition member with the one it carries, takes off the ones
/// it omits, and leaves the trigger's name, type, state and workflow alone — whatever
/// subset of the definition the request carries. The answer is the resulting definition,
/// and a later read reports the same one.
///
/// The rows differ along the trigger's type (each of the four modelled `TriggerType`
/// values), which definition members the update carries, how many actions it names, how
/// many conditions its predicate lists, and whether a member is replaced or dropped.
#[tokio::test]
async fn an_update_overwrites_the_definition_it_carries() {
    let router = emulator();
    register_targets(&router).await;

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "on-demand, one action and nothing else",
            "ON_DEMAND",
            json!({ "Actions": [{ "JobName": OTHER_JOB }] }),
        ),
        (
            "on-demand, a new description and three actions",
            "ON_DEMAND",
            json!({
                "Description": "the trigger after the update",
                "Actions": [
                    { "JobName": JOB },
                    { "JobName": OTHER_JOB, "Timeout": 120 },
                    { "CrawlerName": CRAWLER },
                ],
            }),
        ),
        (
            "on-demand, the reserved Name renames nothing",
            "ON_DEMAND",
            json!({
                "Name": "renamed-by-the-update",
                "Actions": [{ "JobName": JOB }],
            }),
        ),
        (
            "scheduled, the schedule moved",
            "SCHEDULED",
            json!({
                "Schedule": "cron(0 1 * * ? *)",
                "Actions": [{ "JobName": JOB }],
            }),
        ),
        (
            "scheduled, a day-of-week schedule and new job arguments",
            "SCHEDULED",
            json!({
                "Schedule": "cron(30 2 ? * MON-FRI *)",
                "Description": "weekdays only",
                "Actions": [{
                    "JobName": OTHER_JOB,
                    "Arguments": { "--after": "true", "--enable-metrics": "" },
                    "Timeout": 2880,
                    "NotificationProperty": { "NotifyDelayAfter": 1 },
                }],
            }),
        ),
        (
            // The predicate the update carries is the one the trigger then fires on, and
            // "an optional field if only one condition is listed" — so a single-condition
            // predicate needs no `Logical`.
            "conditional, one condition and no logical",
            "CONDITIONAL",
            json!({
                "Predicate": {
                    "Conditions": [
                        { "LogicalOperator": "EQUALS", "JobName": OTHER_JOB, "State": "FAILED" },
                    ],
                },
                "Actions": [{ "JobName": JOB }],
            }),
        ),
        (
            "conditional, two conditions held by ANY",
            "CONDITIONAL",
            json!({
                "Predicate": {
                    "Logical": "ANY",
                    "Conditions": [
                        { "LogicalOperator": "EQUALS", "JobName": JOB, "State": "TIMEOUT" },
                        {
                            "LogicalOperator": "EQUALS",
                            "CrawlerName": CRAWLER,
                            "CrawlState": "FAILED",
                        },
                    ],
                },
                "Actions": [{ "CrawlerName": CRAWLER }],
            }),
        ),
        (
            // The published bounds of `EventBatchingCondition`: `BatchSize` 1..100 and
            // `BatchWindow` 1..900 (`smithy_query shape=EventBatchingCondition`).
            "event, the batching condition at its published ceiling",
            "EVENT",
            json!({
                "EventBatchingCondition": { "BatchSize": 100, "BatchWindow": 900 },
                "Actions": [{ "JobName": JOB }],
            }),
        ),
        (
            "event, the batching condition at its published floor",
            "EVENT",
            json!({
                "EventBatchingCondition": { "BatchSize": 1, "BatchWindow": 1 },
                "Actions": [{ "JobName": JOB }],
            }),
        ),
        (
            "event, the batching condition dropped",
            "EVENT",
            json!({ "Actions": [{ "JobName": JOB }] }),
        ),
    ];

    for (index, (case, trigger_type, update)) in cases.iter().enumerate() {
        let name = format!("overwrite-{index}");
        let before = create_trigger(&router, &name, trigger_type, false).await;

        let answered = update_trigger(&router, &name, update).await;
        let expected = overwritten(&before, update);
        assert_eq!(
            answered, expected,
            "case {case}: expected the update to answer {expected}, got {answered}"
        );

        // What was answered is what is stored: the read reports the same definition.
        let after = get_trigger(&router, &name).await;
        assert_eq!(
            after, expected,
            "case {case}: expected the stored trigger to be {expected}, got {after}"
        );
        // The members `TriggerUpdate` carries no value for are the create's.
        for member in ["Name", "Type", "State", "WorkflowName"] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {case}: an update must not move {member}, got {after}"
            );
        }
    }

    // "Reserved for future use": the update's own `Name` named no trigger, so none was
    // created or renamed to it.
    let (status, body) = call(
        &router,
        "GetTrigger",
        &json!({ "Name": "renamed-by-the-update" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case the reserved Name: no trigger may exist under it, got {status} with {body}"
    );

    // Every modelled trigger type is covered by the table above.
    for trigger_type in TRIGGER_TYPE_VALUES {
        assert!(
            cases.iter().any(|(_, case_type, _)| case_type == trigger_type),
            "case coverage: no row updates a {trigger_type} trigger"
        );
    }
}

/// Repeated updates each overwrite the last: the definition a trigger holds is the one the
/// most recent update carried, not the union of them all.
#[tokio::test]
async fn each_update_overwrites_the_one_before_it() {
    let router = emulator();
    register_targets(&router).await;
    create_trigger(&router, "overwritten-again", "ON_DEMAND", false).await;

    let rounds = [
        json!({
            "Description": "round one",
            "Actions": [{ "JobName": JOB }],
        }),
        json!({
            "Actions": [{ "JobName": OTHER_JOB }, { "CrawlerName": CRAWLER }],
        }),
        json!({
            "Description": "round three",
            "Actions": [{ "CrawlerName": CRAWLER }],
        }),
    ];

    for (round, update) in rounds.iter().enumerate() {
        let answered = update_trigger(&router, "overwritten-again", update).await;
        let after = get_trigger(&router, "overwritten-again").await;
        assert_eq!(
            answered, after,
            "case round {round}: the answer must be what is stored, expected {after}, got \
             {answered}"
        );
        for member in OVERWRITTEN_MEMBERS {
            assert_eq!(
                after.get(member),
                update.get(member),
                "case round {round}: expected {member} to be the round's own value, got {after}"
            );
        }
    }
}

/// An update does not move the trigger's `State`: the state is the create's, or the one a
/// start left behind, and overwriting the definition is not a start or a stop.
///
/// The rows span each modelled trigger type, both states a create can leave a trigger in,
/// and a trigger started after the create.
#[tokio::test]
async fn an_update_does_not_move_the_triggers_state() {
    let router = emulator();
    register_targets(&router).await;

    for trigger_type in TRIGGER_TYPE_VALUES {
        // "True is not supported for ON_DEMAND triggers", so that one type is created idle
        // and then started by its own call instead.
        let started_on_creation = *trigger_type != TRIGGER_TYPE_ON_DEMAND;
        let name = format!("state-{trigger_type}");
        let before =
            create_trigger(&router, &name, trigger_type, started_on_creation).await;
        let expected_state = if started_on_creation {
            TRIGGER_STATE_ACTIVATED
        } else {
            TRIGGER_STATE_CREATED
        };
        assert_eq!(
            before.get("State").and_then(Value::as_str),
            Some(expected_state),
            "case {trigger_type} before the update: expected State {expected_state}, got {before}"
        );

        let mut update = json!({ "Actions": [{ "JobName": OTHER_JOB }] });
        match *trigger_type {
            "SCHEDULED" => update["Schedule"] = json!("cron(0 3 * * ? *)"),
            "CONDITIONAL" => {
                update["Predicate"] = json!({
                    "Conditions": [
                        { "LogicalOperator": "EQUALS", "JobName": JOB, "State": "SUCCEEDED" },
                    ],
                });
            }
            _ => {}
        }
        let answered = update_trigger(&router, &name, &update).await;
        assert_eq!(
            answered.get("State").and_then(Value::as_str),
            Some(expected_state),
            "case {trigger_type}: expected the update to leave State {expected_state}, got \
             {answered}"
        );
    }

    // A trigger started by its own call keeps the state that start left it in: an
    // on-demand trigger has no armed state, so its start leaves it in the state it waits
    // in, and the update must not move that either.
    let name = format!("state-started-{TRIGGER_TYPE_ON_DEMAND}");
    create_trigger(&router, &name, TRIGGER_TYPE_ON_DEMAND, false).await;
    ok(&router, "StartTrigger", &json!({ "Name": name })).await;
    let answered = update_trigger(
        &router,
        &name,
        &json!({ "Actions": [{ "JobName": JOB }] }),
    )
    .await;
    assert_eq!(
        answered.get("State").and_then(Value::as_str),
        Some(TRIGGER_STATE_CREATED),
        "case a started on-demand trigger: expected State {TRIGGER_STATE_CREATED}, got \
         {answered}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A `TriggerUpdate` that would leave behind a definition the service could not fire is
/// refused with the modelled input error, and the stored definition is untouched.
///
/// The rows span the rules the update carries over from the create: a definition that
/// initiates nothing (no `Actions` member at all, and an empty list), one that loses the
/// member its type requires, a predicate that lists several conditions without saying
/// whether all or any must hold, and a `Schedule` that is not a `cron(..)` expression of
/// six fields.
#[tokio::test]
async fn a_refused_update_leaves_the_stored_definition_unchanged() {
    let router = emulator();
    register_targets(&router).await;

    let actions = json!([{ "JobName": JOB }]);
    let one_condition = json!([
        { "LogicalOperator": "EQUALS", "JobName": JOB, "State": "SUCCEEDED" },
    ]);
    let two_conditions = json!([
        { "LogicalOperator": "EQUALS", "JobName": JOB, "State": "SUCCEEDED" },
        { "LogicalOperator": "EQUALS", "JobName": OTHER_JOB, "State": "FAILED" },
    ]);

    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "no Actions member at all",
            "ON_DEMAND",
            json!({ "Description": "this update must not land" }),
            NO_ACTIONS_MESSAGE.to_string(),
        ),
        (
            "an empty Actions list",
            "ON_DEMAND",
            json!({ "Actions": [] }),
            NO_ACTIONS_MESSAGE.to_string(),
        ),
        (
            "a scheduled trigger left with no action",
            "SCHEDULED",
            json!({ "Schedule": "cron(0 1 * * ? *)", "Actions": [] }),
            NO_ACTIONS_MESSAGE.to_string(),
        ),
        (
            "a scheduled trigger left with no schedule",
            "SCHEDULED",
            json!({ "Actions": actions }),
            messages::trigger_member_required("Schedule"),
        ),
        (
            "a conditional trigger left with no predicate",
            "CONDITIONAL",
            json!({ "Actions": actions }),
            messages::trigger_member_required("Predicate"),
        ),
        (
            "a conditional trigger left with no condition",
            "CONDITIONAL",
            json!({ "Predicate": { "Logical": "AND", "Conditions": [] }, "Actions": actions }),
            messages::trigger_member_required("Predicate"),
        ),
        (
            "two conditions and no logical",
            "CONDITIONAL",
            json!({ "Predicate": { "Conditions": two_conditions }, "Actions": actions }),
            messages::predicate_logical_required(2),
        ),
        (
            "a bare cron body",
            "SCHEDULED",
            json!({ "Schedule": "15 12 * * ? *", "Actions": actions }),
            messages::schedule_not_a_cron_expression("15 12 * * ? *"),
        ),
        (
            "a rate expression",
            "SCHEDULED",
            json!({ "Schedule": "rate(5 minutes)", "Actions": actions }),
            messages::schedule_not_a_cron_expression("rate(5 minutes)"),
        ),
        (
            "too few cron fields",
            "SCHEDULED",
            json!({ "Schedule": "cron(15 12 * * ?)", "Actions": actions }),
            messages::schedule_not_a_cron_expression("cron(15 12 * * ?)"),
        ),
        (
            "too many cron fields",
            "SCHEDULED",
            json!({ "Schedule": "cron(15 12 * * ? * *)", "Actions": actions }),
            messages::schedule_not_a_cron_expression("cron(15 12 * * ? * *)"),
        ),
        (
            "an unclosed cron expression",
            "SCHEDULED",
            json!({ "Schedule": "cron(15 12 * * ? *", "Actions": actions }),
            messages::schedule_not_a_cron_expression("cron(15 12 * * ? *"),
        ),
        (
            "the empty schedule",
            "SCHEDULED",
            json!({ "Schedule": "", "Actions": actions }),
            messages::schedule_not_a_cron_expression(""),
        ),
        (
            // A conditional trigger is as refusable for its schedule as a scheduled one:
            // the rule is about the expression, not about the type.
            "a conditional trigger given a schedule that is no cron expression",
            "CONDITIONAL",
            json!({
                "Schedule": "every day",
                "Predicate": { "Conditions": one_condition },
                "Actions": actions,
            }),
            messages::schedule_not_a_cron_expression("every day"),
        ),
    ];

    for (index, (case, trigger_type, update, message)) in cases.iter().enumerate() {
        let name = format!("refused-{index}");
        let stored = create_trigger(&router, &name, trigger_type, false).await;

        assert_refusal(
            &router,
            case,
            "UpdateTrigger",
            &json!({ "Name": name, "TriggerUpdate": update }),
            "InvalidInputException",
            message,
        )
        .await;

        let after = get_trigger(&router, &name).await;
        assert_eq!(
            after, stored,
            "case {case}: the stored definition must be unchanged, expected {stored}, got {after}"
        );
    }
}

/// An update whose action or condition names a job or crawler this account does not hold is
/// the modelled not-found refusal, and the stored definition is untouched: the job miss is
/// the fixed sentence every job lookup answers with, and the crawler miss names the crawler
/// asked for.
#[tokio::test]
async fn an_update_naming_an_absent_target_is_the_modelled_miss() {
    let router = emulator();
    register_targets(&router).await;

    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "an action on a job nothing created",
            "ON_DEMAND",
            json!({ "Actions": [{ "JobName": "no-such-job" }] }),
            messages::JOB_NOT_FOUND.to_string(),
        ),
        (
            "one of two actions on a job nothing created",
            "ON_DEMAND",
            json!({ "Actions": [{ "JobName": JOB }, { "JobName": "another-absent-job" }] }),
            messages::JOB_NOT_FOUND.to_string(),
        ),
        (
            "an action on a crawler nothing created",
            "ON_DEMAND",
            json!({ "Actions": [{ "CrawlerName": "no-such-crawler" }] }),
            messages::crawler_not_found("no-such-crawler"),
        ),
        (
            "a condition on a job nothing created",
            "CONDITIONAL",
            json!({
                "Predicate": {
                    "Conditions": [{
                        "LogicalOperator": "EQUALS",
                        "JobName": "absent-condition-job",
                        "State": "SUCCEEDED",
                    }],
                },
                "Actions": [{ "JobName": JOB }],
            }),
            messages::JOB_NOT_FOUND.to_string(),
        ),
        (
            "a condition on a crawler nothing created",
            "CONDITIONAL",
            json!({
                "Predicate": {
                    "Conditions": [{
                        "LogicalOperator": "EQUALS",
                        "CrawlerName": "absent-condition-crawler",
                        "CrawlState": "SUCCEEDED",
                    }],
                },
                "Actions": [{ "JobName": JOB }],
            }),
            messages::crawler_not_found("absent-condition-crawler"),
        ),
    ];

    for (index, (case, trigger_type, update, message)) in cases.iter().enumerate() {
        let name = format!("absent-target-{index}");
        let stored = create_trigger(&router, &name, trigger_type, false).await;

        assert_refusal(
            &router,
            case,
            "UpdateTrigger",
            &json!({ "Name": name, "TriggerUpdate": update }),
            "EntityNotFoundException",
            message,
        )
        .await;

        let after = get_trigger(&router, &name).await;
        assert_eq!(
            after, stored,
            "case {case}: the stored definition must be unchanged, expected {stored}, got {after}"
        );
    }
}

/// Updating a trigger this endpoint does not hold is the modelled not-found refusal, and it
/// creates nothing: the rows span a name never created, names that differ from a stored
/// trigger's only in case — the model documents no case fold for a trigger name — and the
/// stored trigger's own name addressed through another Region's endpoint, because a trigger
/// is served only through the endpoint that created it.
#[tokio::test]
async fn an_update_of_an_absent_trigger_is_the_modelled_miss() {
    let router = emulator();
    register_targets(&router).await;
    create_trigger(&router, "present-trigger", "ON_DEMAND", false).await;

    let update = json!({ "Actions": [{ "JobName": JOB }] });

    for (case, region, name) in [
        ("never created", "us-east-1", "absent-trigger"),
        ("differs only in case", "us-east-1", "PRESENT-TRIGGER"),
        ("differs only in one letter's case", "us-east-1", "present-Trigger"),
        ("another Region's endpoint", "us-west-2", "present-trigger"),
        ("another Region's endpoint, far away", "eu-west-1", "present-trigger"),
    ] {
        let (status, body) = call_in_region(
            &router,
            region,
            "UpdateTrigger",
            &json!({ "Name": name, "TriggerUpdate": update }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            messages::TRIGGER_NOT_FOUND,
            "case {case}: expected the recorded trigger miss for {name}, got {body}"
        );

        // The refused update created nothing under that name in that Region.
        let (status, body) = call_in_region(
            &router,
            region,
            "GetTrigger",
            &json!({ "Name": name }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: a refused update must not create a trigger, got {status} with {body}"
        );
    }

    // The trigger the misses were aimed at is still the one the create stored.
    let stored = get_trigger(&router, "present-trigger").await;
    assert_eq!(
        stored.get("Description").and_then(Value::as_str),
        Some("the trigger before the update"),
        "case the stored trigger: a refused update must not touch it, got {stored}"
    );
}

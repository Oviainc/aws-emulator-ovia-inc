// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue trigger slice: `CreateTrigger` and the `GetTrigger`
//! that reads back what it stored.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: `CreateTrigger` persists the trigger definition under its name,
//! answers 200 with that name, and fills the one member the client does not set — `State`,
//! which is `ACTIVATED` when the request asked for the trigger to be started and `CREATED`
//! otherwise; `GetTrigger` then reports that stored definition unchanged. A definition the
//! service could not fire is refused instead of stored: a member the named `Type` requires
//! is missing, the `Schedule` is no cron expression, `StartOnCreation` asks to start a type
//! that cannot be started on creation, the trigger would initiate nothing, or an action or
//! condition names a job or crawler this account does not hold.
//!
//! The suites are tables: each row is one input along the axis the rule generalises over
//! (the trigger type, which optional members are sent, how many actions and conditions the
//! definition carries, the form of the schedule expression, whether the create starts the
//! trigger), with each of the four modelled `TriggerType` values represented.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::{CreateTriggerInput, TRIGGER_TYPE_VALUES};
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, TriggerEntry, MAX_TRIGGERS, TRIGGER_STATE_ACTIVATED,
    TRIGGER_STATE_CREATED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{assert_echoed, call, call_in_region, emulator, emulator_on, error_code, ok};

/// The job every trigger action in this suite runs. A trigger's action names a job of the
/// Region the create is served in, so each suite registers it first.
const JOB: &str = "trigger-target-job";

/// A second job, so a row can carry more than one action.
const OTHER_JOB: &str = "trigger-other-job";

/// The crawler an action or a condition can name instead of a job.
const CRAWLER: &str = "trigger-target-crawler";

/// The workflow a trigger can belong to.
const WORKFLOW: &str = "trigger-workflow";

/// Register the workflow, the jobs and the crawler the rows below name, so a create's
/// workflow, action and condition targets resolve.
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

/// A minimal `CreateTrigger` body of `trigger_type`, carrying one action on [`JOB`] plus
/// whatever that type requires.
fn trigger_body(name: &str, trigger_type: &str) -> Value {
    let mut body = json!({
        "Name": name,
        "Type": trigger_type,
        "Actions": [{ "JobName": JOB }],
    });
    match trigger_type {
        // "This field is required when the trigger type is SCHEDULED."
        "SCHEDULED" => body["Schedule"] = json!("cron(15 12 * * ? *)"),
        // "This field is required when the trigger type is CONDITIONAL."
        "CONDITIONAL" => {
            body["Predicate"] = json!({
                "Conditions": [{
                    "LogicalOperator": "EQUALS",
                    "JobName": JOB,
                    "State": "SUCCEEDED",
                }],
            });
        }
        _ => {}
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

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// What `CreateTrigger` persists is what `GetTrigger` reports: every member the request
/// carried round-trips unchanged, the create answers with the trigger's name, and `State`
/// is the one the create's `StartOnCreation` asked for.
///
/// The rows differ along the trigger type (each of the four modelled `TriggerType`
/// values), the member set sent (the bare required members, a description, a workflow
/// name, tags, an event batching condition), the number of actions, and the number of
/// predicate conditions.
#[tokio::test]
async fn created_triggers_are_read_back_from_the_store() {
    let router = emulator();
    register_targets(&router).await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "on-demand, required members only",
            json!({
                "Name": "on-demand-trigger",
                "Type": "ON_DEMAND",
                "Actions": [{ "JobName": JOB }],
            }),
        ),
        (
            "scheduled, every optional member",
            json!({
                "Name": "scheduled-trigger",
                "Type": "SCHEDULED",
                "Schedule": "cron(15 12 * * ? *)",
                "Description": "runs the curated load every day at 12:15 UTC",
                "WorkflowName": WORKFLOW,
                "StartOnCreation": true,
                "Tags": { "team": "data-platform", "env": "test" },
                "Actions": [{
                    "JobName": JOB,
                    "Arguments": { "--enable-metrics": "true" },
                    "Timeout": 60,
                    "SecurityConfiguration": "job-security",
                    "NotificationProperty": { "NotifyDelayAfter": 10 },
                }],
            }),
        ),
        (
            "conditional, two conditions and two actions",
            json!({
                "Name": "conditional-trigger",
                "Type": "CONDITIONAL",
                "Predicate": {
                    "Logical": "AND",
                    "Conditions": [
                        { "LogicalOperator": "EQUALS", "JobName": JOB, "State": "SUCCEEDED" },
                        {
                            "LogicalOperator": "EQUALS",
                            "CrawlerName": CRAWLER,
                            "CrawlState": "SUCCEEDED",
                        },
                    ],
                },
                "Actions": [{ "JobName": JOB }, { "JobName": OTHER_JOB }],
                "StartOnCreation": true,
            }),
        ),
        (
            "conditional, one condition and no logical",
            json!({
                "Name": "single-condition-trigger",
                "Type": "CONDITIONAL",
                "Predicate": {
                    "Conditions": [
                        { "LogicalOperator": "EQUALS", "JobName": OTHER_JOB, "State": "FAILED" },
                    ],
                },
                "Actions": [{ "CrawlerName": CRAWLER }],
            }),
        ),
        (
            "event, with a batching condition",
            json!({
                "Name": "event-trigger",
                "Type": "EVENT",
                "EventBatchingCondition": { "BatchSize": 5, "BatchWindow": 120 },
                "Actions": [{ "JobName": JOB }],
            }),
        ),
        (
            // `StartOnCreation` is refused only for ON_DEMAND ("True is not supported for
            // ON_DEMAND triggers"), so an EVENT trigger may be started on creation and is
            // then ACTIVATED like a started scheduled one.
            "event, started on creation",
            json!({
                "Name": "started-event-trigger",
                "Type": "EVENT",
                "EventBatchingCondition": { "BatchSize": 1 },
                "StartOnCreation": true,
                "Actions": [{ "JobName": JOB }],
            }),
        ),
    ];

    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case names a trigger");
        let created = ok(&router, "CreateTrigger", request).await;
        assert_eq!(
            created.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: the create must answer with the trigger's name, got {created}"
        );

        let trigger = get_trigger(&router, name).await;

        // Everything the request carried, except the members that are not part of the
        // `Trigger` shape (`Tags`, `StartOnCreation`).
        let mut echoed = request.clone();
        let members = echoed
            .as_object_mut()
            .expect("the request is a structure");
        members.remove("Tags");
        let started = members
            .remove("StartOnCreation")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        assert_echoed(case, &echoed, &trigger);

        // The one member the service sets: a started trigger is waiting to fire, one the
        // create did not start is registered and idle.
        let expected_state = if started {
            TRIGGER_STATE_ACTIVATED
        } else {
            TRIGGER_STATE_CREATED
        };
        assert_eq!(
            trigger.get("State").and_then(Value::as_str),
            Some(expected_state),
            "case {case}: expected State {expected_state}, got {trigger}"
        );
        // `Trigger$Id` is "Reserved for future use", so a trigger carries none.
        assert_eq!(
            trigger.get("Id"),
            None,
            "case {case}: a trigger reports no Id, got {trigger}"
        );
    }

    // Each create is its own record: every trigger of the table is still readable.
    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case names a trigger");
        let trigger = get_trigger(&router, name).await;
        assert_eq!(
            trigger.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: {name} must still be readable after the later creates"
        );
    }

    // Every modelled trigger type is covered by the table above, so the rows span the
    // member requirements each type turns on.
    for trigger_type in TRIGGER_TYPE_VALUES {
        assert!(
            cases.iter().any(|(_, request)| request["Type"] == json!(trigger_type)),
            "case coverage: no row creates a {trigger_type} trigger"
        );
    }
}

/// Reading a trigger changes nothing a later caller sees: two identical `GetTrigger`
/// calls answer the same trigger, and neither moves its state.
#[tokio::test]
async fn reads_do_not_change_what_a_later_caller_sees() {
    let router = emulator();
    register_targets(&router).await;

    for trigger_type in TRIGGER_TYPE_VALUES {
        let name = format!("idempotent-{trigger_type}");
        ok(
            &router,
            "CreateTrigger",
            &trigger_body(&name, trigger_type),
        )
        .await;
        let first = get_trigger(&router, &name).await;
        let second = get_trigger(&router, &name).await;
        assert_eq!(
            first, second,
            "case repeated GetTrigger of a {trigger_type} trigger: expected {first}, got {second}"
        );
    }
}

/// A trigger is served by the endpoint that created it: the same name in another Region
/// names another trigger, so a read through another Region's endpoint finds none and a
/// create of that name there succeeds.
#[tokio::test]
async fn a_trigger_is_reachable_only_through_the_region_that_created_it() {
    let router = emulator();
    register_targets(&router).await;
    ok(
        &router,
        "CreateTrigger",
        &trigger_body("regional-trigger", "ON_DEMAND"),
    )
    .await;

    for region in ["us-west-2", "eu-west-1"] {
        let (status, body) = call_in_region(
            &router,
            region,
            "GetTrigger",
            &json!({ "Name": "regional-trigger" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case region {region}: a trigger of another Region must not be read, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case region {region}: expected EntityNotFoundException, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A definition the service could not fire is refused with `InvalidInputException` and
/// nothing is stored.
///
/// The rows span the member a type requires (`Schedule` for SCHEDULED, `Predicate` for
/// CONDITIONAL, including a predicate present but carrying no condition), a
/// multi-condition predicate with no `Logical`, an `Actions` list that is present but
/// empty, and `StartOnCreation` on the one type that cannot be started on creation.
#[tokio::test]
async fn a_definition_the_service_could_not_fire_is_refused() {
    let router = emulator();
    register_targets(&router).await;

    let condition = json!({ "LogicalOperator": "EQUALS", "JobName": JOB, "State": "SUCCEEDED" });
    let cases: Vec<(&str, Value)> = vec![
        (
            "scheduled with no schedule",
            json!({ "Type": "SCHEDULED", "Actions": [{ "JobName": JOB }] }),
        ),
        (
            "conditional with no predicate",
            json!({ "Type": "CONDITIONAL", "Actions": [{ "JobName": JOB }] }),
        ),
        (
            "conditional with an empty condition list",
            json!({
                "Type": "CONDITIONAL",
                "Predicate": { "Conditions": [] },
                "Actions": [{ "JobName": JOB }],
            }),
        ),
        (
            "two conditions and no logical",
            json!({
                "Type": "CONDITIONAL",
                "Predicate": { "Conditions": [condition.clone(), condition.clone()] },
                "Actions": [{ "JobName": JOB }],
            }),
        ),
        (
            "no action at all",
            json!({ "Type": "ON_DEMAND", "Actions": [] }),
        ),
        (
            "on-demand started on creation",
            json!({
                "Type": "ON_DEMAND",
                "StartOnCreation": true,
                "Actions": [{ "JobName": JOB }],
            }),
        ),
    ];

    for (index, (case, request)) in cases.iter().enumerate() {
        let name = format!("unfireable-{index}");
        let mut request = request.clone();
        request["Name"] = json!(name);
        let (status, body) = call(&router, "CreateTrigger", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {body}"
        );
        // A refused create leaves nothing behind.
        let (status, body) = call(&router, "GetTrigger", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: the refused trigger must not be stored, got {status} with {body}"
        );
    }

    // The same requests with the missing member supplied are accepted, so each refusal is
    // about that member and not about the row's shape.
    for (index, trigger_type) in TRIGGER_TYPE_VALUES.iter().enumerate() {
        let name = format!("fireable-{index}");
        let (status, body) = call(&router, "CreateTrigger", &trigger_body(&name, trigger_type)).await;
        assert_eq!(
            status, 200,
            "case {trigger_type} with every required member: expected 200, got {status} with \
             {body}"
        );
    }
}

/// A `Schedule` is "a cron expression used to specify the schedule", of the documented
/// form `cron(<minutes> <hours> <day-of-month> <month> <day-of-week> <year>)`. A value of
/// that form is stored and reported back; anything else is refused with
/// `InvalidInputException` rather than stored as a schedule nothing can run.
///
/// The rows span the accepted forms (the documented example, a wildcard-only expression,
/// named day-of-week fields, a step, a year) and the refused ones (a bare cron body, a
/// rate expression, too few and too many fields, an unclosed expression, the empty
/// string).
#[tokio::test]
async fn a_schedule_must_be_a_cron_expression() {
    let router = emulator();
    register_targets(&router).await;

    // Source: the example on `Trigger$Schedule` / `CreateTriggerRequest$Schedule` ("to
    // run something every day at 12:15 UTC, you would specify: cron(15 12 * * ? *)").
    let accepted = [
        "cron(15 12 * * ? *)",
        "cron(* * * * ? *)",
        "cron(0 10 ? * MON-FRI *)",
        "cron(0/15 * * * ? *)",
        "cron(0 0 1 JAN ? 2027)",
    ];
    for (index, expression) in accepted.iter().enumerate() {
        let name = format!("scheduled-{index}");
        let mut request = trigger_body(&name, "SCHEDULED");
        request["Schedule"] = json!(expression);
        let (status, body) = call(&router, "CreateTrigger", &request).await;
        assert_eq!(
            status, 200,
            "case accepted {expression}: expected 200, got {status} with {body}"
        );
        let trigger = get_trigger(&router, &name).await;
        assert_eq!(
            trigger.get("Schedule").and_then(Value::as_str),
            Some(*expression),
            "case accepted {expression}: expected the sent expression reported back, got {trigger}"
        );
    }

    let refused = [
        "15 12 * * ? *",
        "rate(5 minutes)",
        "cron(15 12 * * ?)",
        "cron(15 12 * * ? * *)",
        "cron(15 12 * * ? *",
        "",
    ];
    for (index, expression) in refused.iter().enumerate() {
        let name = format!("unscheduled-{index}");
        let mut request = trigger_body(&name, "SCHEDULED");
        request["Schedule"] = json!(expression);
        let (status, body) = call(&router, "CreateTrigger", &request).await;
        assert_eq!(
            status, 400,
            "case refused {expression:?}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case refused {expression:?}: expected InvalidInputException, got {body}"
        );
        let (status, body) = call(&router, "GetTrigger", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case refused {expression:?}: the refused trigger must not be stored, got {status} \
             with {body}"
        );
    }
}

/// A trigger belongs to the workflow it names and runs the jobs and crawlers its actions
/// name, and waits on the ones its conditions name, so a definition naming an entity this
/// account does not hold is refused with `EntityNotFoundException`.
///
/// The rows span the place the name appears (the trigger's own workflow, an action, a
/// condition) and the kind of entity (a workflow, a job, a crawler), including a definition
/// whose FIRST target exists and whose second does not.
#[tokio::test]
async fn a_definition_naming_an_absent_entity_is_refused() {
    let router = emulator();
    register_targets(&router).await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "trigger names an absent workflow",
            json!({
                "Type": "ON_DEMAND",
                "WorkflowName": "no-such-workflow",
                "Actions": [{ "JobName": JOB }],
            }),
        ),
        (
            "action names an absent job",
            json!({ "Type": "ON_DEMAND", "Actions": [{ "JobName": "no-such-job" }] }),
        ),
        (
            "action names an absent crawler",
            json!({ "Type": "ON_DEMAND", "Actions": [{ "CrawlerName": "no-such-crawler" }] }),
        ),
        (
            "second action names an absent job",
            json!({
                "Type": "ON_DEMAND",
                "Actions": [{ "JobName": JOB }, { "JobName": "no-such-job" }],
            }),
        ),
        (
            "condition names an absent job",
            json!({
                "Type": "CONDITIONAL",
                "Predicate": {
                    "Conditions": [{
                        "LogicalOperator": "EQUALS",
                        "JobName": "no-such-job",
                        "State": "SUCCEEDED",
                    }],
                },
                "Actions": [{ "JobName": JOB }],
            }),
        ),
        (
            "condition names an absent crawler",
            json!({
                "Type": "CONDITIONAL",
                "Predicate": {
                    "Conditions": [{
                        "LogicalOperator": "EQUALS",
                        "CrawlerName": "no-such-crawler",
                        "CrawlState": "SUCCEEDED",
                    }],
                },
                "Actions": [{ "JobName": JOB }],
            }),
        ),
    ];

    for (index, (case, request)) in cases.iter().enumerate() {
        let name = format!("dangling-{index}");
        let mut request = request.clone();
        request["Name"] = json!(name);
        let (status, body) = call(&router, "CreateTrigger", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {body}"
        );
        let (status, body) = call(&router, "GetTrigger", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: the refused trigger must not be stored, got {status} with {body}"
        );
    }

    // A job of ANOTHER Region is not this Region's job, so a trigger created here cannot
    // run it: the same action is accepted where the job lives and refused where it does
    // not.
    let request = json!({
        "Name": "cross-region-action",
        "Type": "ON_DEMAND",
        "Actions": [{ "JobName": JOB }],
    });
    let (status, body) = call_in_region(&router, "us-west-2", "CreateTrigger", &request).await;
    assert_eq!(
        status, 400,
        "case job of another Region: expected 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("EntityNotFoundException"),
        "case job of another Region: expected EntityNotFoundException, got {body}"
    );
}

/// Addressing a trigger that is not stored is `EntityNotFoundException`, and creating one
/// whose name is already taken is `AlreadyExistsException` — both HTTP 400, and neither
/// changes the stored record.
///
/// The rows span absence (never created, and a name that differs only in case, because the
/// model documents no case fold for a trigger name) and the duplicate create.
#[tokio::test]
async fn absent_and_duplicate_triggers_answer_the_modelled_errors() {
    let router = emulator();
    register_targets(&router).await;
    ok(
        &router,
        "CreateTrigger",
        &trigger_body("present-trigger", "ON_DEMAND"),
    )
    .await;

    for (case, name) in [
        ("never created", "absent-trigger"),
        ("differs only in case", "PRESENT-TRIGGER"),
        ("differs only in one letter's case", "present-Trigger"),
    ] {
        let (status, body) = call(&router, "GetTrigger", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {body}"
        );
    }

    // A name that differs only in case is a DIFFERENT trigger, and creating it works.
    ok(
        &router,
        "CreateTrigger",
        &trigger_body("PRESENT-TRIGGER", "ON_DEMAND"),
    )
    .await;
    for name in ["present-trigger", "PRESENT-TRIGGER"] {
        let trigger = get_trigger(&router, name).await;
        assert_eq!(
            trigger.get("Name").and_then(Value::as_str),
            Some(name),
            "case casing {name}: expected its own record, got {trigger}"
        );
    }

    // The duplicate create is refused, and leaves the stored trigger as it was.
    let stored = get_trigger(&router, "present-trigger").await;
    let mut duplicate = trigger_body("present-trigger", "ON_DEMAND");
    duplicate["Description"] = json!("a second create must not overwrite the first");
    let (status, body) = call(&router, "CreateTrigger", &duplicate).await;
    assert_eq!(
        status, 400,
        "case duplicate create: expected 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("AlreadyExistsException"),
        "case duplicate create: expected AlreadyExistsException, got {body}"
    );
    assert_eq!(
        get_trigger(&router, "present-trigger").await,
        stored,
        "case duplicate create: the stored trigger must be unchanged, expected {stored}"
    );
}

/// The store holds a bounded number of triggers: the create at the ceiling succeeds, the
/// one past it is refused with the modelled limit error, and freeing a slot makes a create
/// succeed again — so the ceiling is a bound on what is stored, not a latch on how many
/// creates were ever made.
#[tokio::test]
async fn a_create_past_the_trigger_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    register_targets(&router).await;

    // Reaching the ceiling over the wire would take a thousand requests and would
    // exercise the transport rather than the bound.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for index in 0..MAX_TRIGGERS - 1 {
            let name = format!("seeded-{index}");
            let input = CreateTriggerInput {
                name: name.clone(),
                r#type: "ON_DEMAND".to_string(),
                ..CreateTriggerInput::default()
            };
            let entry = TriggerEntry::from_create_input(&input, TRIGGER_STATE_CREATED);
            store.insert_trigger(crate::wire::REGION, entry);
        }
    }

    ok(
        &router,
        "CreateTrigger",
        &trigger_body("last-trigger", "ON_DEMAND"),
    )
    .await;
    let (status, body) = call(
        &router,
        "CreateTrigger",
        &trigger_body("one-too-many", "ON_DEMAND"),
    )
    .await;
    assert_eq!(
        status, 400,
        "case trigger {} of {MAX_TRIGGERS}: expected 400, got {status} with {body}",
        MAX_TRIGGERS + 1
    );
    assert!(
        error_code(&body).contains("ResourceNumberLimitExceededException"),
        "case one trigger too many: expected ResourceNumberLimitExceededException, got {body}"
    );

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        store
            .triggers
            .get_mut(crate::wire::REGION)
            .expect("the seeded Region holds triggers")
            .remove("seeded-0");
    }
    ok(
        &router,
        "CreateTrigger",
        &trigger_body("one-too-many", "ON_DEMAND"),
    )
    .await;
}

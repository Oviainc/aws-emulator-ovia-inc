// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteTrigger`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: `DeleteTrigger` removes the trigger the request names from the
//! Region that holds it, with the WHOLE record — its definition, the state a start or a
//! stop left it in, and the workflow it belonged to — so every projection built from the
//! stored triggers (the reads, the listings, the batch read, the graph of its workflow)
//! stops reporting it and the name is free for the next create; the delete is idempotent,
//! so a name the Region holds no trigger under is answered the same way ("If the trigger
//! is not found, no exception is thrown") and leaves every other record standing.
//!
//! The recorded case — one delete of a stored trigger — is ONE row of the tables below;
//! the rows differ along the trigger's type (each of the four modelled `TriggerType`
//! values), the state the delete arrives at (registered, started on creation, started by a
//! call, stopped), how many actions it initiates and of which kind, whether it belongs to
//! a workflow at all, whether it carried tags and a description, and the form and length
//! of its name.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    messages, NODE_TYPE_TRIGGER, TRIGGER_STATE_ACTIVATED, TRIGGER_STATE_CREATED,
    TRIGGER_STATE_DEACTIVATED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_messages::assert_refusal;
use crate::trigger_starts::{register_targets, trigger_state, CRAWLER, JOB, OTHER_JOB};
use crate::wire::{call_in_region, emulator, error_code, names_in, ok, string_list, REGION};

/// The workflow the rows that belong to one are attached to.
const WORKFLOW: &str = "trigger-deletion-workflow";

/// A trigger beside each row's own, in the same workflow, so a delete that removed more
/// than the one record it names is visible.
const BYSTANDER: &str = "trigger-deletion-bystander";

/// Another Region's endpoint, to show a delete resolves the trigger in the endpoint's
/// Region alone.
const OTHER_REGION: &str = "eu-west-1";

/// One row of the deletion table: the trigger to store, how it came to be in the state the
/// delete arrives at, and that state.
struct Row {
    /// What this row varies, named in every assertion it drives.
    case: &'static str,
    /// The trigger's name.
    name: String,
    /// The `CreateTrigger` body, minus `Name` and `WorkflowName`.
    definition: Value,
    /// Whether the trigger belongs to [`WORKFLOW`].
    in_workflow: bool,
    /// The lifecycle call made before the delete, if any.
    lifecycle: Option<&'static str>,
    /// The `TriggerState` a read reports when the delete arrives.
    state_before: &'static str,
}

/// The rows: the class the rule generalises over, one row per axis.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "on-demand, one job action, registered and never started",
            name: "on-demand-delete".to_string(),
            definition: json!({
                "Type": "ON_DEMAND",
                "Actions": [{ "JobName": JOB }],
            }),
            in_workflow: true,
            lifecycle: None,
            state_before: TRIGGER_STATE_CREATED,
        },
        Row {
            // "This field is required when the trigger type is SCHEDULED", and "Set to
            // true to start SCHEDULED and CONDITIONAL triggers when created".
            case: "scheduled, started on creation, a job and a crawler action, with tags",
            name: "scheduled-delete".to_string(),
            definition: json!({
                "Type": "SCHEDULED",
                "Schedule": "cron(15 12 * * ? *)",
                "StartOnCreation": true,
                "Description": "the trigger a delete removes",
                "Tags": { "owner": "data-platform" },
                "Actions": [{ "JobName": JOB }, { "CrawlerName": CRAWLER }],
            }),
            in_workflow: true,
            lifecycle: None,
            state_before: TRIGGER_STATE_ACTIVATED,
        },
        Row {
            // "This field is required when the trigger type is CONDITIONAL." The predicate
            // waits on a job none of the actions runs, and the trigger belongs to no
            // workflow at all.
            case: "conditional, started by a call, outside any workflow",
            name: "conditional-delete".to_string(),
            definition: json!({
                "Type": "CONDITIONAL",
                "Predicate": {
                    "Conditions": [{
                        "LogicalOperator": "EQUALS",
                        "JobName": OTHER_JOB,
                        "State": "SUCCEEDED",
                    }],
                },
                "Actions": [{ "JobName": JOB }],
            }),
            in_workflow: false,
            lifecycle: Some("StartTrigger"),
            state_before: TRIGGER_STATE_ACTIVATED,
        },
        Row {
            // "Batch condition that must be met … before EventBridge event trigger fires."
            case: "event, stopped before the delete",
            name: "event-delete".to_string(),
            definition: json!({
                "Type": "EVENT",
                "EventBatchingCondition": { "BatchSize": 3, "BatchWindow": 900 },
                "Actions": [{ "JobName": OTHER_JOB }],
            }),
            in_workflow: true,
            lifecycle: Some("StopTrigger"),
            state_before: TRIGGER_STATE_DEACTIVATED,
        },
        Row {
            case: "a one-character name",
            // The lower published bound of `NameString`'s `length 1..255`
            // (`smithy_query operation=DeleteTrigger`).
            name: "t".to_string(),
            definition: json!({
                "Type": "ON_DEMAND",
                "Actions": [{ "CrawlerName": CRAWLER }],
            }),
            in_workflow: true,
            lifecycle: None,
            state_before: TRIGGER_STATE_CREATED,
        },
        Row {
            case: "a name at the modelled maximum length",
            // The other published bound of that same `length 1..255`.
            name: "t".repeat(255),
            definition: json!({
                "Type": "ON_DEMAND",
                "Actions": [{ "JobName": JOB }],
            }),
            in_workflow: false,
            lifecycle: Some("StartTrigger"),
            state_before: TRIGGER_STATE_CREATED,
        },
        Row {
            case: "punctuation and mixed case in the name",
            name: "Team.Data_Lake-2026/raw".to_string(),
            definition: json!({
                "Type": "SCHEDULED",
                "Schedule": "cron(0 */6 * * ? *)",
                "Actions": [
                    { "JobName": JOB },
                    { "JobName": OTHER_JOB, "Timeout": 30 },
                    { "CrawlerName": CRAWLER },
                ],
            }),
            in_workflow: true,
            lifecycle: Some("StartTrigger"),
            state_before: TRIGGER_STATE_ACTIVATED,
        },
    ]
}

/// Register the workflow and the bystander trigger every row is deleted beside.
async fn register_bystanders(router: &MultiServiceRouter) {
    register_targets(router).await;
    ok(router, "CreateWorkflow", &json!({ "Name": WORKFLOW })).await;
    ok(
        router,
        "CreateTrigger",
        &json!({
            "Name": BYSTANDER,
            "WorkflowName": WORKFLOW,
            "Type": "ON_DEMAND",
            "Actions": [{ "JobName": OTHER_JOB }],
        }),
    )
    .await;
}

/// Store the row's trigger and put it in the state its delete arrives at.
async fn register_row(router: &MultiServiceRouter, row: &Row) {
    let mut body = row.definition.clone();
    body["Name"] = json!(row.name);
    if row.in_workflow {
        body["WorkflowName"] = json!(WORKFLOW);
    }
    ok(router, "CreateTrigger", &body).await;
    if let Some(operation) = row.lifecycle {
        ok(router, operation, &json!({ "Name": row.name })).await;
    }
}

/// The names of the `TRIGGER` nodes the graph of `workflow` reports: the triggers the
/// workflow still holds.
async fn graph_trigger_names(
    router: &MultiServiceRouter,
    case: &str,
    workflow: &str,
) -> Vec<String> {
    let body = ok(
        router,
        "GetWorkflow",
        &json!({ "Name": workflow, "IncludeGraph": true }),
    )
    .await;
    let graph = body["Workflow"]
        .get("Graph")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: IncludeGraph must report a Graph, got {body}"));
    graph
        .get("Nodes")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: Graph.Nodes must be a list, got {graph}"))
        .iter()
        .filter(|node| node.get("Type").and_then(Value::as_str) == Some(NODE_TYPE_TRIGGER))
        .map(|node| {
            node.get("Name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a node reports its Name, got {node}"))
                .to_string()
        })
        .collect()
}

/// The names `ListTriggers` reports through this Region's endpoint.
async fn listed_names(router: &MultiServiceRouter) -> Vec<String> {
    let body = ok(router, "ListTriggers", &json!({})).await;
    string_list(&body, "TriggerNames")
}

// ---------------------------------------------------------------------------
// Delete, then read back
// ---------------------------------------------------------------------------

/// A delete removes the named trigger and the whole record with it, answers with the name,
/// leaves every other record standing, and frees the name for the next create.
#[tokio::test]
async fn a_delete_removes_the_named_trigger_and_answers_with_its_name() {
    for row in rows() {
        let router = emulator();
        let case = row.case;
        register_bystanders(&router).await;
        register_row(&router, &row).await;

        let before = trigger_state(&router, &row.name).await;
        assert_eq!(
            before, row.state_before,
            "case {case:?}: the delete must arrive at a trigger in {:?}, got {before:?}",
            row.state_before
        );

        let deleted = ok(&router, "DeleteTrigger", &json!({ "Name": row.name })).await;
        assert_eq!(
            deleted.get("Name").and_then(Value::as_str),
            Some(row.name.as_str()),
            "case {case:?}: the delete must answer with the name it deleted, got {deleted}"
        );

        // The trigger is gone: the read of it is the modelled not-found refusal, the one
        // recorded sentence this surface words a trigger miss with.
        assert_refusal(
            &router,
            case,
            "GetTrigger",
            &json!({ "Name": row.name }),
            "EntityNotFoundException",
            messages::TRIGGER_NOT_FOUND,
        )
        .await;

        // And it is gone from every projection of the namespace, with only the bystander
        // left.
        let listed = listed_names(&router).await;
        assert_eq!(
            listed,
            vec![BYSTANDER.to_string()],
            "case {case:?}: the delete must leave only the bystander listed, got {listed:?}"
        );
        let projected = names_in(&ok(&router, "GetTriggers", &json!({})).await, "Triggers");
        assert_eq!(
            projected,
            vec![BYSTANDER.to_string()],
            "case {case:?}: the delete must leave only the bystander projected, got \
             {projected:?}"
        );
        let batch = ok(
            &router,
            "BatchGetTriggers",
            &json!({ "TriggerNames": [row.name, BYSTANDER] }),
        )
        .await;
        assert_eq!(
            string_list(&batch, "TriggersNotFound"),
            vec![row.name.clone()],
            "case {case:?}: the batch read must report the deleted name as missing, got \
             {batch}"
        );
        assert_eq!(
            names_in(&batch, "Triggers"),
            vec![BYSTANDER.to_string()],
            "case {case:?}: the batch read must still resolve the bystander, got {batch}"
        );

        // The workflow keeps the triggers it still holds and loses the one that went: the
        // graph is built from the stored triggers, so a deleted one is no node of it.
        let nodes = graph_trigger_names(&router, case, WORKFLOW).await;
        assert!(
            !nodes.contains(&row.name),
            "case {case:?}: the deleted trigger must be no node of its workflow, got \
             {nodes:?}"
        );
        assert!(
            nodes.contains(&BYSTANDER.to_string()),
            "case {case:?}: the bystander must still be a node of the workflow, got \
             {nodes:?}"
        );

        // Nothing the trigger named went with it: the workflow it belonged to and the job
        // and crawler its actions initiated are all still served.
        let kept_bystander = ok(&router, "GetTrigger", &json!({ "Name": BYSTANDER })).await;
        assert_eq!(
            kept_bystander["Trigger"].get("Name").and_then(Value::as_str),
            Some(BYSTANDER),
            "case {case:?}: the delete must leave the bystander readable, got \
             {kept_bystander}"
        );
        ok(&router, "GetJob", &json!({ "JobName": JOB })).await;
        ok(&router, "GetCrawler", &json!({ "Name": CRAWLER })).await;

        // The name is free again, and the record created under it is the NEW one: neither
        // the deleted definition, nor the state it was in, nor its workflow membership
        // survives into the next create.
        ok(
            &router,
            "CreateTrigger",
            &json!({
                "Name": row.name,
                "Type": "ON_DEMAND",
                "Description": "the trigger created after the delete",
                "Actions": [{ "JobName": OTHER_JOB }],
            }),
        )
        .await;
        let again = ok(&router, "GetTrigger", &json!({ "Name": row.name })).await;
        let fresh = &again["Trigger"];
        assert_eq!(
            fresh.get("Description").and_then(Value::as_str),
            Some("the trigger created after the delete"),
            "case {case:?}: the re-created trigger is the one this create sent, got {again}"
        );
        assert_eq!(
            fresh.get("Type").and_then(Value::as_str),
            Some("ON_DEMAND"),
            "case {case:?}: the re-created trigger carries its own type, got {again}"
        );
        assert_eq!(
            fresh.get("State").and_then(Value::as_str),
            Some(TRIGGER_STATE_CREATED),
            "case {case:?}: the re-created trigger starts in {TRIGGER_STATE_CREATED}, got \
             {again}"
        );
        assert_eq!(
            fresh.get("Schedule").and_then(Value::as_str),
            None,
            "case {case:?}: the deleted trigger's schedule must not survive, got {again}"
        );
        assert_eq!(
            fresh.get("WorkflowName").and_then(Value::as_str),
            None,
            "case {case:?}: the deleted trigger's workflow must not survive, got {again}"
        );
    }
}

/// A delete of a name the endpoint's Region holds no trigger under is served, not refused:
/// it answers with the name it was given and changes nothing.
///
/// The rows are the ways a name can name no trigger here: one nothing ever created, one
/// just deleted, one differing only in case from a stored trigger, one at the modelled
/// maximum length, one of punctuation and digits, and the name of a trigger stored in
/// ANOTHER Region.
#[tokio::test]
async fn a_delete_of_a_name_no_trigger_is_stored_under_changes_nothing() {
    let router = emulator();
    register_bystanders(&router).await;

    // A trigger of another Region, created through that Region's endpoint — with the job
    // its action names created there too, because a job lives in one Region.
    let elsewhere = "eu-only-trigger";
    let (status, body) = call_in_region(
        &router,
        OTHER_REGION,
        "CreateJob",
        &json!({
            "Name": JOB,
            "Role": "arn:aws:iam::000000000000:role/sim2real-glue-job-role",
            "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/eu.py" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the other Region's job create must be served, got {status} with {body}"
    );
    let (status, body) = call_in_region(
        &router,
        OTHER_REGION,
        "CreateTrigger",
        &json!({
            "Name": elsewhere,
            "Type": "ON_DEMAND",
            "Actions": [{ "JobName": JOB }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the other Region's create must be served, got {status} with {body}"
    );

    // One name that WAS stored here, deleted first, so the second delete of it is the
    // idempotent one.
    let gone = "already-deleted-trigger";
    ok(
        &router,
        "CreateTrigger",
        &json!({
            "Name": gone,
            "Type": "ON_DEMAND",
            "Actions": [{ "JobName": JOB }],
        }),
    )
    .await;
    ok(&router, "DeleteTrigger", &json!({ "Name": gone })).await;

    let unknown: Vec<(&str, String)> = vec![
        ("never created", "no-such-trigger".to_string()),
        ("already deleted", gone.to_string()),
        ("differing only in case", BYSTANDER.to_uppercase()),
        ("at the modelled maximum length", "n".repeat(255)),
        ("punctuation and digits", "team.data_lake-2026/raw".to_string()),
        ("stored in another Region", elsewhere.to_string()),
    ];
    let definition_before = ok(&router, "GetTrigger", &json!({ "Name": BYSTANDER })).await;
    for (case, name) in &unknown {
        let answer = ok(&router, "DeleteTrigger", &json!({ "Name": name })).await;
        assert_eq!(
            answer.get("Name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: an idempotent delete answers with the name it was given, got \
             {answer}"
        );

        // Nothing else moved: this Region still holds exactly the trigger it stored, with
        // the definition it was stored with.
        let listed = listed_names(&router).await;
        assert_eq!(
            listed,
            vec![BYSTANDER.to_string()],
            "case {case:?}: the delete must leave the stored trigger listed, got {listed:?}"
        );
        let definition_after = ok(&router, "GetTrigger", &json!({ "Name": BYSTANDER })).await;
        assert_eq!(
            definition_after, definition_before,
            "case {case:?}: the stored trigger must be untouched, got {definition_after} for \
             {definition_before}"
        );
    }

    // The other Region's trigger is still served there: a delete addressed to this
    // endpoint never reached it.
    let (status, body) = call_in_region(
        &router,
        OTHER_REGION,
        "GetTrigger",
        &json!({ "Name": elsewhere }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the other Region must still serve its own trigger, got {status} with {body}"
    );

    // And a delete through THAT endpoint does remove it, so the misses above are about the
    // Region and not about the name.
    let (status, body) = call_in_region(
        &router,
        OTHER_REGION,
        "DeleteTrigger",
        &json!({ "Name": elsewhere }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the owning Region's delete must be served, got {status} with {body}"
    );
    assert_eq!(
        body.get("Name").and_then(Value::as_str),
        Some(elsewhere),
        "the owning Region's delete answers with the name it deleted, got {body}"
    );
    let (status, body) = call_in_region(
        &router,
        OTHER_REGION,
        "GetTrigger",
        &json!({ "Name": elsewhere }),
    )
    .await;
    assert_eq!(
        status, 400,
        "the other Region's trigger must be gone after its own delete, got {status} with \
         {body}"
    );
    assert!(
        error_code(&body).ends_with("EntityNotFoundException"),
        "expected EntityNotFoundException from the read in {OTHER_REGION}, got {}",
        error_code(&body)
    );

    // This Region, whose endpoint the misses were addressed to, is untouched by that
    // delete as well.
    let listed = listed_names(&router).await;
    assert_eq!(
        listed,
        vec![BYSTANDER.to_string()],
        "the other Region's delete must leave {REGION} alone, got {listed:?}"
    );
}

/// Repeating a delete is served every time: the first one removes the record and each
/// later one is the idempotent answer, whatever state the trigger was in when it went.
#[tokio::test]
async fn repeating_a_delete_is_served_every_time() {
    for row in rows() {
        let router = emulator();
        let case = row.case;
        register_bystanders(&router).await;
        register_row(&router, &row).await;

        for attempt in 1..=3 {
            let answer = ok(&router, "DeleteTrigger", &json!({ "Name": row.name })).await;
            assert_eq!(
                answer.get("Name").and_then(Value::as_str),
                Some(row.name.as_str()),
                "case {case:?}: delete {attempt} answers with the name it was given, got \
                 {answer}"
            );
            let listed = listed_names(&router).await;
            assert_eq!(
                listed,
                vec![BYSTANDER.to_string()],
                "case {case:?}: after delete {attempt} only the bystander is listed, got \
                 {listed:?}"
            );
        }
    }
}

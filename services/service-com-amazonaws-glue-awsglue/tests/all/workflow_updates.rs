// SPDX-License-Identifier: CC0-1.0

//! `UpdateWorkflow`: patching a workflow this account already holds.
//!
//! RULE: an update addresses one workflow by the `Name` its create stored and is a PATCH
//! over the stored definition — every member the request carries replaces the stored one,
//! a member whose value is a whole collection replaces that collection whole rather than
//! merging into it, every member the request omits is preserved, and a later read reports
//! the merged record and the instant of the write. A `Name` this account holds no workflow
//! under is refused with the operation's modelled `EntityNotFoundException` rather than
//! creating anything.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler. The job, the
//! entry-point trigger and the run readers are the ones
//! [`crate::workflow_run_properties`] already defines, so every suite that starts a run
//! registers one the same way.
//!
//! The rows differ along what the rule generalises over: which members the update names
//! (one, two, all of them), which members the workflow already carried, the form and size
//! of the value sent (an empty description, a description at the modelled length bound, one
//! property, many properties, a property collection replacing another, an empty collection,
//! a ceiling of zero, of one and a large one, a name carrying whitespace) and — for the
//! refusals — the form of the name, including each published length bound and one past it.

use serde_json::{json, Map, Value};
use service_com_amazonaws_glue_awsglue::state::messages::WORKFLOW_NOT_FOUND;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{assert_echoed, call, emulator, error_code, error_message, ok};
use crate::workflow_run_properties::{
    get_properties, register_job, register_workflow, start_run,
};

/// The longest `Name` the model accepts: "len 1..255" on `NameString`
/// (`smithy_query operation=UpdateWorkflow`).
const MAX_NAME_LENGTH: usize = 255;

/// The longest `Description` the model accepts: "len 0..120000" on
/// `WorkflowDescriptionString` (`smithy_query operation=UpdateWorkflow`).
const MAX_DESCRIPTION_LENGTH: usize = 120_000;

/// One row of the patch table.
struct PatchRow {
    /// What this row varies, named in every assertion it drives.
    case: &'static str,
    /// The members the create carried beside the `Name`, or `null` for a bare create.
    created: Value,
    /// The members the update names beside the `Name`, or `null` for an update that names
    /// none.
    update: Value,
    /// The members the read must then report, with `null` for a member carrying no value.
    /// `DefaultRunProperties` is asserted on its own, through [`default_run_properties`].
    expected: Value,
    /// The default run properties the read must then report, written out.
    expected_properties: Value,
}

/// Create one workflow under `name`, carrying `members` beside its name.
async fn create_workflow(router: &MultiServiceRouter, name: &str, members: &Value) {
    let mut body = json!({ "Name": name });
    if let Some(named) = members.as_object() {
        for (member, value) in named {
            body[member] = value.clone();
        }
    }
    ok(router, "CreateWorkflow", &body).await;
}

/// The `Workflow` structure `GetWorkflow` reports for `name`.
async fn get_workflow(router: &MultiServiceRouter, name: &str) -> Value {
    let body = ok(router, "GetWorkflow", &json!({ "Name": name })).await;
    body.get("Workflow")
        .cloned()
        .unwrap_or_else(|| panic!("GetWorkflow must report a Workflow, got {body}"))
}

/// Send one update, assert it answers "the name of the workflow which was specified in
/// input", and answer the body.
async fn update_workflow(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    members: &Value,
) -> Value {
    let mut body = json!({ "Name": name });
    if let Some(named) = members.as_object() {
        for (member, value) in named {
            body[member] = value.clone();
        }
    }
    let answered = ok(router, "UpdateWorkflow", &body).await;
    assert_eq!(
        answered["Name"].as_str(),
        Some(name),
        "case {case:?}: an update must answer the name of the workflow it updated, got \
         {answered}"
    );
    answered
}

/// The default run properties a read reports, with the absent map read as the empty one
/// awsJson may omit.
fn default_run_properties(workflow: &Value) -> Value {
    match workflow.get("DefaultRunProperties") {
        None | Some(Value::Null) => json!({}),
        Some(properties) => properties.clone(),
    }
}

/// A property collection of `count` keys, so a row can span how many properties a workflow
/// holds without writing them all out.
fn many_properties(count: usize, value_prefix: &str) -> Value {
    let properties: Map<String, Value> = (0..count)
        .map(|index| {
            (
                format!("property-{index}"),
                json!(format!("{value_prefix}-{index}")),
            )
        })
        .collect();
    Value::Object(properties)
}

/// The rows the patch suite drives.
fn patch_rows() -> Vec<PatchRow> {
    vec![
        PatchRow {
            case: "only the description, onto a workflow carrying none",
            created: Value::Null,
            update: json!({ "Description": "nightly ingest and enrichment" }),
            expected: json!({
                "Description": "nightly ingest and enrichment",
                "MaxConcurrentRuns": Value::Null,
            }),
            expected_properties: json!({}),
        },
        PatchRow {
            case: "a description replacing the stored one",
            created: json!({ "Description": "the first description" }),
            update: json!({ "Description": "the second description" }),
            expected: json!({ "Description": "the second description" }),
            expected_properties: json!({}),
        },
        PatchRow {
            // `WorkflowDescriptionString` is `length 0..120000`, so the empty description
            // is a value the model accepts and is stored as the create stores one.
            case: "an empty description",
            created: json!({ "Description": "replaced" }),
            update: json!({ "Description": "" }),
            expected: json!({ "Description": "" }),
            expected_properties: json!({}),
        },
        PatchRow {
            // The other published bound of the same member: the longest description the
            // model accepts is stored whole.
            case: "a description at the modelled length bound",
            created: Value::Null,
            update: json!({ "Description": "d".repeat(MAX_DESCRIPTION_LENGTH) }),
            expected: json!({ "Description": "d".repeat(MAX_DESCRIPTION_LENGTH) }),
            expected_properties: json!({}),
        },
        PatchRow {
            case: "one default run property, onto a workflow carrying none",
            created: Value::Null,
            update: json!({ "DefaultRunProperties": { "env": "staging" } }),
            expected: json!({ "Description": Value::Null }),
            expected_properties: json!({ "env": "staging" }),
        },
        PatchRow {
            // The collection is ONE member: a request naming it replaces it whole, so a
            // key the stored collection held and the request did not name is gone.
            case: "a property collection replacing the stored collection",
            created: json!({ "DefaultRunProperties": { "stage": "extract", "retries": "0" } }),
            update: json!({ "DefaultRunProperties": { "stage": "load" } }),
            expected: json!({}),
            expected_properties: json!({ "stage": "load" }),
        },
        PatchRow {
            case: "an empty property collection over a stored one",
            created: json!({ "DefaultRunProperties": { "stage": "extract" } }),
            update: json!({ "DefaultRunProperties": {} }),
            expected: json!({}),
            expected_properties: json!({}),
        },
        PatchRow {
            case: "many default run properties at once",
            created: json!({ "DefaultRunProperties": { "stage": "extract" } }),
            update: json!({ "DefaultRunProperties": many_properties(32, "value") }),
            expected: json!({}),
            expected_properties: many_properties(32, "value"),
        },
        PatchRow {
            case: "a property with an empty value and a non-ASCII key and value",
            created: Value::Null,
            update: json!({
                "DefaultRunProperties": { "stage": "", "jeu-de-données": "ventes-été" },
            }),
            expected: json!({}),
            expected_properties: json!({ "stage": "", "jeu-de-données": "ventes-été" }),
        },
        PatchRow {
            case: "only the concurrency ceiling, onto a workflow carrying none",
            created: Value::Null,
            update: json!({ "MaxConcurrentRuns": 25 }),
            expected: json!({ "Description": Value::Null, "MaxConcurrentRuns": 25 }),
            expected_properties: json!({}),
        },
        PatchRow {
            case: "a smaller concurrency ceiling over a stored one",
            created: json!({ "MaxConcurrentRuns": 10 }),
            update: json!({ "MaxConcurrentRuns": 1 }),
            expected: json!({ "MaxConcurrentRuns": 1 }),
            expected_properties: json!({}),
        },
        PatchRow {
            // `MaxConcurrentRuns` is a `NullableInteger` the model constrains no range on,
            // so a zero is a value the request may carry and is stored as it was sent —
            // the same way the create stores it.
            case: "a concurrency ceiling of zero",
            created: json!({ "MaxConcurrentRuns": 4 }),
            update: json!({ "MaxConcurrentRuns": 0 }),
            expected: json!({ "MaxConcurrentRuns": 0 }),
            expected_properties: json!({}),
        },
        PatchRow {
            case: "one member named, the other two left as the create stored them",
            created: json!({
                "Description": "the create's description",
                "DefaultRunProperties": { "stage": "extract" },
                "MaxConcurrentRuns": 3,
            }),
            update: json!({ "Description": "only this moved" }),
            expected: json!({ "Description": "only this moved", "MaxConcurrentRuns": 3 }),
            expected_properties: json!({ "stage": "extract" }),
        },
        PatchRow {
            case: "every member the update can name",
            created: json!({
                "Description": "replaced",
                "DefaultRunProperties": { "stage": "extract" },
                "MaxConcurrentRuns": 2,
            }),
            update: json!({
                "Description": "every member replaced",
                "DefaultRunProperties": { "stage": "load", "shards": "16" },
                "MaxConcurrentRuns": 9,
            }),
            expected: json!({ "Description": "every member replaced", "MaxConcurrentRuns": 9 }),
            expected_properties: json!({ "stage": "load", "shards": "16" }),
        },
    ]
}

#[tokio::test]
async fn an_update_replaces_the_members_it_names_and_preserves_the_rest() {
    let router = emulator();

    for (index, row) in patch_rows().into_iter().enumerate() {
        let case = row.case;
        // Each row gets its own workflow, so a row's write cannot be read as another
        // row's. One row's name carries the whitespace `NameString` admits, which is
        // another form of the member that addresses the record.
        let name = if index == 0 {
            format!("patched workflow\t{index}")
        } else {
            format!("patched-workflow-{index}")
        };
        create_workflow(&router, &name, &row.created).await;
        let before = get_workflow(&router, &name).await;

        update_workflow(&router, case, &name, &row.update).await;

        let after = get_workflow(&router, &name).await;
        assert_echoed(case, &row.expected, &after);
        assert_eq!(
            default_run_properties(&after),
            row.expected_properties,
            "case {case:?}: the read must report the properties the update left, got {after}"
        );

        // `Name` addresses the workflow and the request has no member for it, so the
        // merged record still lives under it.
        assert_eq!(
            after["Name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: expected the workflow addressed by {name:?}, got {after}"
        );

        // The members no update can move: the creation instant, and the runs of a workflow
        // nothing has started one of.
        let created_on = before["CreatedOn"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: CreatedOn must be a timestamp, got {before}")
        });
        assert_eq!(
            after["CreatedOn"].as_f64(),
            Some(created_on),
            "case {case:?}: an update must not move the creation instant, got {after}"
        );
        assert!(
            after.get("LastRun").is_none_or(Value::is_null),
            "case {case:?}: an update starts no run of the workflow, got {after}"
        );

        // The write is what moves the workflow's last-modified instant, which was the
        // creation instant until now.
        let last_modified = after["LastModifiedOn"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: LastModifiedOn must be a timestamp, got {after}")
        });
        assert!(
            last_modified > created_on,
            "case {case:?}: expected the update's instant to be later than the creation \
             instant {created_on}, got {last_modified}"
        );

        // A read does not change what the next caller sees.
        let again = get_workflow(&router, &name).await;
        assert_eq!(
            again, after,
            "case {case:?}: the read of the merged workflow must be repeatable, got {again}"
        );
    }
}

#[tokio::test]
async fn an_update_writes_to_the_workflow_it_names_only() {
    let router = emulator();

    // Two workflows carrying the same definition: the update names one of them.
    for name in ["addressed-workflow", "other-workflow"] {
        create_workflow(
            &router,
            name,
            &json!({
                "Description": "the create's description",
                "DefaultRunProperties": { "stage": "extract" },
                "MaxConcurrentRuns": 3,
            }),
        )
        .await;
    }
    let case = "an update on one of two identical workflows";
    let untouched_before = get_workflow(&router, "other-workflow").await;

    update_workflow(
        &router,
        case,
        "addressed-workflow",
        &json!({ "Description": "moved", "MaxConcurrentRuns": 7 }),
    )
    .await;

    let addressed = get_workflow(&router, "addressed-workflow").await;
    assert_echoed(
        case,
        &json!({ "Description": "moved", "MaxConcurrentRuns": 7 }),
        &addressed,
    );
    let untouched = get_workflow(&router, "other-workflow").await;
    assert_eq!(
        untouched, untouched_before,
        "case {case:?}: the workflow the update did not name is left as it was, got \
         {untouched}"
    );
}

#[tokio::test]
async fn an_update_naming_no_member_to_change_writes_nothing() {
    let router = emulator();

    // (case, the name, the members the create carried)
    for (case, name, created) in [
        (
            "a workflow carrying only its name",
            "untouched-bare",
            Value::Null,
        ),
        (
            "a workflow carrying every optional member",
            "untouched-full",
            json!({
                "Description": "untouched",
                "DefaultRunProperties": { "stage": "extract" },
                "MaxConcurrentRuns": 6,
            }),
        ),
    ] {
        create_workflow(&router, name, &created).await;
        let before = get_workflow(&router, name).await;

        // Every member of the request other than the `Name` that addresses the workflow is
        // optional, so a request naming none of them asks for no change.
        update_workflow(&router, case, name, &Value::Null).await;

        let after = get_workflow(&router, name).await;
        assert_eq!(
            after, before,
            "case {case:?}: an update naming no member must leave the workflow, \
             LastModifiedOn included, exactly as it was"
        );
    }
}

#[tokio::test]
async fn a_name_this_account_holds_no_workflow_under_is_refused() {
    let router = emulator();
    create_workflow(
        &router,
        "StoredWorkflow",
        &json!({ "Description": "stored", "MaxConcurrentRuns": 2 }),
    )
    .await;
    let before = get_workflow(&router, "StoredWorkflow").await;

    // The model documents no case fold for a workflow name, so a differently-cased name is
    // a different workflow; the last two rows are the published length bounds of
    // `NameString` ("len 1..255"), where the answer is a miss and not a boundary refusal.
    for (case, name) in [
        ("never created", "no-such-workflow".to_string()),
        ("a prefix of a stored name", "Stored".to_string()),
        ("a suffix of a stored name", "Workflow".to_string()),
        ("a stored name lowercased", "storedworkflow".to_string()),
        ("a stored name uppercased", "STOREDWORKFLOW".to_string()),
        ("the shortest name the model accepts", "w".to_string()),
        (
            "the longest name the model accepts",
            "w".repeat(MAX_NAME_LENGTH),
        ),
    ] {
        let (status, body) = call(
            &router,
            "UpdateWorkflow",
            &json!({ "Name": name, "Description": "written nowhere" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled EntityNotFoundException's 400, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException, got {}",
            error_code(&body)
        );
        // This crate's one sentence for a workflow a request named and the store does not
        // hold, which names no workflow at all.
        assert_eq!(
            error_message(&body),
            WORKFLOW_NOT_FOUND,
            "case {case:?}: expected the recorded refusal for a workflow that resolved to \
             nothing, got {body}"
        );

        // A miss creates nothing: the name the request addressed still resolves to no
        // workflow at all.
        let (status, body) = call(&router, "GetWorkflow", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a missed update must register no workflow, got {status} with {body}"
        );
    }

    // One step past the model's upper length bound is refused at the BOUNDARY, before the
    // store is consulted at all.
    let case = "one character past the longest name the model accepts";
    let (status, body) = call(
        &router,
        "UpdateWorkflow",
        &json!({ "Name": "w".repeat(MAX_NAME_LENGTH + 1), "Description": "written nowhere" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).ends_with("ValidationException"),
        "case {case:?}: a modelled constraint is refused at the boundary, got {} with {}",
        error_code(&body),
        error_message(&body)
    );

    // A refused update wrote nothing: the one workflow this account holds still reads back
    // as it was.
    let after = get_workflow(&router, "StoredWorkflow").await;
    assert_eq!(
        after, before,
        "case a refused update: the stored workflow must be unchanged"
    );
}

#[tokio::test]
async fn updated_defaults_reach_the_runs_started_after_the_write() {
    let router = emulator();
    register_job(&router).await;

    // "A collection of properties to be used as part of each execution of the workflow":
    // the collection the update leaves is what the runs STARTED AFTER it begin from, and a
    // run already begun holds the properties it began with.
    let workflow = "defaults-reaching-runs";
    register_workflow(
        &router,
        workflow,
        &json!({ "stage": "extract", "retries": "0" }),
    )
    .await;
    let before_the_write = start_run(&router, workflow, &Value::Null).await;

    let case = "a property collection replacing the one runs began from";
    update_workflow(
        &router,
        case,
        workflow,
        &json!({ "DefaultRunProperties": { "stage": "load" } }),
    )
    .await;

    let after_the_write = start_run(&router, workflow, &Value::Null).await;
    let started_after = get_properties(&router, workflow, &after_the_write).await;
    assert_eq!(
        started_after,
        json!({ "stage": "load" }),
        "case {case:?}: a run started after the write begins from the collection the update \
         left, got {started_after}"
    );

    let started_before = get_properties(&router, workflow, &before_the_write).await;
    assert_eq!(
        started_before,
        json!({ "stage": "extract", "retries": "0" }),
        "case {case:?}: a run already begun holds the properties it began with, got \
         {started_before}"
    );
}

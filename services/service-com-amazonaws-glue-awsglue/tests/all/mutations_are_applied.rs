// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rule every Glue mutation obeys: a call the service ACCEPTS is
//! applied to the one record the reads consult, so the read that follows observes it.
//!
//! RULE: a successful delete removes the whole record the name it addressed stood for, so
//! every later read of that name misses — 400 with the read's modelled
//! `EntityNotFoundException` — and the name is free to create again; a successful update
//! writes the members it carried over that record, so the next read reports the values it
//! supplied and not the ones the create wrote. A delete the model gives no entity-miss
//! error is served for a name holding nothing instead of being refused, and leaves the
//! namespace as it found it.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: WHICH resource is mutated (a
//! trigger, a workflow, a blueprint, a data quality ruleset, a partition — one namespace
//! per Region, one per account, and one inside the Data Catalog's tree), which optional
//! members the record carried, the form of its name, which members an update supplies, and
//! the way a requested name misses.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::messages::{
    BLUEPRINT_NOT_FOUND, TRIGGER_NOT_FOUND, WORKFLOW_NOT_FOUND,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::partitions::{seed_table, stored, values_of};
use crate::wire::{call, call_in_region, emulator, error_code, error_message, ok, REGION};

/// The role the jobs a trigger acts on are created with.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-job-role";

/// The job every trigger here acts on: an action names a job of the Region the create is
/// served in, so each suite registers it first.
const JOB: &str = "mutation-target-job";

/// The ruleset text a data quality ruleset is created with.
const RULESET: &str = "Rules = [ ColumnExists \"id\" ]";

/// Register the job `JOB` through `region`'s endpoint, so a trigger's action may name it.
async fn given_job_in(router: &MultiServiceRouter, region: &str) {
    let (status, body) = call_in_region(
        router,
        region,
        "CreateJob",
        &json!({
            "Name": JOB,
            "Role": ROLE_ARN,
            "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {region}: CreateJob must answer 200, got {status} with {body}"
    );
}

/// One `CreateTrigger` body for an `ON_DEMAND` trigger acting on [`JOB`].
fn trigger_body(name: &str) -> Value {
    json!({
        "Name": name,
        // "True is not supported for ON_DEMAND triggers", so no row here depends on the
        // state a create left the trigger in.
        "Type": "ON_DEMAND",
        "Actions": [ { "JobName": JOB } ],
    })
}

/// One row of the delete table: a resource to create, the read that observes it, the
/// delete that removes it, and the wording the read's miss carries afterwards.
struct DeleteCase {
    case: &'static str,
    /// The call that writes the record, as `(operation, body)`.
    create: (&'static str, Value),
    /// The read that must serve it before the delete and miss after it.
    read: (&'static str, Value),
    /// The delete under test.
    delete: (&'static str, Value),
    /// The `Message` the read's refusal must carry once the record is gone.
    miss_message: &'static str,
    /// The member of the delete's answer that names what it deleted.
    deleted_name: &'static str,
}

/// The rows of the delete table, as data: one resource per namespace the rule holds over.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            case: "a trigger, in the Region's own namespace",
            create: ("CreateTrigger", trigger_body("doomed-trigger")),
            read: ("GetTrigger", json!({ "Name": "doomed-trigger" })),
            delete: ("DeleteTrigger", json!({ "Name": "doomed-trigger" })),
            miss_message: TRIGGER_NOT_FOUND,
            deleted_name: "doomed-trigger",
        },
        DeleteCase {
            case: "a trigger whose name carries dots, dashes and digits",
            create: ("CreateTrigger", trigger_body("team.data_lake-42")),
            read: ("GetTrigger", json!({ "Name": "team.data_lake-42" })),
            delete: ("DeleteTrigger", json!({ "Name": "team.data_lake-42" })),
            miss_message: TRIGGER_NOT_FOUND,
            deleted_name: "team.data_lake-42",
        },
        DeleteCase {
            case: "a workflow, in the account's namespace",
            create: ("CreateWorkflow", json!({ "Name": "doomed-workflow" })),
            read: ("GetWorkflow", json!({ "Name": "doomed-workflow" })),
            delete: ("DeleteWorkflow", json!({ "Name": "doomed-workflow" })),
            miss_message: WORKFLOW_NOT_FOUND,
            deleted_name: "doomed-workflow",
        },
        DeleteCase {
            case: "a workflow carrying a description, default run properties and tags",
            create: (
                "CreateWorkflow",
                json!({
                    "Name": "described-workflow",
                    "Description": "nightly ingest",
                    "DefaultRunProperties": { "window": "24h" },
                    "MaxConcurrentRuns": 2,
                    "Tags": { "team": "data" },
                }),
            ),
            read: (
                "GetWorkflow",
                json!({ "Name": "described-workflow", "IncludeGraph": true }),
            ),
            delete: ("DeleteWorkflow", json!({ "Name": "described-workflow" })),
            miss_message: WORKFLOW_NOT_FOUND,
            deleted_name: "described-workflow",
        },
        DeleteCase {
            case: "a blueprint, read by the same name it was registered under",
            create: (
                "CreateBlueprint",
                json!({
                    "Name": "doomed-blueprint",
                    "BlueprintLocation": "s3://blueprint-bucket/doomed.zip",
                }),
            ),
            read: ("GetBlueprint", json!({ "Name": "doomed-blueprint" })),
            delete: ("DeleteBlueprint", json!({ "Name": "doomed-blueprint" })),
            miss_message: BLUEPRINT_NOT_FOUND,
            deleted_name: "doomed-blueprint",
        },
        DeleteCase {
            case: "a data quality ruleset",
            create: (
                "CreateDataQualityRuleset",
                json!({ "Name": "doomed-ruleset", "Ruleset": RULESET }),
            ),
            read: ("GetDataQualityRuleset", json!({ "Name": "doomed-ruleset" })),
            delete: (
                "DeleteDataQualityRuleset",
                json!({ "Name": "doomed-ruleset" }),
            ),
            // The ruleset miss names the account whose namespace was searched and the name
            // the request sent, so the row builds it where it is asserted.
            miss_message: "",
            deleted_name: "",
        },
    ]
}

// ---------------------------------------------------------------------------
// Delete, then read
// ---------------------------------------------------------------------------

/// A delete the service accepts removes the record from the store the reads consult: the
/// read that served the resource before it refuses afterwards with its modelled
/// `EntityNotFoundException`, and the name the delete freed can be created again.
#[tokio::test]
async fn a_delete_is_observed_by_the_read_that_follows_it() {
    let router = emulator();
    given_job_in(&router, REGION).await;

    for row in delete_cases() {
        let case = row.case;
        let (create_operation, create_body) = &row.create;
        let (read_operation, read_body) = &row.read;
        let (delete_operation, delete_body) = &row.delete;

        ok(&router, create_operation, create_body).await;
        // The read serves the resource before the delete, so what the delete changes is
        // observable and not a resource that was never there.
        ok(&router, read_operation, read_body).await;

        let answer = ok(&router, delete_operation, delete_body).await;
        if !row.deleted_name.is_empty() {
            assert_eq!(
                answer["Name"].as_str(),
                Some(row.deleted_name),
                "case {case}: {delete_operation} must report the name it deleted, got {answer}"
            );
        }

        let (status, body) = call(&router, read_operation, read_body).await;
        assert_eq!(
            status, 400,
            "case {case}: {read_operation} must refuse after the delete, got {status} with \
             {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException after the delete, got {body}"
        );
        let expected = if row.miss_message.is_empty() {
            // The one wording this surface gives a ruleset miss, which names the account
            // and the requested name.
            format!(
                "Cannot find Data Quality Ruleset in account {} with name doomed-ruleset",
                crate::wire::ACCOUNT_ID
            )
        } else {
            row.miss_message.to_string()
        };
        assert_eq!(
            error_message(&body),
            expected,
            "case {case}: expected the miss wording {expected:?}, got {body}"
        );

        // The name the delete freed is free: the create under it succeeds again and the
        // read serves what that second create wrote.
        ok(&router, create_operation, create_body).await;
        ok(&router, read_operation, read_body).await;
    }
}

/// A delete reaches only the namespace the request was served in: a trigger lives in the
/// Region that created it, so deleting one Region's trigger leaves another Region's trigger
/// of the same name exactly where it is.
#[tokio::test]
async fn deleting_a_trigger_leaves_another_regions_trigger_of_that_name() {
    let router = emulator();
    let other_region = "eu-west-1";
    for region in [REGION, other_region] {
        given_job_in(&router, region).await;
        let (status, body) =
            call_in_region(&router, region, "CreateTrigger", &trigger_body("shared")).await;
        assert_eq!(
            status, 200,
            "case {region}: CreateTrigger must answer 200, got {status} with {body}"
        );
    }

    let (status, body) =
        call_in_region(&router, REGION, "DeleteTrigger", &json!({ "Name": "shared" })).await;
    assert_eq!(
        status, 200,
        "case the addressed Region: DeleteTrigger must answer 200, got {status} with {body}"
    );

    let (status, body) =
        call_in_region(&router, REGION, "GetTrigger", &json!({ "Name": "shared" })).await;
    assert_eq!(
        status, 400,
        "case the addressed Region: the trigger must be gone, got {status} with {body}"
    );

    let (status, body) = call_in_region(
        &router,
        other_region,
        "GetTrigger",
        &json!({ "Name": "shared" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the untouched Region: its own trigger must survive, got {status} with {body}"
    );
    assert_eq!(
        body["Trigger"]["Name"].as_str(),
        Some("shared"),
        "case the untouched Region: the surviving trigger reports its name, got {body}"
    );
}

// ---------------------------------------------------------------------------
// A name the store holds nothing under
// ---------------------------------------------------------------------------

/// `DeleteTrigger` and `DeleteWorkflow` model no entity-miss failure — "Deletes a specified
/// trigger. If the trigger is not found, no exception is thrown" — so a name the store
/// holds nothing under is served, answers with the name it was addressed by, and leaves the
/// namespace exactly as it found it, which makes the delete idempotent and makes a
/// differently-cased or partial name a miss rather than a match.
#[tokio::test]
async fn deleting_a_name_that_holds_nothing_is_served_and_changes_nothing() {
    let router = emulator();
    given_job_in(&router, REGION).await;
    ok(&router, "CreateTrigger", &trigger_body("StoredTrigger")).await;
    ok(&router, "CreateWorkflow", &json!({ "Name": "StoredWorkflow" })).await;
    // One of each that IS deleted, so deleting it a second time is a miss too.
    ok(&router, "CreateTrigger", &trigger_body("gone-trigger")).await;
    ok(&router, "CreateWorkflow", &json!({ "Name": "gone-workflow" })).await;
    ok(&router, "DeleteTrigger", &json!({ "Name": "gone-trigger" })).await;
    ok(&router, "DeleteWorkflow", &json!({ "Name": "gone-workflow" })).await;

    // `Name` is "len 1..255" on `NameString` for both deletes
    // (`smithy_query operation=DeleteTrigger,DeleteWorkflow`), so a row sits on that bound.
    let longest = "n".repeat(255);
    let misses: Vec<(&str, String)> = vec![
        ("never created", "no-such-name".to_string()),
        ("already deleted, so deleted twice", "gone-trigger".to_string()),
        ("a prefix of a stored name", "Stored".to_string()),
        ("a suffix of a stored name", "Trigger".to_string()),
        ("a stored name lowercased", "storedtrigger".to_string()),
        ("a stored name uppercased", "STOREDWORKFLOW".to_string()),
        ("the shortest name the model accepts", "x".to_string()),
        ("a name at the longest the model accepts", longest),
    ];

    for (case, name) in &misses {
        for operation in ["DeleteTrigger", "DeleteWorkflow"] {
            let answer = ok(&router, operation, &json!({ "Name": name })).await;
            assert_eq!(
                answer["Name"].as_str(),
                Some(name.as_str()),
                "case {case} / {operation}: a delete of {name:?} must be served and report \
                 that name, got {answer}"
            );
        }
        // Nothing else went with it: both stored records are still readable.
        let trigger = ok(&router, "GetTrigger", &json!({ "Name": "StoredTrigger" })).await;
        assert_eq!(
            trigger["Trigger"]["Name"].as_str(),
            Some("StoredTrigger"),
            "case {case}: the stored trigger must be untouched, got {trigger}"
        );
        let workflow = ok(&router, "GetWorkflow", &json!({ "Name": "StoredWorkflow" })).await;
        assert_eq!(
            workflow["Workflow"]["Name"].as_str(),
            Some("StoredWorkflow"),
            "case {case}: the stored workflow must be untouched, got {workflow}"
        );
    }

    // The stored names themselves delete, so the misses above are misses and not a broken
    // lookup.
    for (operation, name) in [
        ("DeleteTrigger", "StoredTrigger"),
        ("DeleteWorkflow", "StoredWorkflow"),
    ] {
        ok(&router, operation, &json!({ "Name": name })).await;
    }
    for (operation, name) in [
        ("GetTrigger", "StoredTrigger"),
        ("GetWorkflow", "StoredWorkflow"),
    ] {
        let (status, body) = call(&router, operation, &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case the stored name / {operation}: {name} must be gone, got {status} with {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Update, then read
// ---------------------------------------------------------------------------

/// One row of the ruleset update table: the members the update supplies.
struct RulesetUpdate {
    case: &'static str,
    description: Option<&'static str>,
    ruleset: Option<&'static str>,
}

/// An update of a data quality ruleset writes the members it carried over the stored
/// record, so the read that follows reports the NEW description and the NEW DQDL text while
/// the ruleset keeps the name and the identifier the create assigned it.
#[tokio::test]
async fn a_ruleset_update_is_read_back_from_the_store() {
    let rows = [
        RulesetUpdate {
            case: "a description where the create carried none",
            description: Some("sim2real updated"),
            ruleset: None,
        },
        RulesetUpdate {
            case: "new rules",
            description: None,
            ruleset: Some("Rules = [ ColumnExists \"id\", IsComplete \"id\" ]"),
        },
        RulesetUpdate {
            case: "a description and new rules together",
            description: Some("checks id and amount"),
            ruleset: Some("Rules = [ ColumnExists \"amount\", RowCount > 0 ]"),
        },
        RulesetUpdate {
            case: "a multi-line ruleset",
            description: Some("multi-line"),
            ruleset: Some("Rules = [\n    ColumnExists \"a\",\n    Completeness \"a\" > 0.95\n]"),
        },
        RulesetUpdate {
            // `DescriptionString` is "len 0..2048", so the empty string is a value a client
            // may send (`smithy_query operation=UpdateDataQualityRuleset`).
            case: "an empty description",
            description: Some(""),
            ruleset: None,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let router = emulator();
        let case = row.case;
        let name = format!("ruleset-{index}");
        ok(
            &router,
            "CreateDataQualityRuleset",
            &json!({ "Name": &name, "Ruleset": RULESET }),
        )
        .await;
        let before = ok(
            &router,
            "GetDataQualityRuleset",
            &json!({ "Name": &name }),
        )
        .await;
        // The identity the create assigned the record, which an update does not move.
        let identifier = before["Identifier"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: the read must report an Identifier, got {before}"))
            .to_string();

        let mut update = json!({ "Name": &name });
        if let Some(description) = row.description {
            update["Description"] = json!(description);
        }
        if let Some(ruleset) = row.ruleset {
            update["Ruleset"] = json!(ruleset);
        }
        ok(&router, "UpdateDataQualityRuleset", &update).await;

        let after = ok(
            &router,
            "GetDataQualityRuleset",
            &json!({ "Name": &name }),
        )
        .await;
        let expected_ruleset = row.ruleset.unwrap_or(RULESET);
        assert_eq!(
            after["Ruleset"].as_str(),
            Some(expected_ruleset),
            "case {case}: the read must report the rules the update wrote, got {after}"
        );
        let expected_description = row.description.or_else(|| before["Description"].as_str());
        assert_eq!(
            after["Description"].as_str(),
            expected_description,
            "case {case}: the read must report the description the update wrote, got {after}"
        );
        assert_eq!(
            after["Name"].as_str(),
            Some(name.as_str()),
            "case {case}: an update does not move the ruleset's name, got {after}"
        );
        assert_eq!(
            after["Identifier"].as_str(),
            Some(identifier.as_str()),
            "case {case}: an update does not move the identifier the create assigned, got \
             {after}"
        );
        assert_eq!(
            after["CreatedOn"], before["CreatedOn"],
            "case {case}: an update does not move the instant the create wrote, got {after}"
        );
    }
}

/// One row of the partition update table: the definition the create stores and the one the
/// update writes over it, plus the column list the read must report afterwards.
struct PartitionUpdate {
    case: &'static str,
    created: Value,
    updated: Value,
    /// The `StorageDescriptor.Columns` the read must report after the update: `[]` when the
    /// submitted descriptor named no columns, since a column list the catalog holds none of
    /// is EMPTY rather than unknown.
    expected_columns: Value,
}

/// An update of a partition writes the submitted definition over the stored one, so the
/// read that follows reports the relocated `StorageDescriptor.Location`, the parameters the
/// update supplied, and the column list the submitted descriptor named — the empty list
/// when it named none.
#[tokio::test]
async fn a_partition_update_is_read_back_from_the_store() {
    let rows = [
        PartitionUpdate {
            case: "a relocated partition whose descriptor names no columns",
            created: json!({
                "Values": ["2024-01-01"],
                "StorageDescriptor": { "Location": "s3://bucket/events/dt=2024-01-01/" },
            }),
            updated: json!({
                "Values": ["2024-01-01"],
                "StorageDescriptor": {
                    "Location": "s3://bucket/events/dt=2024-01-01-updated/",
                },
                "Parameters": { "sim2real": "updated" },
            }),
            expected_columns: json!([]),
        },
        PartitionUpdate {
            case: "a relocated partition whose descriptor names columns",
            created: json!({
                "Values": ["2024-01-02"],
                "StorageDescriptor": {
                    "Columns": [ { "Name": "id", "Type": "string" } ],
                    "Location": "s3://bucket/events/dt=2024-01-02/",
                },
            }),
            updated: json!({
                "Values": ["2024-01-02"],
                "StorageDescriptor": {
                    "Columns": [
                        { "Name": "id", "Type": "string" },
                        { "Name": "amount", "Type": "double", "Comment": "cents" },
                    ],
                    "Location": "s3://bucket/events/relocated/dt=2024-01-02/",
                },
                "Parameters": { "owner": "ingest", "retention": "30d" },
            }),
            expected_columns: json!([
                { "Name": "id", "Type": "string" },
                { "Name": "amount", "Type": "double", "Comment": "cents" },
            ]),
        },
        PartitionUpdate {
            case: "a bare partition given a described location",
            created: json!({ "Values": ["2024-01-03"] }),
            updated: json!({
                "Values": ["2024-01-03"],
                "StorageDescriptor": {
                    "Location": "s3://bucket/events/dt=2024-01-03/",
                    "InputFormat": "org.apache.hadoop.mapred.TextInputFormat",
                },
            }),
            expected_columns: json!([]),
        },
    ];

    let router = emulator();
    seed_table(&router, "mutation_db", "events", &["dt"]).await;

    for row in &rows {
        let case = row.case;
        let values = values_of(&row.created);
        ok(
            &router,
            "CreatePartition",
            &json!({
                "DatabaseName": "mutation_db",
                "TableName": "events",
                "PartitionInput": &row.created,
            }),
        )
        .await;

        ok(
            &router,
            "UpdatePartition",
            &json!({
                "DatabaseName": "mutation_db",
                "TableName": "events",
                "PartitionValueList": &values,
                "PartitionInput": &row.updated,
            }),
        )
        .await;

        let partition = stored(&router, "mutation_db", "events", &values).await;
        assert_eq!(
            partition["StorageDescriptor"]["Location"],
            row.updated["StorageDescriptor"]["Location"],
            "case {case}: the read must report the location the update wrote, got {partition}"
        );
        assert_eq!(
            partition["Parameters"],
            row.updated
                .get("Parameters")
                .cloned()
                .unwrap_or(Value::Null),
            "case {case}: the read must report the parameters the update wrote, got {partition}"
        );
        assert_eq!(
            partition["StorageDescriptor"]["Columns"], row.expected_columns,
            "case {case}: expected Columns {}, got {}",
            row.expected_columns, partition["StorageDescriptor"]["Columns"]
        );
    }
}

/// The catalog reports a stored `StorageDescriptor`'s column list whether or not the
/// definition named any columns, on a TABLE as on a partition: the two reads project a
/// descriptor through the same projection, so neither can report a column list the other
/// omits.
#[tokio::test]
async fn a_table_reports_an_empty_column_list() {
    let router = emulator();
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": "descriptor_db" } }),
    )
    .await;

    for (index, (case, descriptor, expected)) in [
        (
            "a descriptor naming no columns",
            json!({ "Location": "s3://bucket/no_columns/" }),
            json!([]),
        ),
        (
            "a descriptor with an explicitly empty column list",
            json!({ "Columns": [], "Location": "s3://bucket/empty_columns/" }),
            json!([]),
        ),
        (
            "a descriptor naming one column",
            json!({
                "Columns": [ { "Name": "id", "Type": "string" } ],
                "Location": "s3://bucket/one_column/",
            }),
            json!([ { "Name": "id", "Type": "string" } ]),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let table = format!("descriptor_table_{index}");
        ok(
            &router,
            "CreateTable",
            &json!({
                "DatabaseName": "descriptor_db",
                "TableInput": { "Name": &table, "StorageDescriptor": descriptor },
            }),
        )
        .await;
        let read = ok(
            &router,
            "GetTable",
            &json!({ "DatabaseName": "descriptor_db", "Name": &table }),
        )
        .await;
        assert_eq!(
            read["Table"]["StorageDescriptor"]["Columns"], expected,
            "case {case}: expected Columns {expected}, got {}",
            read["Table"]["StorageDescriptor"]["Columns"]
        );
    }
}

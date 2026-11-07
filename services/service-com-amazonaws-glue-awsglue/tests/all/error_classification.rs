// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how this service CLASSIFIES a refusal: which modelled error a bad
//! request is answered with, and how that error's `Message` is worded.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a request the service will not accept is refused with its
//! VALIDATION error (`InvalidInputException`), decided from the request alone and so before
//! any resource it names is resolved — a definition that breaks a rule of its own shape, or
//! an operation asked for a resource whose kind cannot carry it, is never reported as a
//! missing entity — while a reference that resolves to nothing is the NOT-FOUND error; and
//! each refusal's `Message` is the wording the service words that error class with: the
//! fixed sentence of the class, the request's own values where the sentence carries any, and
//! the envelope the surface puts a refusal on the wire in — the diagnostic clause the
//! service framework appends (whose request id is its one per-run field), or the JSON
//! document the business-catalog surface wraps its sentence in.
//!
//! The suites are tables whose rows differ along what the rule generalises over: where the
//! refused value sits in the request, whether the entities it names exist, which member or
//! optimizer type is in play, which operation of a surface asks, and how a reference misses.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID};

/// Every value of the modelled `TableOptimizerType`, which `Type` is one of. Source:
/// `smithy_query shape=TableOptimizerType`.
const OPTIMIZER_TYPES: [&str; 3] = ["compaction", "retention", "orphan_file_deletion"];

/// The role an optimizer configuration passes to the service.
const OPTIMIZER_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/glue-table-optimizer";

/// The role every job registered here is created with.
const JOB_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-job-role";

// ---------------------------------------------------------------------------
// The diagnostic clause a refusal carries
// ---------------------------------------------------------------------------

/// Strip the diagnostic clause `message` must END with, asserting it states `service`,
/// `status` and `code` and carries a well-formed request id, and answer the text in front
/// of it.
///
/// The clause is
/// ` (Service: <service>; Status Code: <status>; Error Code: <code>; Request ID: <id>; Proxy: null)`.
/// Its request id is the one per-run field, so it is asserted by SHAPE rather than by value;
/// everything else is fixed text. A refusal that wraps another service's refusal carries two
/// clauses, so this strips the outermost one and can be applied again to what it returns.
pub(crate) fn without_diagnostic(
    case: &str,
    message: &str,
    service: &str,
    status: u16,
    code: &str,
) -> String {
    let opening =
        format!(" (Service: {service}; Status Code: {status}; Error Code: {code}; Request ID: ");
    let closing = "; Proxy: null)";
    let at = message.rfind(&opening).unwrap_or_else(|| {
        panic!("case {case}: expected the clause {opening:?} in Message {message:?}")
    });
    assert!(
        message.ends_with(closing),
        "case {case}: expected Message to end with {closing:?}, got {message:?}"
    );
    let request_id = &message[at + opening.len()..message.len() - closing.len()];
    assert!(
        is_request_id(request_id),
        "case {case}: the clause must carry a request id, got {request_id:?} in {message:?}"
    );
    message[..at].to_string()
}

/// Whether `id` has the 8-4-4-4-12 lowercase-hex form every AWS request id carries.
fn is_request_id(id: &str) -> bool {
    let mut groups = id.split('-');
    for width in [8usize, 4, 4, 4, 12] {
        let Some(group) = groups.next() else {
            return false;
        };
        let hex = group.len() == width
            && group
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
        if !hex {
            return false;
        }
    }
    groups.next().is_none()
}

/// Make one call that must be refused, answering its body: HTTP 400 with `expected_code`.
async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
    expected_code: &str,
) -> Value {
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        status, 400,
        "case {case}: expected {operation} to answer 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains(expected_code),
        "case {case}: expected {expected_code} from {operation}, got {body}"
    );
    body
}

// ---------------------------------------------------------------------------
// A conditional trigger naming one job twice
// ---------------------------------------------------------------------------

/// Register `jobs` so a trigger's actions and conditions resolve.
async fn register_jobs(router: &MultiServiceRouter, jobs: &[&str]) {
    for job in jobs {
        ok(
            router,
            "CreateJob",
            &json!({
                "Name": job,
                "Role": JOB_ROLE_ARN,
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
            }),
        )
        .await;
    }
}

/// A `Predicate` whose conditions wait on each `(job, state)` pair, plus an optional
/// `Logical`.
fn predicate(logical: Option<&str>, jobs: &[&str]) -> Value {
    let conditions: Vec<Value> = jobs
        .iter()
        .map(|job| json!({ "LogicalOperator": "EQUALS", "JobName": job, "State": "SUCCEEDED" }))
        .collect();
    let mut value = json!({ "Conditions": conditions });
    if let Some(logical) = logical {
        value["Logical"] = json!(logical);
    }
    value
}

/// An `Actions` list running each of `jobs`.
fn actions(jobs: &[&str]) -> Value {
    Value::Array(jobs.iter().map(|job| json!({ "JobName": job })).collect())
}

/// A CONDITIONAL trigger whose `Predicate` waits on a job one of its `Actions` also runs is
/// a VALIDATION failure: `InvalidInputException` with the fixed sentence the service words
/// that rule with, and nothing is stored — and that holds whether or not the doubled job
/// exists, because the definition is judged before its targets are resolved.
///
/// The rows differ along where the overlap sits (the only action, the second of two, one of
/// several conditions), how many conditions the predicate lists, whether the doubled job is
/// a job this account holds, and — for the accepted rows — along the ways a conditional
/// definition can name jobs WITHOUT doubling one up.
#[tokio::test]
async fn a_conditional_trigger_may_not_name_one_job_in_its_predicate_and_its_actions() {
    let router = emulator();
    register_jobs(&router, &["job-one", "job-two", "job-three"]).await;
    ok(
        &router,
        "CreateCrawler",
        &json!({
            "Name": "classification-crawler",
            "Role": "arn:aws:iam::000000000000:role/sim2real-crawler-role",
            "Targets": { "S3Targets": [{ "Path": "s3://sim2real-glue-crawler/data/" }] },
        }),
    )
    .await;

    // (case, the Predicate, the Actions)
    let refusals: Vec<(&str, Value, Value)> = vec![
        (
            "the one condition names the one action's job",
            predicate(None, &["job-one"]),
            actions(&["job-one"]),
        ),
        (
            "the condition names the SECOND of two action jobs",
            predicate(None, &["job-two"]),
            actions(&["job-one", "job-two"]),
        ),
        (
            "one of three conditions names the one action's job",
            predicate(Some("AND"), &["job-two", "job-three", "job-one"]),
            actions(&["job-one"]),
        ),
        (
            "every condition names an action job",
            predicate(Some("ANY"), &["job-one", "job-two"]),
            actions(&["job-one", "job-two"]),
        ),
        (
            // The definition is refused on its own terms, so a job NOTHING created is this
            // validation failure and not the job miss a trigger's targets are resolved
            // with.
            "the doubled job is one this account does not hold",
            predicate(None, &["never-created-job"]),
            actions(&["never-created-job"]),
        ),
        (
            "a doubled absent job beside a stored one",
            predicate(Some("AND"), &["job-one", "never-created-job"]),
            actions(&["never-created-job"]),
        ),
    ];

    for (index, (case, conditions, runs)) in refusals.iter().enumerate() {
        let name = format!("doubled-{index}");
        let body = refused(
            &router,
            case,
            "CreateTrigger",
            &json!({
                "Name": name,
                "Type": "CONDITIONAL",
                "Predicate": conditions,
                "Actions": runs,
            }),
            "InvalidInputException",
        )
        .await;
        assert_eq!(
            error_message(&body),
            "Job cannot be a Predicate and Action for Conditional Trigger.",
            "case {case}: unexpected refusal wording, got {body}"
        );
        // A refused create stores nothing.
        let (status, read) = call(&router, "GetTrigger", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: the refused trigger must not be stored, got {status} with {read}"
        );
    }

    // The same shape WITHOUT one job in both places is accepted, so each refusal above is
    // about the doubling and not about the row's shape.
    let accepted: Vec<(&str, Value, Value)> = vec![
        (
            "disjoint condition and action jobs",
            predicate(None, &["job-one"]),
            actions(&["job-two"]),
        ),
        (
            "two conditions, neither naming an action job",
            predicate(Some("AND"), &["job-one", "job-two"]),
            actions(&["job-three"]),
        ),
        (
            "the predicate waits on a crawler and the action runs a job",
            json!({
                "Conditions": [{
                    "LogicalOperator": "EQUALS",
                    "CrawlerName": "classification-crawler",
                    "CrawlState": "SUCCEEDED",
                }],
            }),
            actions(&["job-one"]),
        ),
        (
            "the predicate waits on a job and the action runs a crawler",
            predicate(None, &["job-one"]),
            json!([{ "CrawlerName": "classification-crawler" }]),
        ),
    ];

    for (index, (case, conditions, runs)) in accepted.iter().enumerate() {
        let name = format!("disjoint-{index}");
        let created = ok(
            &router,
            "CreateTrigger",
            &json!({
                "Name": name,
                "Type": "CONDITIONAL",
                "Predicate": conditions,
                "Actions": runs,
            }),
        )
        .await;
        assert_eq!(
            created.get("Name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case}: the create must answer with the trigger's name, got {created}"
        );
    }
}

// ---------------------------------------------------------------------------
// The member a trigger's type requires
// ---------------------------------------------------------------------------

/// A trigger definition missing a member its `Type` requires is refused with
/// `InvalidInputException` whose `Message` names THAT MEMBER and says that neither no value
/// nor an empty one will do — one sentence for every such member, carrying nothing else of
/// the request.
///
/// The rows differ along which member is missing (`Actions`, `Schedule`, `Predicate`), how
/// it is missing (absent, or present but empty), and which operation submits the definition
/// (the create that stores one, the update that overwrites one).
#[tokio::test]
async fn a_missing_definition_member_is_refused_by_naming_that_member() {
    let router = emulator();
    register_jobs(&router, &["member-job", "member-predicate-job"]).await;
    let runs = actions(&["member-job"]);

    // (case, the Type, the definition minus Name, the member the refusal must name, whether
    // the CREATE can submit this definition at all — `CreateTriggerRequest$Actions` is
    // @required, so an absent `Actions` is refused by the modelled constraint before this
    // rule is reached, while `TriggerUpdate$Actions` is optional)
    let rows: Vec<(&str, &str, Value, &str, bool)> = vec![
        (
            "no Actions member at all",
            "ON_DEMAND",
            json!({}),
            "Actions",
            false,
        ),
        (
            "an empty Actions list",
            "ON_DEMAND",
            json!({ "Actions": [] }),
            "Actions",
            true,
        ),
        (
            "a scheduled trigger with no Schedule",
            "SCHEDULED",
            json!({ "Actions": runs }),
            "Schedule",
            true,
        ),
        (
            "a conditional trigger with no Predicate",
            "CONDITIONAL",
            json!({ "Actions": runs }),
            "Predicate",
            true,
        ),
        (
            "a conditional trigger whose Predicate lists no condition",
            "CONDITIONAL",
            json!({ "Predicate": { "Conditions": [] }, "Actions": runs }),
            "Predicate",
            true,
        ),
        (
            "a conditional trigger whose Predicate carries a Logical and no condition",
            "CONDITIONAL",
            json!({ "Predicate": { "Logical": "AND", "Conditions": [] }, "Actions": runs }),
            "Predicate",
            true,
        ),
    ];

    for (index, (case, trigger_type, definition, member, on_create)) in rows.iter().enumerate() {
        let expected = format!("{member} cannot be null or empty");

        if *on_create {
            let name = format!("missing-member-{index}");
            let mut create = definition.clone();
            create["Name"] = json!(name);
            create["Type"] = json!(trigger_type);
            let body = refused(
                &router,
                case,
                "CreateTrigger",
                &create,
                "InvalidInputException",
            )
            .await;
            assert_eq!(
                error_message(&body),
                expected,
                "case {case} via CreateTrigger: unexpected refusal wording, got {body}"
            );
        }

        // The UPDATE that would overwrite a stored definition with the same one is refused
        // by the same sentence: it is the definition that is judged, not the operation.
        let stored = format!("updated-{index}");
        ok(
            &router,
            "CreateTrigger",
            &json!({
                "Name": stored,
                "Type": trigger_type,
                "Actions": actions(&["member-job"]),
                "Schedule": "cron(15 12 * * ? *)",
                "Predicate": predicate(None, &["member-predicate-job"]),
            }),
        )
        .await;
        let body = refused(
            &router,
            case,
            "UpdateTrigger",
            &json!({ "Name": stored, "TriggerUpdate": definition }),
            "InvalidInputException",
        )
        .await;
        assert_eq!(
            error_message(&body),
            expected,
            "case {case} via UpdateTrigger: unexpected refusal wording, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// A table whose type cannot be optimized
// ---------------------------------------------------------------------------

/// Create the database every table below lives in.
async fn given_database(router: &MultiServiceRouter, database: &str) {
    ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": database } }),
    )
    .await;
}

/// Create `table` in `database` from the `CreateTable` body `extra` adds to.
async fn given_table(router: &MultiServiceRouter, database: &str, table: &str, extra: Value) {
    let mut request = json!({
        "DatabaseName": database,
        "TableInput": {
            "Name": table,
            "StorageDescriptor": { "Location": format!("s3://lake/{table}/") }
        }
    });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            if member == "TableInput" {
                for (inner, value) in value.as_object().expect("TableInput is a structure") {
                    request["TableInput"][inner] = value.clone();
                }
            } else {
                request[member] = value.clone();
            }
        }
    }
    ok(router, "CreateTable", &request).await;
}

/// One `GetTableOptimizer`/`DeleteTableOptimizer` address.
fn optimizer_address(database: &str, table: &str, optimizer_type: &str) -> Value {
    json!({
        "CatalogId": ACCOUNT_ID,
        "DatabaseName": database,
        "TableName": table,
        "Type": optimizer_type
    })
}

/// The same address plus the configuration a create submits.
fn optimizer_create(database: &str, table: &str, optimizer_type: &str) -> Value {
    let mut request = optimizer_address(database, table, optimizer_type);
    request["TableOptimizerConfiguration"] =
        json!({ "roleArn": OPTIMIZER_ROLE_ARN, "enabled": true });
    request
}

/// A table optimizer works on the data files of an OPEN-TABLE-FORMAT table, so a request
/// naming a stored table of any other kind is a validation failure — `InvalidInputException`
/// with the fixed sentence the service words that check with — whatever the optimizer type
/// asked for and whichever operation of the surface asks; a table that IS one is addressed
/// normally, and a type it holds no optimizer of is the not-found refusal instead.
///
/// REGISTERING an optimizer reads the table's current metadata file on top of that, so a
/// table that declares the open table format while carrying no metadata filepath is served
/// by the read and refused by the create, each with its own sentence.
///
/// The rows differ along how the table declares its kind (no parameters at all, a parameter
/// naming another format, parameters without the marker, a metadata filepath under another
/// format, the Hive table type, the marker in either case, the marker written by the create
/// that mints the metadata) and along the optimizer type — every value of the modelled enum.
#[tokio::test]
async fn an_optimizer_of_a_table_that_cannot_be_optimized_is_a_validation_failure() {
    let router = emulator();
    given_database(&router, "optimizer_classification").await;

    /// One table the rows address an optimizer of.
    struct TableRow {
        /// What this row varies, named in every assertion it drives.
        case: &'static str,
        /// The table's name, which is also the one the refusals must word a miss with.
        table: &'static str,
        /// Whether the table's declared kind can carry an optimizer at all.
        optimizable: bool,
        /// Whether the table carries the metadata filepath a CREATE reads.
        carries_metadata: bool,
        /// What this row's `CreateTable` body adds to the shared one.
        extra: Value,
    }

    let tables: Vec<TableRow> = vec![
        TableRow {
            case: "a table with no parameters at all",
            table: "plain_table",
            optimizable: false,
            carries_metadata: false,
            extra: json!({}),
        },
        TableRow {
            case: "a table whose parameters name another format",
            table: "hive_table",
            optimizable: false,
            carries_metadata: false,
            extra: json!({ "TableInput": { "Parameters": { "table_type": "HIVE" } } }),
        },
        TableRow {
            case: "a table carrying parameters but no format marker",
            table: "unmarked_table",
            optimizable: false,
            carries_metadata: false,
            extra: json!({ "TableInput": { "Parameters": { "classification": "parquet" } } }),
        },
        TableRow {
            // A metadata filepath does not make a table one an optimizer can belong to:
            // the create has to refuse this one as the read and the delete do, or it would
            // store an optimizer no operation could address.
            case: "a table naming a metadata filepath while declaring another format",
            table: "hive_table_with_metadata",
            optimizable: false,
            carries_metadata: true,
            extra: json!({
                "TableInput": {
                    "Parameters": {
                        "table_type": "HIVE",
                        "metadata_location":
                            "s3://lake/hive_table_with_metadata/metadata/00000-a.metadata.json"
                    }
                }
            }),
        },
        TableRow {
            // `TableType` is the Hive table kind, not the open-table-format marker.
            case: "an external table",
            table: "external_table",
            optimizable: false,
            carries_metadata: false,
            extra: json!({ "TableInput": { "TableType": "EXTERNAL_TABLE" } }),
        },
        TableRow {
            case: "a table the create minted open-table-format metadata for",
            table: "iceberg_table",
            optimizable: true,
            carries_metadata: true,
            extra: json!({
                "OpenTableFormatInput": { "IcebergInput": { "MetadataOperation": "CREATE" } }
            }),
        },
        TableRow {
            // The marker is a table PARAMETER, so a table that carries it is one whatever
            // write put it there — but this one names no metadata file, so only the read
            // is served.
            case: "a table whose parameters carry the marker and no metadata filepath",
            table: "marked_table",
            optimizable: true,
            carries_metadata: false,
            extra: json!({ "TableInput": { "Parameters": { "table_type": "ICEBERG" } } }),
        },
        TableRow {
            case: "a table whose marker is in another case, with a metadata filepath",
            table: "lowercase_marked_table",
            optimizable: true,
            carries_metadata: true,
            extra: json!({
                "TableInput": {
                    "Parameters": {
                        "table_type": "iceberg",
                        "metadata_location":
                            "s3://lake/lowercase_marked_table/metadata/00000-a.metadata.json"
                    }
                }
            }),
        },
    ];

    for row in &tables {
        given_table(
            &router,
            "optimizer_classification",
            row.table,
            row.extra.clone(),
        )
        .await;
    }

    for row in &tables {
        for optimizer_type in OPTIMIZER_TYPES {
            let case = format!("{}, type {optimizer_type}", row.case);
            if !row.optimizable {
                // Every operation of the surface answers the one validation refusal, and
                // the create records nothing.
                for (operation, request) in [
                    (
                        "GetTableOptimizer",
                        optimizer_address("optimizer_classification", row.table, optimizer_type),
                    ),
                    (
                        "DeleteTableOptimizer",
                        optimizer_address("optimizer_classification", row.table, optimizer_type),
                    ),
                ] {
                    let label = format!("{case} via {operation}");
                    let body =
                        refused(&router, &label, operation, &request, "InvalidInputException").await;
                    assert_eq!(
                        without_diagnostic(
                            &label,
                            &error_message(&body),
                            "glue",
                            400,
                            "InvalidInputException",
                        ),
                        "Request Validation Failed: Unsupported Table type",
                        "case {label}: unexpected refusal wording, got {body}"
                    );
                }
            } else {
                // A table that CAN carry an optimizer and holds none of this type is the
                // not-found refusal, worded as a miss of the optimizer rather than of the
                // table.
                let label = format!("{case} before the create");
                let body = refused(
                    &router,
                    &label,
                    "GetTableOptimizer",
                    &optimizer_address("optimizer_classification", row.table, optimizer_type),
                    "EntityNotFoundException",
                )
                .await;
                assert_eq!(
                    without_diagnostic(
                        &label,
                        &error_message(&body),
                        "glue",
                        404,
                        "EntityNotFoundException",
                    ),
                    format!(
                        "No optimizer of type {optimizer_type} found for the table: {}",
                        row.table
                    ),
                    "case {label}: unexpected refusal wording, got {body}"
                );
            }

            // The CREATE reads the table's metadata file first, so a table that names none
            // is refused with the sentence that states exactly that — whether or not its
            // declared kind could carry an optimizer — and a table that names one while
            // declaring another kind is refused as the read and the delete refuse it.
            if !row.carries_metadata || !row.optimizable {
                let label = format!("{case} via CreateTableOptimizer");
                let expected = if row.carries_metadata {
                    "Request Validation Failed: Unsupported Table type"
                } else {
                    "Glue table metadata filepath is null"
                };
                let body = refused(
                    &router,
                    &label,
                    "CreateTableOptimizer",
                    &optimizer_create("optimizer_classification", row.table, optimizer_type),
                    "InvalidInputException",
                )
                .await;
                assert_eq!(
                    without_diagnostic(
                        &label,
                        &error_message(&body),
                        "glue",
                        400,
                        "InvalidInputException",
                    ),
                    expected,
                    "case {label}: unexpected refusal wording, got {body}"
                );
                continue;
            }

            // ... and the same address is served once the optimizer is registered, so the
            // refusal above was about the optimizer and not about the table.
            ok(
                &router,
                "CreateTableOptimizer",
                &optimizer_create("optimizer_classification", row.table, optimizer_type),
            )
            .await;
            let read = ok(
                &router,
                "GetTableOptimizer",
                &optimizer_address("optimizer_classification", row.table, optimizer_type),
            )
            .await;
            assert_eq!(
                read["TableOptimizer"]["type"].as_str(),
                Some(optimizer_type),
                "case {case}: the registered optimizer must be read back, got {read}"
            );

            // The delete of that optimizer is served; a second one is the same miss as
            // before the create, worded the DELETE's own way rather than the read's.
            ok(
                &router,
                "DeleteTableOptimizer",
                &optimizer_address("optimizer_classification", row.table, optimizer_type),
            )
            .await;
            let label = format!("{case} after the delete");
            let body = refused(
                &router,
                &label,
                "DeleteTableOptimizer",
                &optimizer_address("optimizer_classification", row.table, optimizer_type),
                "EntityNotFoundException",
            )
            .await;
            assert_eq!(
                without_diagnostic(
                    &label,
                    &error_message(&body),
                    "glue",
                    404,
                    "EntityNotFoundException",
                ),
                format!(
                    "An optimizer of type {optimizer_type} does not exist on table {}",
                    row.table
                ),
                "case {label}: unexpected refusal wording, got {body}"
            );
            let read_again = refused(
                &router,
                &label,
                "GetTableOptimizer",
                &optimizer_address("optimizer_classification", row.table, optimizer_type),
                "EntityNotFoundException",
            )
            .await;
            assert_ne!(
                error_message(&read_again),
                error_message(&body),
                "case {label}: the read and the delete word this miss differently, both \
                 answered {}",
                error_message(&body)
            );
        }
    }
}

/// A table-optimizer request whose address resolves to no table is the DATA CATALOG's own
/// read miss, surfaced wrapped in this service's diagnostic clause: the catalog's fixed
/// sentence with its own clause, inside the clause this service appends, and nothing is
/// stored.
///
/// The rows differ along which part of the address misses (the database, the table), how it
/// misses (never created, created and then deleted) and along the operation that asks.
#[tokio::test]
async fn a_table_optimizer_address_that_resolves_to_no_table_surfaces_the_catalog_miss() {
    let router = emulator();
    given_database(&router, "optimizer_misses").await;
    given_table(
        &router,
        "optimizer_misses",
        "dropped_table",
        json!({ "OpenTableFormatInput": { "IcebergInput": { "MetadataOperation": "CREATE" } } }),
    )
    .await;
    ok(
        &router,
        "DeleteTable",
        &json!({ "DatabaseName": "optimizer_misses", "Name": "dropped_table" }),
    )
    .await;

    // (case, the database, the table)
    let rows: [(&str, &str, &str); 4] = [
        ("a database nothing created", "no_such_db", "any_table"),
        (
            "a table the database does not hold",
            "optimizer_misses",
            "no_such_table",
        ),
        (
            "a table that was created and dropped",
            "optimizer_misses",
            "dropped_table",
        ),
        (
            "a table name at the modelled length bound",
            "optimizer_misses",
            "tttttttttttttttttttttttttttttttttttttttttttttttttt",
        ),
    ];

    for (case, database, table) in rows {
        for optimizer_type in OPTIMIZER_TYPES {
            for (operation, request) in [
                (
                    "CreateTableOptimizer",
                    optimizer_create(database, table, optimizer_type),
                ),
                (
                    "GetTableOptimizer",
                    optimizer_address(database, table, optimizer_type),
                ),
                (
                    "DeleteTableOptimizer",
                    optimizer_address(database, table, optimizer_type),
                ),
            ] {
                let label = format!("{case}, type {optimizer_type} via {operation}");
                let body = refused(
                    &router,
                    &label,
                    operation,
                    &request,
                    "EntityNotFoundException",
                )
                .await;
                let wrapped = without_diagnostic(
                    &label,
                    &error_message(&body),
                    "glue",
                    404,
                    "EntityNotFoundException",
                );
                assert_eq!(
                    without_diagnostic(
                        &label,
                        &wrapped,
                        "AmazonDataCatalog",
                        400,
                        "EntityNotFoundException",
                    ),
                    "Entity Not Found",
                    "case {label}: the catalog's own refusal must be surfaced, got {body}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A create under a name already taken
// ---------------------------------------------------------------------------

/// A trigger's name is the create's idempotency identifier, so a create under a name the
/// Region already holds a trigger under is told WHICH of the two it is: a retry of the
/// create that stored the trigger (`AlreadyExistsException`), or one request associating
/// that identifier with a second, different trigger
/// (`IdempotentParameterMismatchException`, whose sentence names the resource kind). Either
/// way the stored trigger is left exactly as it was.
///
/// The rows differ along WHICH member of the definition differs — including a member nested
/// inside an action, the tags, which are no part of the reported trigger, and
/// `StartOnCreation`, which the service reads into the trigger's state rather than storing.
#[tokio::test]
async fn a_duplicate_trigger_create_is_a_mismatch_only_when_the_definition_differs() {
    let router = emulator();
    register_jobs(&router, &["dup-job", "dup-other-job"]).await;
    for workflow in ["dup-workflow", "dup-other-workflow"] {
        ok(&router, "CreateWorkflow", &json!({ "Name": workflow })).await;
    }

    let stored = json!({
        "Name": "duplicate-trigger",
        "Type": "SCHEDULED",
        "Schedule": "cron(15 12 * * ? *)",
        "Description": "the stored trigger",
        "WorkflowName": "dup-workflow",
        "Tags": { "owner": "data-platform" },
        "Actions": [{ "JobName": "dup-job", "Timeout": 60 }],
    });
    ok(&router, "CreateTrigger", &stored).await;

    // The identical request is the retry of the create that stored it, however often it is
    // repeated.
    for attempt in ["the first retry", "a second retry"] {
        let body = refused(
            &router,
            attempt,
            "CreateTrigger",
            &stored,
            "AlreadyExistsException",
        )
        .await;
        assert_eq!(
            error_message(&body),
            "Trigger already exists.",
            "case {attempt}: unexpected duplicate wording, got {body}"
        );
    }

    // ... and it stays that retry after the trigger has been STARTED and STOPPED: what the
    // create asked for is the request's `StartOnCreation`, not the state the trigger is in
    // now, so a start does not turn an identical create into a different configuration.
    for operation in ["StartTrigger", "StopTrigger"] {
        ok(
            &router,
            operation,
            &json!({ "Name": "duplicate-trigger" }),
        )
        .await;
        let case = format!("an identical retry after {operation}");
        let body = refused(
            &router,
            &case,
            "CreateTrigger",
            &stored,
            "AlreadyExistsException",
        )
        .await;
        assert_eq!(
            error_message(&body),
            "Trigger already exists.",
            "case {case}: unexpected duplicate wording, got {body}"
        );
    }
    // The state those calls left behind is what the rows below compare against.
    let before = ok(
        &router,
        "GetTrigger",
        &json!({ "Name": "duplicate-trigger" }),
    )
    .await;

    // (case, the member to change, the value to change it to)
    let differing: Vec<(&str, &str, Value)> = vec![
        ("another description", "Description", json!("another")),
        (
            "another schedule",
            "Schedule",
            json!("cron(0 3 * * ? *)"),
        ),
        ("no description at all", "Description", Value::Null),
        (
            "another action job",
            "Actions",
            json!([{ "JobName": "dup-other-job", "Timeout": 60 }]),
        ),
        (
            "a member nested inside the action",
            "Actions",
            json!([{ "JobName": "dup-job", "Timeout": 30 }]),
        ),
        (
            "one action more",
            "Actions",
            json!([
                { "JobName": "dup-job", "Timeout": 60 },
                { "JobName": "dup-other-job" },
            ]),
        ),
        ("another workflow", "WorkflowName", json!("dup-other-workflow")),
        ("another tag value", "Tags", json!({ "owner": "someone-else" })),
        ("no tags at all", "Tags", Value::Null),
        ("started on creation", "StartOnCreation", json!(true)),
        (
            "a predicate the stored trigger has not",
            "Predicate",
            json!({
                "Conditions": [{
                    "LogicalOperator": "EQUALS",
                    "JobName": "dup-other-job",
                    "State": "SUCCEEDED",
                }],
            }),
        ),
    ];

    for (case, member, value) in &differing {
        let mut request = stored.clone();
        if value.is_null() {
            request
                .as_object_mut()
                .expect("the request is a structure")
                .remove(*member);
        } else {
            request[*member] = value.clone();
        }

        let body = refused(
            &router,
            case,
            "CreateTrigger",
            &request,
            "IdempotentParameterMismatchException",
        )
        .await;
        assert_eq!(
            error_message(&body),
            "Trigger already submitted with different configuration.",
            "case {case}: unexpected mismatch wording, got {body}"
        );

        // A refused create overwrites nothing: the stored trigger is the one the first
        // create left behind.
        let after = ok(
            &router,
            "GetTrigger",
            &json!({ "Name": "duplicate-trigger" }),
        )
        .await;
        assert_eq!(
            after, before,
            "case {case}: a refused create must leave the stored trigger alone; expected \
             {before}, got {after}"
        );
    }
}

// ---------------------------------------------------------------------------
// The business catalog's JSON-wrapped refusals
// ---------------------------------------------------------------------------

/// A `Forms` map sending every `(form name, form type identifier)` pair.
fn forms(entries: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (form_name, form_type_identifier) in entries {
        map.insert(
            (*form_name).to_string(),
            json!({ "FormTypeIdentifier": form_type_identifier }),
        );
    }
    Value::Object(map)
}

/// `GetAssetType` for an identifier this account holds no asset type under is
/// `EntityNotFoundException` whose `Message` is a JSON DOCUMENT carrying the sentence under
/// a `Message` key — the way every refusal of this surface is put on the wire, so a client
/// that parses `Message` as JSON reads the sentence.
///
/// The rows differ along how the identifier misses: never registered, the name of an asset
/// type that was deleted, the identifier the catalog minted for it, a recased name, and a
/// name that only prefixes a stored one.
#[tokio::test]
async fn the_asset_type_miss_is_a_json_document_naming_the_identifier() {
    let router = emulator();
    let form = ok(
        &router,
        "PutFormType",
        &json!({ "Name": "ClassForm", "Schema": "structure ClassForm { a: String }" }),
    )
    .await;
    let form_id = form["Id"].as_str().expect("PutFormType reports an Id");
    let entries = forms(&[("details", form_id)]);

    // An asset type that is kept, so each row below is about the identifier it names and
    // not about an empty namespace.
    ok(
        &router,
        "PutAssetType",
        &json!({ "Name": "KeptAssetType", "Forms": entries }),
    )
    .await;
    // One that is registered and then deleted, so a row names an address that WAS live.
    let gone = ok(
        &router,
        "PutAssetType",
        &json!({ "Name": "GoneAssetType", "Forms": entries }),
    )
    .await;
    let gone_id = gone["Id"]
        .as_str()
        .expect("PutAssetType reports an Id")
        .to_string();
    ok(
        &router,
        "DeleteAssetType",
        &json!({ "Identifier": "GoneAssetType" }),
    )
    .await;

    let rows: Vec<(&str, String)> = vec![
        ("never registered", "NoSuchAssetType".to_string()),
        ("a deleted asset type, by name", "GoneAssetType".to_string()),
        ("a deleted asset type, by its minted id", gone_id),
        ("a stored name recased", "KEPTASSETTYPE".to_string()),
        ("a prefix of a stored name", "KeptAssetTyp".to_string()),
        (
            "a stored name with one more character",
            "KeptAssetTypes".to_string(),
        ),
    ];

    for (case, identifier) in &rows {
        let body = refused(
            &router,
            case,
            "GetAssetType",
            &json!({ "Identifier": identifier }),
            "EntityNotFoundException",
        )
        .await;
        let message = error_message(&body);
        assert_eq!(
            message,
            format!(r#"{{"Message":"Asset type {identifier} not found."}}"#),
            "case {case}: unexpected refusal wording, got {body}"
        );
        // The wording is a document a client can parse, not a sentence that merely looks
        // like one.
        let parsed: Value = serde_json::from_str(&message)
            .unwrap_or_else(|error| panic!("case {case}: Message must be JSON ({error}): {message}"));
        assert_eq!(
            parsed["Message"].as_str(),
            Some(format!("Asset type {identifier} not found.").as_str()),
            "case {case}: the document must carry the sentence, got {parsed}"
        );
    }

    // The asset type the rows did not name is still readable, so none of those refusals
    // changed anything.
    let read = ok(
        &router,
        "GetAssetType",
        &json!({ "Identifier": "KeptAssetType" }),
    )
    .await;
    assert_eq!(
        read["Name"].as_str(),
        Some("KeptAssetType"),
        "case the stored asset type: a refused read must change nothing, got {read}"
    );
}

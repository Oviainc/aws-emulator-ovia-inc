// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the wording and the CLASSIFICATION of this service's
//! name-collision refusals: what a create answers when the name it asks for is already
//! taken, and what a delete answers for a classifier that is not there.
//!
//! The rule under test: a refusal's `Message` is the sentence the service words THAT
//! error class with for THAT operation and resource — its fixed text exactly, with only
//! the variable parts the service names (read off the request, or the account this
//! emulator serves) and composed ONCE, never a per-resource sentence concatenated with a
//! generic one; and where the service's surface puts its refusals inside a JSON document
//! (the business catalog), the `Message` carries that document. A create for a name
//! already taken on an operation that models `IdempotentParameterMismatchException`
//! ("the same unique identifier was associated with two different records") answers THAT
//! error when the definition submitted is not the stored one, and
//! `AlreadyExistsException` only when it is.
//!
//! Each suite is a table whose rows differ along what the rule generalises over — which
//! resource the collision is about, the name's value, casing, character set and length,
//! and (for the job) which member of the submitted definition differs from the stored
//! one. The expected strings are written out here as literals rather than read from the
//! service's own constants, so the test pins the wording instead of restating it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_messages::assert_refusal;
use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID};

/// A role ARN a job definition can carry.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-job-role";

/// A well-formed Avro schema definition, for the schema-registry row.
const AVRO_DEFINITION: &str =
    r#"{"type":"record","name":"Row","fields":[{"name":"id","type":"string"}]}"#;

/// Names that span the axis every template below generalises over: the recorded
/// fixtures' own shape, a single character, a mixed-case name carrying the punctuation
/// these members admit, and one at the tightest ceiling the resources share.
///
/// The ceiling and the character set are the TIGHTEST of the members under test, so one
/// table serves every row: `CreateBlueprint$Name` is `OrchestrationNameString`, length
/// 1..128 with pattern `^[\.\-_A-Za-z0-9]+$`, and `CreateSchema$SchemaName` is
/// `SchemaRegistryNameString` with pattern `^[a-zA-Z0-9-_$#.]+$` (both from
/// `smithy_query(query="operation", operation="CreateBlueprint,CreateSchema")`), while the
/// other members are `NameString`, length 1..255.
const SHARED_NAME_CEILING: usize = 128;

/// One row of the collision table: the resource, the create that takes a name, and the
/// refusal the second create of that name answers.
struct Collision {
    /// The resource the collision is about.
    case: &'static str,
    /// The operation that takes the name.
    operation: &'static str,
    /// The create request for a given name.
    body: fn(&str) -> Value,
    /// The modelled error the second create answers.
    code: &'static str,
    /// The `Message` that refusal carries for that name.
    message: fn(&str) -> String,
}

fn blueprint_body(name: &str) -> Value {
    json!({ "Name": name, "BlueprintLocation": "s3://blueprints/layout.zip" })
}

fn classifier_body(name: &str) -> Value {
    json!({ "CsvClassifier": { "Name": name, "Delimiter": ";" } })
}

fn custom_entity_type_body(name: &str) -> Value {
    json!({ "Name": name, "RegexString": "KEEP-[0-9]+" })
}

fn data_quality_ruleset_body(name: &str) -> Value {
    json!({ "Name": name, "Ruleset": "Rules = [ RowCount > 0 ]" })
}

fn glossary_body(name: &str) -> Value {
    json!({ "Name": name })
}

fn connection_body(name: &str) -> Value {
    json!({
        "ConnectionInput": {
            "Name": name,
            "ConnectionType": "JDBC",
            "ConnectionProperties": {
                "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
                "USERNAME": "etl",
            },
        },
    })
}

fn job_body(name: &str) -> Value {
    json!({
        "Name": name,
        "Role": ROLE_ARN,
        "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
    })
}

fn schema_body(name: &str) -> Value {
    json!({
        "SchemaName": name,
        "DataFormat": "AVRO",
        "SchemaDefinition": AVRO_DEFINITION,
    })
}

fn workflow_body(name: &str) -> Value {
    json!({ "Name": name })
}

/// Every create whose name collision this service words its own way, with that wording.
fn collisions() -> Vec<Collision> {
    vec![
        Collision {
            case: "blueprint",
            operation: "CreateBlueprint",
            body: blueprint_body,
            code: "AlreadyExistsException",
            message: |_| "Blueprint with given name already exists".to_string(),
        },
        Collision {
            case: "classifier",
            operation: "CreateClassifier",
            body: classifier_body,
            code: "AlreadyExistsException",
            message: |name| format!("Classifier {name} already exists. already exists"),
        },
        Collision {
            case: "custom entity type",
            operation: "CreateCustomEntityType",
            body: custom_entity_type_body,
            code: "AlreadyExistsException",
            message: |_| "A Custom Entity Type with the same name exist in this account".to_string(),
        },
        Collision {
            case: "data quality ruleset",
            operation: "CreateDataQualityRuleset",
            body: data_quality_ruleset_body,
            code: "AlreadyExistsException",
            message: |name| format!("Another ruleset with the same name already exists: {name}"),
        },
        Collision {
            case: "glossary",
            operation: "CreateGlossary",
            body: glossary_body,
            code: "AlreadyExistsException",
            message: |name| {
                format!("{{\"Message\":\"A glossary with name '{name}' already exists.\"}}")
            },
        },
        Collision {
            case: "connection",
            operation: "CreateConnection",
            body: connection_body,
            code: "AlreadyExistsException",
            message: |_| "Connection already exists.".to_string(),
        },
        Collision {
            case: "job definition re-submitted unchanged",
            operation: "CreateJob",
            body: job_body,
            code: "AlreadyExistsException",
            message: |_| "Job already exists.".to_string(),
        },
        Collision {
            case: "schema set",
            operation: "CreateSchema",
            body: schema_body,
            code: "AlreadyExistsException",
            message: |name| format!("Schema already exists. SchemaName: {name}"),
        },
        Collision {
            case: "workflow",
            operation: "CreateWorkflow",
            body: workflow_body,
            code: "AlreadyExistsException",
            message: |_| "Workflow with given name already Exists".to_string(),
        },
    ]
}

/// The names one row is driven with.
fn names(resource: &str) -> Vec<(String, String)> {
    let stem = resource.replace(' ', "-");
    vec![
        (
            "recorded fixture shape".to_string(),
            format!("sim2real-probe-glue-{stem}-9a546660"),
        ),
        ("single character".to_string(), "x".to_string()),
        (
            "mixed case with punctuation".to_string(),
            format!("Curated.{stem}_One-2026"),
        ),
        (
            format!("at the shared {SHARED_NAME_CEILING} ceiling"),
            "n".repeat(SHARED_NAME_CEILING),
        ),
    ]
}

/// Every create above answers its own sentence for a name already taken, whatever the
/// name is: the fixed text of that resource's refusal, carrying only the variable parts
/// the service names and stating the collision once.
#[tokio::test]
async fn a_duplicate_create_answers_the_service_wording_for_that_resource() {
    for collision in collisions() {
        for (case, name) in names(collision.case) {
            // A fresh emulator per row, so a name at the shared ceiling can be taken in
            // every table without a store filling up across rows.
            let router = emulator();
            let case = format!("{} / {case}", collision.case);
            ok(&router, collision.operation, &(collision.body)(&name)).await;
            assert_refusal(
                &router,
                &case,
                collision.operation,
                &(collision.body)(&name),
                collision.code,
                &(collision.message)(&name),
            )
            .await;
        }
    }
}

/// The business catalog's refusal is a JSON DOCUMENT in `Message`, so a client that
/// parses the nested envelope — which is what that surface's refusals require — reads the
/// sentence out of it and finds the glossary it asked for named there.
#[tokio::test]
async fn a_duplicate_glossary_create_carries_a_json_document_as_its_message() {
    let router = emulator();

    for (case, name) in names("glossary") {
        ok(&router, "CreateGlossary", &glossary_body(&name)).await;
        let (status, body) = call(&router, "CreateGlossary", &glossary_body(&name)).await;
        assert_eq!(
            status, 400,
            "case {case}: a duplicate glossary name must be refused with 400, got {status} \
             with {body}"
        );
        let message = error_message(&body);
        let nested: Value = serde_json::from_str(&message).unwrap_or_else(|error| {
            panic!("case {case}: Message must be a JSON document, got {message:?} ({error})")
        });
        assert_eq!(
            nested.get("Message").and_then(Value::as_str),
            Some(format!("A glossary with name '{name}' already exists.").as_str()),
            "case {case}: the nested envelope must carry the sentence, got {nested}"
        );
    }
}

/// `CreateJob` treats the job name as the create's idempotency identifier: a second
/// create under a name already taken that submits a DIFFERENT definition is the
/// mismatch, and one that submits the stored definition is the duplicate. Either way the
/// stored definition is left as the first create wrote it.
///
/// The rows differ along which member of the definition the second create changes — a
/// required member, an optional one, a server-defaulted one, a map, a list-valued
/// connection list and the tags — and two rows change nothing but the ORDER of a map's
/// entries, which is not a different definition.
#[tokio::test]
async fn a_second_create_job_is_classified_by_the_definition_it_submits() {
    /// One row: how the second create's definition differs, and whether that makes it
    /// another definition.
    struct Row {
        case: &'static str,
        /// The member the second create replaces, as a JSON patch over the first body.
        patch: Value,
        /// Whether the row submits a definition other than the stored one.
        differs: bool,
    }

    let rows = vec![
        Row {
            case: "the identical definition",
            patch: json!({}),
            differs: false,
        },
        Row {
            case: "another role",
            patch: json!({ "Role": "arn:aws:iam::000000000000:role/other" }),
            differs: true,
        },
        Row {
            case: "another script location",
            patch: json!({
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/other.py" },
            }),
            differs: true,
        },
        Row {
            case: "a description the stored definition has not",
            patch: json!({ "Description": "the second submission" }),
            differs: true,
        },
        Row {
            case: "another retry budget",
            patch: json!({ "MaxRetries": 3 }),
            differs: true,
        },
        Row {
            case: "another Glue version, which the service defaults",
            patch: json!({ "GlueVersion": "4.0" }),
            differs: true,
        },
        Row {
            case: "another worker configuration",
            patch: json!({ "WorkerType": "G.2X", "NumberOfWorkers": 4 }),
            differs: true,
        },
        Row {
            case: "another default argument",
            patch: json!({ "DefaultArguments": { "--job-language": "scala" } }),
            differs: true,
        },
        Row {
            case: "another connection list",
            patch: json!({ "Connections": { "Connections": ["warehouse"] } }),
            differs: true,
        },
        Row {
            case: "another tag",
            patch: json!({ "Tags": { "owner": "platform" } }),
            differs: true,
        },
    ];

    /// The definition the first create stores: enough members that a row can change one
    /// of each kind, and two maps whose entries a row re-orders.
    fn stored(name: &str) -> Value {
        json!({
            "Name": name,
            "Role": ROLE_ARN,
            "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
            "Description": "",
            "MaxRetries": 1,
            "DefaultArguments": { "--job-language": "python", "--enable-metrics": "true" },
            "Tags": { "team": "data", "env": "test" },
        })
    }

    for (index, row) in rows.iter().enumerate() {
        let router = emulator();
        let name = format!("classified-{index}");
        ok(&router, "CreateJob", &stored(&name)).await;
        let read = ok(&router, "GetJob", &json!({ "JobName": &name })).await;

        let mut second = stored(&name);
        for (member, value) in row.patch.as_object().expect("the patch is an object") {
            second[member] = value.clone();
        }

        let (status, body) = call(&router, "CreateJob", &second).await;
        assert_eq!(
            status, 400,
            "case {}: a name already taken must be refused with 400, got {status} with {body}",
            row.case
        );
        let (expected_code, expected_message) = if row.differs {
            (
                "IdempotentParameterMismatchException",
                "Job already submitted with different configuration.",
            )
        } else {
            ("AlreadyExistsException", "Job already exists.")
        };
        assert!(
            error_code(&body).contains(expected_code),
            "case {}: expected {expected_code}, got {body}",
            row.case
        );
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {}: expected Message {expected_message:?}, got {body}",
            row.case
        );

        // Neither refusal writes: the stored definition is still the first create's.
        let after = ok(&router, "GetJob", &json!({ "JobName": &name })).await;
        assert_eq!(
            after, read,
            "case {}: a refused create must change nothing; expected {read}, got {after}",
            row.case
        );
    }
}

/// The same definition re-submitted with its maps written in another ORDER is the stored
/// definition, not another one: a map is a set of entries, so a retry whose JSON lists
/// them differently is the duplicate and not the mismatch.
#[tokio::test]
async fn a_map_written_in_another_order_is_the_same_job_definition() {
    let router = emulator();

    let first = json!({
        "Name": "ordered-maps",
        "Role": ROLE_ARN,
        "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
        "DefaultArguments": { "--a": "1", "--b": "2", "--c": "3" },
        "Tags": { "one": "1", "two": "2" },
    });
    let reordered = json!({
        "Name": "ordered-maps",
        "Role": ROLE_ARN,
        "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
        "DefaultArguments": { "--c": "3", "--a": "1", "--b": "2" },
        "Tags": { "two": "2", "one": "1" },
    });

    ok(&router, "CreateJob", &first).await;
    assert_refusal(
        &router,
        "the same definition with its maps re-ordered",
        "CreateJob",
        &reordered,
        "AlreadyExistsException",
        "Job already exists.",
    )
    .await;
}

/// A classifier this account holds none of is refused with the sentence the service words
/// THAT operation's miss with: the read and the update state the service's own doubled
/// sentence, and the delete states the miss once over the account whose namespace was
/// searched and the requested name.
///
/// The rows differ along how the name misses and along the operation that addresses it,
/// so each operation's own wording is pinned rather than one borrowed for all three.
#[tokio::test]
async fn a_classifier_miss_is_worded_by_the_operation_that_addresses_it() {
    let router = emulator();
    ok(
        &router,
        "CreateClassifier",
        &classifier_body("stored-classifier"),
    )
    .await;

    for (case, name) in names("classifier") {
        let requests: Vec<(&str, Value, String)> = vec![
            (
                "GetClassifier",
                json!({ "Name": &name }),
                format!("Classifier {name} not found. not found."),
            ),
            (
                "UpdateClassifier",
                json!({ "CsvClassifier": { "Name": &name, "Delimiter": "," } }),
                format!("Classifier {name} not found. not found."),
            ),
            (
                "DeleteClassifier",
                json!({ "Name": &name }),
                format!("Classifier {ACCOUNT_ID}:{name} not found."),
            ),
        ];
        for (operation, request, expected) in &requests {
            let case = format!("{case} through {operation}");
            let body = assert_refusal(
                &router,
                &case,
                operation,
                request,
                "EntityNotFoundException",
                expected,
            )
            .await;
            // The delete states the miss ONCE: a sentence carrying its suffix twice is
            // the composition bug this suite pins against.
            if *operation == "DeleteClassifier" {
                assert_eq!(
                    error_message(&body).matches("not found.").count(),
                    1,
                    "case {case}: the delete states the miss once, got {body}"
                );
            }
        }
    }

    // The stored classifier still resolves, so the misses above are misses and not a
    // broken lookup.
    let read = ok(
        &router,
        "GetClassifier",
        &json!({ "Name": "stored-classifier" }),
    )
    .await;
    assert_eq!(
        read["Classifier"]["CsvClassifier"]["Name"].as_str(),
        Some("stored-classifier"),
        "case the stored name: expected the stored classifier, got {read}"
    );
}

/// Every refusal above is a 400 that names a modelled error of its operation — the
/// wording is what this suite pins, and the status and code are what a client branches
/// on, so one row of each is asserted here through the shared helper too.
#[tokio::test]
async fn every_collision_refusal_is_a_modelled_client_error() {
    let router: MultiServiceRouter = emulator();

    for collision in collisions() {
        let name = format!("modelled-{}", collision.case.replace(' ', "-"));
        ok(&router, collision.operation, &(collision.body)(&name)).await;
        let (status, body) = call(&router, collision.operation, &(collision.body)(&name)).await;
        assert_eq!(
            status, 400,
            "case {}: expected 400, got {status} with {body}",
            collision.case
        );
        assert!(
            error_code(&body).contains(collision.code),
            "case {}: expected {}, got {body}",
            collision.case,
            collision.code
        );
    }
}

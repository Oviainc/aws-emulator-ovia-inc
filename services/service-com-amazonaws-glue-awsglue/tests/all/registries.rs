// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue schema-registry container: `CreateRegistry` and the
//! `GetRegistry` that reads back what it stored.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The suites are tables: each row is one input along an axis the rule generalises over
//! (the name's length at each published bound and the character classes its pattern
//! admits, its casing, whether a `Description` was sent and whether it was the empty
//! string, how many `Tags` the create carried, and — for the read — whether the registry
//! is addressed by name, by ARN or by both). The recorded workflow's own case, a named
//! registry created before a schema set is created inside it, is one row among them.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Map, Value};
use service_com_amazonaws_glue_awsglue::state::messages::REGISTRY_ALREADY_EXISTS;
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, RegistryEntry, MAX_REGISTRIES, REGISTRY_STATUS_AVAILABLE,
};

use crate::wire::{call, emulator, emulator_on, error_code, error_message, ok, ACCOUNT_ID};

/// The Region every request from this harness is scoped to, which is the Region the ARNs
/// a response carries are minted for.
const REGION: &str = "us-east-1";

/// The longest name `SchemaRegistryNameString` admits.
///
/// Source: `smithy_query operation=CreateRegistry` — `RegistryName` targets
/// `SchemaRegistryNameString`, length 1..255, pattern `^[a-zA-Z0-9-_$#.]+$`.
const MAX_NAME_LENGTH: usize = 255;

/// The most entries `TagsMap` admits. Source: the same call — `Tags` is a map of
/// length 0..50.
const MAX_TAGS: usize = 50;

/// The ARN of registry `name` as this endpoint mints it.
fn registry_arn(name: &str) -> String {
    format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:registry/{name}")
}

/// The string member `member` of `body`.
fn text(case: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: {member} must be a string, got {body}"))
        .to_string()
}

/// A tag map of `count` entries, so a row can sit on the published `TagsMap` bound
/// without spelling fifty literals.
fn tags(count: usize) -> Value {
    let mut map = Map::new();
    for index in 0..count {
        map.insert(format!("key-{index}"), Value::String(format!("value-{index}")));
    }
    Value::Object(map)
}

/// One row of the create-then-read table.
struct Case {
    /// What this row varies, named in every assertion.
    label: &'static str,
    /// The registry name the create sends and the store must key the record by.
    name: String,
    /// The `Description` the create sends, or `None` when it sends none.
    description: Option<&'static str>,
    /// The `Tags` the create sends, or `None` when it sends none.
    tags: Option<Value>,
}

impl Case {
    /// The `CreateRegistry` request body this row sends.
    fn body(&self) -> Value {
        let mut body = json!({ "RegistryName": self.name });
        if let Some(description) = self.description {
            body["Description"] = json!(description);
        }
        if let Some(tags) = self.tags.clone() {
            body["Tags"] = tags;
        }
        body
    }

    /// The `Tags` the response must report: what the create sent, and the EMPTY map for a
    /// create that sent none.
    fn expected_tags(&self) -> Value {
        self.tags.clone().unwrap_or_else(|| json!({}))
    }
}

/// The table: one row per input along the axis the rule generalises over.
fn cases() -> Vec<Case> {
    vec![
        Case {
            // The recorded workflow's own shape: a registry created by name, then used as
            // the container a schema set is created inside.
            label: "a named registry, described and tagged",
            name: "compatibility-registry".to_string(),
            description: Some("schemas whose compatibility is enforced across versions"),
            tags: Some(json!({ "team": "payments", "tier": "gold" })),
        },
        Case {
            label: "no Description and no Tags",
            name: "bare-registry".to_string(),
            description: None,
            tags: None,
        },
        Case {
            // `DescriptionString` is length 0..2048, so the empty string is a description
            // the model admits and is not the same input as sending none.
            label: "an empty Description and an empty Tags map",
            name: "empty-described-registry".to_string(),
            description: Some(""),
            tags: Some(json!({})),
        },
        Case {
            // Every character class the name's pattern admits, in one name.
            label: "every character class the name pattern admits",
            name: "ops_registry-01$dev#north.east".to_string(),
            description: Some("punctuation the pattern admits"),
            tags: Some(json!({ "owner": "platform" })),
        },
        Case {
            // The lower published bound on the name's length.
            label: "a name of the shortest admitted length",
            name: "r".to_string(),
            description: None,
            tags: Some(json!({ "size": "min" })),
        },
        Case {
            // The upper published bound.
            label: "a name of the longest admitted length",
            name: "z".repeat(MAX_NAME_LENGTH),
            description: Some("sitting on the published name-length bound"),
            tags: None,
        },
        Case {
            // `SchemaRegistryNameString`'s pattern admits both cases and the registry
            // documents no fold, so this is a different registry from the first row's.
            label: "the first row's name in another casing",
            name: "Compatibility-Registry".to_string(),
            description: Some("a distinct registry, keyed by the name as sent"),
            tags: None,
        },
        Case {
            // The upper published bound on the tag map.
            label: "as many Tags as the map admits",
            name: "fully-tagged-registry".to_string(),
            description: None,
            tags: Some(tags(MAX_TAGS)),
        },
    ]
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// What `CreateRegistry` persists is what `GetRegistry` reports: the members the request
/// carried round-trip, the ARN is minted from the Region and the name, the record is
/// readable at once, and its two timestamps agree because nothing has updated it.
#[tokio::test]
async fn created_registries_are_read_back_from_the_store() {
    let router = emulator();

    for case in cases() {
        let label = case.label;
        let created = ok(&router, "CreateRegistry", &case.body()).await;

        assert_eq!(
            text(label, &created, "RegistryName"),
            case.name,
            "case {label}: expected RegistryName {}, got {created}",
            case.name
        );
        assert_eq!(
            text(label, &created, "RegistryArn"),
            registry_arn(&case.name),
            "case {label}: expected the registry's ARN, got {created}"
        );
        assert_eq!(
            created.get("Description").cloned(),
            case.description.map(|description| json!(description)),
            "case {label}: Description must round-trip, got {created}"
        );
        assert_eq!(
            created.get("Tags"),
            Some(&case.expected_tags()),
            "case {label}: expected Tags {}, got {created}",
            case.expected_tags()
        );

        // Addressed every way a `RegistryId` can address one registry, the read answers
        // the same record.
        let addressings = [
            ("by RegistryName", json!({ "RegistryName": case.name })),
            (
                "by RegistryArn",
                json!({ "RegistryArn": registry_arn(&case.name) }),
            ),
            (
                "by both members",
                json!({
                    "RegistryName": case.name,
                    "RegistryArn": registry_arn(&case.name),
                }),
            ),
        ];
        for (addressing, registry_id) in addressings {
            let read = ok(&router, "GetRegistry", &json!({ "RegistryId": registry_id })).await;
            assert_eq!(
                text(label, &read, "RegistryName"),
                case.name,
                "case {label} read {addressing}: expected RegistryName {}, got {read}",
                case.name
            );
            assert_eq!(
                text(label, &read, "RegistryArn"),
                registry_arn(&case.name),
                "case {label} read {addressing}: expected the registry's ARN, got {read}"
            );
            assert_eq!(
                read.get("Description").cloned(),
                case.description.map(|description| json!(description)),
                "case {label} read {addressing}: expected the stored Description, got {read}"
            );
            assert_eq!(
                text(label, &read, "Status"),
                REGISTRY_STATUS_AVAILABLE,
                "case {label} read {addressing}: a created registry is readable at once, \
                 got {read}"
            );
            let created_time = text(label, &read, "CreatedTime");
            assert_eq!(
                created_time,
                text(label, &read, "UpdatedTime"),
                "case {label} read {addressing}: nothing has updated the registry, got {read}"
            );
            assert!(
                ::aws_smithy_types::DateTime::from_str(
                    &created_time,
                    ::aws_smithy_types::date_time::Format::DateTime,
                )
                .is_ok(),
                "case {label} read {addressing}: expected a date-time CreatedTime, got \
                 {created_time:?}"
            );
        }
    }
}

/// A `GetRegistry` reports and writes nothing: two reads of one registry answer the same
/// thing, and the store holds exactly the records the creates made.
#[tokio::test]
async fn reading_a_registry_changes_nothing() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    let expected: Vec<String> = cases().iter().map(|case| case.name.clone()).collect();
    for case in cases() {
        ok(&router, "CreateRegistry", &case.body()).await;
    }

    for case in cases() {
        let label = case.label;
        let body = json!({ "RegistryId": { "RegistryName": case.name } });
        let first = ok(&router, "GetRegistry", &body).await;
        let second = ok(&router, "GetRegistry", &body).await;
        assert_eq!(
            first, second,
            "case {label}: two reads of one registry must answer the same thing, got \
             {first} then {second}"
        );
    }

    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let store = state.lock();
    let mut stored: Vec<String> = store.registries.keys().cloned().collect();
    stored.sort();
    let mut expected = expected;
    expected.sort();
    assert_eq!(
        stored, expected,
        "case every row: the store holds exactly the created registries, got {stored:?}"
    );
}

/// A created registry is the container a schema set is created inside: the registry
/// `CreateRegistry` wrote is the one `CreateSchema` resolves, and the schema it stores
/// reports that registry's name and ARN.
#[tokio::test]
async fn a_created_registry_holds_schema_sets() {
    let router = emulator();

    for case in cases() {
        let label = case.label;
        ok(&router, "CreateRegistry", &case.body()).await;

        let created = ok(
            &router,
            "CreateSchema",
            &json!({
                "RegistryId": { "RegistryName": case.name },
                "SchemaName": "payments",
                "DataFormat": "AVRO",
                "Compatibility": "BACKWARD",
                "SchemaDefinition": r#"{"type":"record","name":"Payment","fields":[]}"#,
            }),
        )
        .await;
        assert_eq!(
            text(label, &created, "RegistryName"),
            case.name,
            "case {label}: the schema set is created inside the new registry, got {created}"
        );
        assert_eq!(
            text(label, &created, "RegistryArn"),
            registry_arn(&case.name),
            "case {label}: expected the new registry's ARN, got {created}"
        );

        let read = ok(
            &router,
            "GetSchema",
            &json!({
                "SchemaId": { "RegistryName": case.name, "SchemaName": "payments" },
            }),
        )
        .await;
        assert_eq!(
            text(label, &read, "RegistryName"),
            case.name,
            "case {label}: the stored schema set names its registry, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A registry name is unique per account: a second create of a name this endpoint already
/// holds is refused with 400 `AlreadyExistsException`, and the stored registry keeps the
/// description and tags its own create carried.
#[tokio::test]
async fn a_name_the_account_already_holds_is_refused_and_overwrites_nothing() {
    let router = emulator();

    for case in cases() {
        let label = case.label;
        ok(&router, "CreateRegistry", &case.body()).await;

        let again = json!({
            "RegistryName": case.name,
            "Description": "a second create, with another description",
            "Tags": { "attempt": "second" },
        });
        let (status, refusal) = call(&router, "CreateRegistry", &again).await;
        assert_eq!(
            status, 400,
            "case {label}: expected 400, got {status} with {refusal}"
        );
        assert!(
            error_code(&refusal).contains("AlreadyExistsException"),
            "case {label}: expected AlreadyExistsException, got {refusal}"
        );
        assert_eq!(
            error_message(&refusal),
            REGISTRY_ALREADY_EXISTS,
            "case {label}: expected the already-exists wording, got {refusal}"
        );

        let read = ok(
            &router,
            "GetRegistry",
            &json!({ "RegistryId": { "RegistryName": case.name } }),
        )
        .await;
        assert_eq!(
            read.get("Description").cloned(),
            case.description.map(|description| json!(description)),
            "case {label}: the refused create left the stored Description alone, got {read}"
        );
    }
}

/// A `RegistryId` that resolves to a registry this endpoint holds none of is refused with
/// 400 `EntityNotFoundException` naming what was looked for — whether the registry was
/// never created, was addressed by an ARN of another Region, or by one of another account.
#[tokio::test]
async fn a_registry_the_endpoint_does_not_hold_is_not_found() {
    let router = emulator();
    ok(
        &router,
        "CreateRegistry",
        &json!({ "RegistryName": "present-registry" }),
    )
    .await;

    let cases = [
        (
            "a name no create has taken",
            json!({ "RegistryName": "absent-registry" }),
            "absent-registry",
        ),
        (
            "an ARN of a registry no create has taken",
            json!({ "RegistryArn": registry_arn("absent-registry") }),
            "absent-registry",
        ),
        (
            "an ARN of another Region",
            json!({
                "RegistryArn": format!("arn:aws:glue:eu-west-1:{ACCOUNT_ID}:registry/present-registry"),
            }),
            "present-registry",
        ),
        (
            "an ARN of another account",
            json!({ "RegistryArn": "arn:aws:glue:us-east-1:210987654321:registry/present-registry" }),
            "present-registry",
        ),
    ];

    for (label, registry_id, looked_for) in cases {
        let (status, refusal) =
            call(&router, "GetRegistry", &json!({ "RegistryId": registry_id })).await;
        assert_eq!(
            status, 400,
            "case {label}: expected 400, got {status} with {refusal}"
        );
        assert!(
            error_code(&refusal).contains("EntityNotFoundException"),
            "case {label}: expected EntityNotFoundException, got {refusal}"
        );
        let message = error_message(&refusal);
        assert!(
            message.contains(looked_for),
            "case {label}: the refusal names what was looked for, got {message:?}"
        );
    }
}

/// A `RegistryId` that names no registry at all is a bad parameter, not a miss: 400
/// `InvalidInputException` for a wrapper carrying neither member, for two members naming
/// different registries, and for a value that is not a registry ARN.
#[tokio::test]
async fn a_registry_id_that_names_nothing_is_a_bad_parameter() {
    let router = emulator();
    ok(
        &router,
        "CreateRegistry",
        &json!({ "RegistryName": "present-registry" }),
    )
    .await;

    let cases = [
        ("neither member", json!({})),
        (
            "two members naming different registries",
            json!({
                "RegistryName": "present-registry",
                "RegistryArn": registry_arn("another-registry"),
            }),
        ),
        (
            "an ARN of another kind of Glue resource",
            json!({ "RegistryArn": format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:database/sales") }),
        ),
        (
            "an ARN whose registry segment carries a deeper resource path",
            json!({
                "RegistryArn": format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:registry/outer/inner"),
            }),
        ),
    ];

    for (label, registry_id) in cases {
        let (status, refusal) =
            call(&router, "GetRegistry", &json!({ "RegistryId": registry_id })).await;
        assert_eq!(
            status, 400,
            "case {label}: expected 400, got {status} with {refusal}"
        );
        assert!(
            error_code(&refusal).contains("InvalidInputException"),
            "case {label}: expected InvalidInputException, got {refusal}"
        );
    }
}

/// One past each published bound on the create's own members is refused, and nothing is
/// stored for it: a name longer than the modelled length, a name carrying a character the
/// pattern does not admit, and a description past its length.
#[tokio::test]
async fn an_input_past_a_published_bound_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    let cases = [
        (
            "a name one character past the length bound",
            json!({ "RegistryName": "z".repeat(MAX_NAME_LENGTH + 1) }),
        ),
        (
            "a name of no characters at all",
            json!({ "RegistryName": "" }),
        ),
        (
            "a name carrying a character the pattern does not admit",
            json!({ "RegistryName": "has a space" }),
        ),
        (
            "a Description one character past its length bound",
            json!({ "RegistryName": "described-too-far", "Description": "d".repeat(2049) }),
        ),
        (
            "one Tag past the bound on the map",
            json!({ "RegistryName": "tagged-too-far", "Tags": tags(MAX_TAGS + 1) }),
        ),
    ];

    for (label, body) in cases {
        let (status, refusal) = call(&router, "CreateRegistry", &body).await;
        assert_eq!(
            status, 400,
            "case {label}: expected 400, got {status} with {refusal}"
        );
    }

    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let store = state.lock();
    assert!(
        store.registries.is_empty(),
        "case every refused row: a refused create stores nothing, got {:?}",
        store.registries.keys().collect::<Vec<_>>()
    );
}

/// The store is resident memory and this create is reachable by any caller, so the number
/// of registries one process holds is bounded: the create past the ceiling is refused with
/// `ResourceNumberLimitExceededException`, and the one that fits is served.
#[tokio::test]
async fn a_registry_past_the_store_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for index in 0..MAX_REGISTRIES {
            let name = format!("seeded-registry-{index}");
            store.registries.insert(
                name.clone(),
                RegistryEntry::new(&name, None, None, SystemTime::UNIX_EPOCH),
            );
        }
    }

    let body = json!({ "RegistryName": "one-registry-too-many" });
    let (status, refusal) = call(&router, "CreateRegistry", &body).await;
    assert_eq!(
        status, 400,
        "case registry {} of {MAX_REGISTRIES}: expected 400, got {status} with {refusal}",
        MAX_REGISTRIES + 1
    );
    assert!(
        error_code(&refusal).contains("ResourceNumberLimitExceededException"),
        "case one registry too many: expected ResourceNumberLimitExceededException, got \
         {refusal}"
    );

    // One place freed, and the same create is served: the ceiling is on how many records
    // the store holds, not on this name.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().registries.remove("seeded-registry-0");
    }
    let created = ok(&router, "CreateRegistry", &body).await;
    assert_eq!(
        text("one place freed", &created, "RegistryName"),
        "one-registry-too-many",
        "case one place freed: expected the create to be served, got {created}"
    );
}

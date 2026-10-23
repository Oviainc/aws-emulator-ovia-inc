// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Athena's workgroup slice: `CreateWorkGroup` writes a workgroup
//! and `GetWorkGroup` reads it back, driven over the wire (awsJson1_1,
//! `x-amz-target: AmazonAthena.<Operation>`) through the same router the emulator
//! boots, so every assertion is about what an SDK client observes.
//!
//! Each test states a rule and asserts it over a TABLE of inputs rather than one
//! recorded request: the rows differ along whatever the rule generalises over — name
//! shape and length, which optional members are present, the size of a tag list, the
//! engine version selected.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::ops::types::WORK_GROUP_STATE_VALUES;
use service_com_amazonaws_athena_amazonathena::state::{
    self, DEFAULT_EFFECTIVE_ENGINE_VERSION, ENGINE_VERSION_AUTO, MAX_TAGS_PER_RESOURCE,
    MAX_WORK_GROUP_CONFIGURATION_BYTES, WORK_GROUP_STATE_DISABLED, WORK_GROUP_STATE_ENABLED,
};
use service_com_amazonaws_athena_amazonathena::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `x-amz-target` prefix of this service: the local name of the Smithy service
/// shape (`com.amazonaws.athena#AmazonAthena`), which is how the awsJson protocols
/// build that header.
const TARGET_PREFIX: &str = "AmazonAthena";

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `athena` (this service's `sigv4Name`). Neither the key id
/// nor the signature is a credential: the harness turns request authentication off
/// below, so nothing here is verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/athena/aws4_request, \
     SignedHeaders=host;x-amz-target, Signature=not-verified";

/// One service mounted on its own engine, reached the way the production router
/// reaches it, plus the engine itself for the one assertion that has no wire reader
/// (`ListTagsForResource` is still a scaffold).
fn engine_and_router() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Pinned off so the suite does not depend on how the emulator process was started;
    // these tests assert Athena's behaviour, not the emulator's SigV4 check.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// A router over a fresh engine, for a test that needs no direct state access.
fn router() -> MultiServiceRouter {
    engine_and_router().1
}

/// Invoke one operation and return `(status, body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "athena.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("content-type", "application/x-amz-json-1.1")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code (`__type`) of an error body, with the shape-id prefix the
/// awsJson protocols may carry stripped.
fn error_code(body: &Value) -> String {
    let raw = body["__type"].as_str().unwrap_or("<no __type>");
    raw.rsplit(['#', '/']).next().unwrap_or(raw).to_string()
}

/// `GetWorkGroup`'s `WorkGroup` member, or `Value::Null`.
fn work_group(body: &Value) -> Value {
    body["WorkGroup"].clone()
}

/// A string member of a JSON object, or `None` when absent.
fn text(value: &Value, member: &str) -> Option<String> {
    value[member].as_str().map(str::to_string)
}

/// Seconds since the epoch, the form an awsJson timestamp renders in.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// A valid workgroup name of `length` characters, all of them inside the modelled
/// `WorkGroupName` pattern `^[a-zA-Z0-9._-]{1,128}$`.
fn name_of_length(length: usize) -> String {
    "aA0._-".chars().cycle().take(length).collect()
}

/// One row of the create/read-back table: case name, the `CreateWorkGroup` request
/// body, and the `Description` the stored workgroup must report.
struct CreateCase {
    case: &'static str,
    request: Value,
    name: String,
    description: Option<String>,
}

/// The rows: a workgroup differs from the next along its name's shape and length,
/// whether a description or a configuration is present, and how much of the
/// configuration is filled in.
fn create_cases() -> Vec<CreateCase> {
    let shortest = name_of_length(1);
    let longest = name_of_length(128);
    let long_description = "d".repeat(1024);
    vec![
        CreateCase {
            case: "only the required name, one character long",
            request: json!({ "Name": shortest }),
            name: shortest.clone(),
            description: None,
        },
        CreateCase {
            case: "every character class the name pattern allows",
            request: json!({ "Name": "analytics.team_0-eu", "Description": "" }),
            name: "analytics.team_0-eu".to_string(),
            description: Some(String::new()),
        },
        CreateCase {
            case: "the longest name and the longest description the model allows",
            request: json!({ "Name": longest, "Description": long_description }),
            name: longest.clone(),
            description: Some(long_description.clone()),
        },
        CreateCase {
            case: "an Athena SQL configuration with a results location",
            request: json!({
                "Name": "sql-workgroup",
                "Description": "an SQL workgroup",
                "Configuration": {
                    "ResultConfiguration": {
                        "OutputLocation": "s3://athena-results/prefix/",
                        "EncryptionConfiguration": { "EncryptionOption": "SSE_S3" }
                    },
                    "EnforceWorkGroupConfiguration": true,
                    "PublishCloudWatchMetricsEnabled": true,
                    "BytesScannedCutoffPerQuery": 10_000_000_i64,
                    "RequesterPaysEnabled": false
                }
            }),
            name: "sql-workgroup".to_string(),
            description: Some("an SQL workgroup".to_string()),
        },
        CreateCase {
            case: "a Spark enabled configuration with an engine configuration",
            request: json!({
                "Name": "spark_workgroup",
                "Configuration": {
                    "ExecutionRole": "arn:aws:iam::000000000000:role/AthenaSparkExecution",
                    "EngineConfiguration": {
                        "CoordinatorDpuSize": 1,
                        "MaxConcurrentDpus": 20,
                        "DefaultExecutorDpuSize": 1
                    },
                    "AdditionalConfiguration": "{\"spark.sql.shuffle.partitions\":\"8\"}"
                }
            }),
            name: "spark_workgroup".to_string(),
            description: None,
        },
        CreateCase {
            case: "three tags",
            request: json!({
                "Name": "tagged-workgroup",
                "Tags": [
                    { "Key": "env", "Value": "prod" },
                    { "Key": "team", "Value": "analytics" },
                    { "Key": "cost-center", "Value": "" }
                ]
            }),
            name: "tagged-workgroup".to_string(),
            description: None,
        },
    ]
}

#[tokio::test]
async fn a_created_work_group_is_read_back_by_get_work_group() {
    let router = router();

    for case in create_cases() {
        let before = now_epoch_seconds();
        let (status, created) = call(&router, "CreateWorkGroup", &case.request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateWorkGroup to answer 200, got {status} with {created:?}",
            case.case
        );
        assert_eq!(
            created,
            json!({}),
            "case {:?}: CreateWorkGroupOutput models no members, got {created:?}",
            case.case
        );

        let (status, read) =
            call(&router, "GetWorkGroup", &json!({ "WorkGroup": case.name })).await;
        let after = now_epoch_seconds();
        assert_eq!(
            status, 200,
            "case {:?}: expected GetWorkGroup to answer 200, got {status} with {read:?}",
            case.case
        );
        let group = work_group(&read);

        assert_eq!(
            text(&group, "Name").as_deref(),
            Some(case.name.as_str()),
            "case {:?}: expected the workgroup to be named {:?}, got {group:?}",
            case.case,
            case.name
        );
        assert_eq!(
            text(&group, "State").as_deref(),
            Some(WORK_GROUP_STATE_ENABLED),
            "case {:?}: expected a created workgroup to be {WORK_GROUP_STATE_ENABLED}, got {group:?}",
            case.case
        );
        assert_eq!(
            text(&group, "Description"),
            case.description,
            "case {:?}: expected the stored description {:?}, got {:?}",
            case.case,
            case.description,
            text(&group, "Description")
        );

        let creation_time = group["CreationTime"].as_f64();
        assert!(
            creation_time.is_some_and(|at| at >= before && at <= after),
            "case {:?}: expected CreationTime inside [{before}, {after}], got {:?}",
            case.case,
            group["CreationTime"]
        );

        // Every configuration member the request carried comes back unchanged. The
        // engine version is the one exception and has its own test: the model
        // documents it read-only on this operation.
        if let Some(sent) = case.request.get("Configuration") {
            let read_back = &group["Configuration"];
            for (member, value) in sent.as_object().expect("the sent configuration is an object") {
                assert_eq!(
                    &read_back[member], value,
                    "case {:?}: expected Configuration.{member} to read back as {value:?}, got {:?}",
                    case.case, read_back[member]
                );
            }
        }
    }
}

/// One row of the tag table: the case name, the `Tags` member sent (null = absent),
/// and the (key, value) pairs the stored record must hold, in order.
type TagCase = (&'static str, Value, Vec<(&'static str, &'static str)>);

#[tokio::test]
async fn tags_a_create_carried_are_persisted_against_the_work_group() {
    // `ListTagsForResource` is still a scaffold, so the tag list is asserted against
    // the record the write persisted rather than over the wire.
    let (engine, router) = engine_and_router();

    // (case, the tag list sent, the (key, value) pairs the record must hold)
    let cases: Vec<TagCase> = vec![
        ("no Tags member at all", Value::Null, Vec::new()),
        ("an empty tag list", json!([]), Vec::new()),
        (
            "one tag",
            json!([{ "Key": "env", "Value": "prod" }]),
            vec![("env", "prod")],
        ),
        (
            "a tag whose value is the empty string the model allows",
            json!([{ "Key": "owner", "Value": "" }]),
            vec![("owner", "")],
        ),
        (
            "keys differing only in case, which Tag$Key calls case-sensitive",
            json!([{ "Key": "env", "Value": "prod" }, { "Key": "Env", "Value": "dev" }]),
            vec![("env", "prod"), ("Env", "dev")],
        ),
        (
            "four tags",
            json!([
                { "Key": "a", "Value": "1" },
                { "Key": "b", "Value": "2" },
                { "Key": "c", "Value": "3" },
                { "Key": "d", "Value": "4" }
            ]),
            vec![("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")],
        ),
    ];

    for (index, (case, tags, expected)) in cases.into_iter().enumerate() {
        let name = format!("tagging-{index}");
        let mut request = json!({ "Name": name });
        if !tags.is_null() {
            request["Tags"] = tags;
        }
        let (status, body) = call(&router, "CreateWorkGroup", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateWorkGroup to answer 200, got {status} with {body:?}"
        );

        let store = state::service_state(&*engine).expect("this service's state");
        let stored: Vec<(String, String)> = store
            .lock()
            .work_groups
            .get(&name)
            .unwrap_or_else(|| panic!("case {case:?}: the workgroup must be persisted"))
            .tags
            .iter()
            .map(|tag| {
                (
                    tag.key.clone().unwrap_or_default(),
                    tag.value.clone().unwrap_or_default(),
                )
            })
            .collect();
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the persisted tags {expected:?}, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn the_effective_engine_version_is_resolved_by_the_service_not_echoed() {
    // `EngineVersion$EffectiveEngineVersion`: "Read only. ... If the user requests a
    // valid engine version other than Auto, the effective engine version is the same as
    // the engine version that the user requested. If the user requests Auto, the
    // effective engine version is chosen by Athena. When a request to update the engine
    // version is made by a CreateWorkGroup or UpdateWorkGroup operation, the
    // EffectiveEngineVersion field is ignored."
    let router = router();

    // (case, the EngineVersion sent (null = no Configuration at all), expected
    //  SelectedEngineVersion, expected EffectiveEngineVersion)
    let cases: Vec<(&str, Value, &str, &str)> = vec![
        (
            "no configuration, so the documented default selection applies",
            Value::Null,
            ENGINE_VERSION_AUTO,
            DEFAULT_EFFECTIVE_ENGINE_VERSION,
        ),
        (
            "a configuration with no engine version",
            json!({}),
            ENGINE_VERSION_AUTO,
            DEFAULT_EFFECTIVE_ENGINE_VERSION,
        ),
        (
            "AUTO selected explicitly",
            json!({ "SelectedEngineVersion": ENGINE_VERSION_AUTO }),
            ENGINE_VERSION_AUTO,
            DEFAULT_EFFECTIVE_ENGINE_VERSION,
        ),
        (
            "AUTO selected alongside an effective version the client made up",
            json!({
                "SelectedEngineVersion": ENGINE_VERSION_AUTO,
                "EffectiveEngineVersion": "Athena engine version 1"
            }),
            ENGINE_VERSION_AUTO,
            DEFAULT_EFFECTIVE_ENGINE_VERSION,
        ),
        (
            "an Athena SQL engine version selected by name",
            json!({ "SelectedEngineVersion": "Athena engine version 2" }),
            "Athena engine version 2",
            "Athena engine version 2",
        ),
        (
            "a PySpark engine version selected by name, with a contradictory effective one",
            json!({
                "SelectedEngineVersion": "PySpark engine version 3",
                "EffectiveEngineVersion": "PySpark engine version 9"
            }),
            "PySpark engine version 3",
            "PySpark engine version 3",
        ),
    ];

    for (index, (case, selected, expect_selected, expect_effective)) in
        cases.into_iter().enumerate()
    {
        let name = format!("engine-version-{index}");
        let mut request = json!({ "Name": name });
        if !selected.is_null() {
            request["Configuration"] = json!({ "EngineVersion": selected });
        }
        let (status, body) = call(&router, "CreateWorkGroup", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateWorkGroup to answer 200, got {status} with {body:?}"
        );

        let (status, read) = call(&router, "GetWorkGroup", &json!({ "WorkGroup": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetWorkGroup to answer 200, got {status} with {read:?}"
        );
        let engine_version = work_group(&read)["Configuration"]["EngineVersion"].clone();
        assert_eq!(
            text(&engine_version, "SelectedEngineVersion").as_deref(),
            Some(expect_selected),
            "case {case:?}: expected SelectedEngineVersion {expect_selected:?}, got {engine_version:?}"
        );
        assert_eq!(
            text(&engine_version, "EffectiveEngineVersion").as_deref(),
            Some(expect_effective),
            "case {case:?}: expected EffectiveEngineVersion {expect_effective:?}, got {engine_version:?}"
        );
    }
}

#[tokio::test]
async fn a_name_the_registry_already_holds_is_refused_and_the_first_record_stands() {
    let router = router();

    // The rule holds whatever the second request differs in, so the rows vary that:
    // (case, the first create, the second create, the description the record keeps)
    let cases: Vec<(&str, Value, Value, Option<&str>)> = vec![
        (
            "an identical second request",
            json!({ "Name": "same.request", "Description": "first" }),
            json!({ "Name": "same.request", "Description": "first" }),
            Some("first"),
        ),
        (
            "a second request with a different description",
            json!({ "Name": "other-description", "Description": "first" }),
            json!({ "Name": "other-description", "Description": "second" }),
            Some("first"),
        ),
        (
            "a second request that adds a configuration",
            json!({ "Name": "added_configuration" }),
            json!({
                "Name": "added_configuration",
                "Configuration": { "ResultConfiguration": { "OutputLocation": "s3://later/" } }
            }),
            None,
        ),
    ];

    for (case, first, second, kept_description) in cases {
        let (status, body) = call(&router, "CreateWorkGroup", &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first CreateWorkGroup to answer 200, got {status} with {body:?}"
        );

        let (status, refused) = call(&router, "CreateWorkGroup", &second).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the second CreateWorkGroup to answer 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {refused:?}"
        );

        let name = first["Name"].as_str().expect("the case names a workgroup");
        let (status, read) = call(&router, "GetWorkGroup", &json!({ "WorkGroup": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetWorkGroup to answer 200, got {status} with {read:?}"
        );
        let group = work_group(&read);
        assert_eq!(
            text(&group, "Description").as_deref(),
            kept_description,
            "case {case:?}: expected the refused create to leave the description {kept_description:?}, got {group:?}"
        );
        assert_eq!(
            group["Configuration"]["ResultConfiguration"]["OutputLocation"],
            first["Configuration"]["ResultConfiguration"]["OutputLocation"],
            "case {case:?}: expected the refused create to leave the configuration alone, got {group:?}"
        );
    }
}

#[tokio::test]
async fn get_work_group_refuses_a_name_no_work_group_holds() {
    let router = router();
    let (status, body) = call(
        &router,
        "CreateWorkGroup",
        &json!({ "Name": "the-one-that-exists" }),
    )
    .await;
    assert_eq!(status, 200, "the workgroup under test must be created: {body:?}");

    // Every row is a name inside the modelled `WorkGroupName` pattern that no create
    // call registered — including one that differs from the registered name only in
    // case, and one that is a prefix of it.
    for case in [
        "no-such-workgroup",
        "The-One-That-Exists",
        "the-one-that-exist",
        "primary",
        &name_of_length(128),
    ] {
        let (status, refused) = call(&router, "GetWorkGroup", &json!({ "WorkGroup": case })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected GetWorkGroup to answer 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {refused:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_key_repeated_in_one_request_is_refused_and_nothing_is_persisted() {
    // `Tag$Key`: "Tag keys are case-sensitive and must be unique per resource."
    let (engine, router) = engine_and_router();

    // (case, the tag list sent)
    let cases: Vec<(&str, Value)> = vec![
        (
            "the same key twice with the same value",
            json!([{ "Key": "env", "Value": "prod" }, { "Key": "env", "Value": "prod" }]),
        ),
        (
            "the same key twice with different values",
            json!([{ "Key": "env", "Value": "prod" }, { "Key": "env", "Value": "dev" }]),
        ),
        (
            "a repeat separated by another key",
            json!([
                { "Key": "env", "Value": "prod" },
                { "Key": "team", "Value": "analytics" },
                { "Key": "env", "Value": "dev" }
            ]),
        ),
    ];

    for (index, (case, tags)) in cases.into_iter().enumerate() {
        let name = format!("duplicate-keys-{index}");
        let (status, refused) = call(
            &router,
            "CreateWorkGroup",
            &json!({ "Name": name, "Tags": tags }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateWorkGroup to answer 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {refused:?}"
        );

        let store = state::service_state(&*engine).expect("this service's state");
        let persisted = store.lock().work_groups.contains_key(&name);
        assert!(
            !persisted,
            "case {case:?}: a refused create must persist nothing, but {name} exists"
        );

        let (status, read) = call(&router, "GetWorkGroup", &json!({ "WorkGroup": name })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected GetWorkGroup to answer 400 for the refused name, got {status} with {read:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_that_carries_no_key_is_refused_on_its_own_terms() {
    // `Tag$Key` is 1 to 128 characters; the member is optional in the model, so a tag
    // with no key reaches the handler and is refused there, while a present-but-empty
    // key fails the modelled length at the boundary.
    let (engine, router) = engine_and_router();

    // (case, the tag list sent, the wire error code expected)
    let cases: [(&str, Value, &str); 4] = [
        (
            "a single tag with no key at all",
            json!([{ "Value": "prod" }]),
            "InvalidRequestException",
        ),
        (
            "a keyless tag beside a well-formed one",
            json!([{ "Key": "env", "Value": "prod" }, { "Value": "dev" }]),
            "InvalidRequestException",
        ),
        (
            "two keyless tags",
            json!([{ "Value": "prod" }, { "Value": "dev" }]),
            "InvalidRequestException",
        ),
        (
            "a present but empty key, which the modelled TagKey length refuses",
            json!([{ "Key": "", "Value": "prod" }]),
            "ValidationException",
        ),
    ];

    for (index, (case, tags, expected_code)) in cases.into_iter().enumerate() {
        let name = format!("keyless-tag-{index}");
        let (status, refused) = call(
            &router,
            "CreateWorkGroup",
            &json!({ "Name": name, "Tags": tags }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateWorkGroup to answer 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            expected_code,
            "case {case:?}: expected {expected_code}, got {refused:?}"
        );

        let store = state::service_state(&*engine).expect("this service's state");
        let persisted = store.lock().work_groups.contains_key(&name);
        assert!(
            !persisted,
            "case {case:?}: a refused create must persist nothing, but {name} exists"
        );
    }
}

#[tokio::test]
async fn reading_a_work_group_reports_the_same_record_every_time() {
    let router = router();

    for case in create_cases() {
        let (status, body) = call(&router, "CreateWorkGroup", &case.request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateWorkGroup to answer 200, got {status} with {body:?}",
            case.case
        );

        let probe = json!({ "WorkGroup": case.name });
        let (_, first) = call(&router, "GetWorkGroup", &probe).await;
        let (_, second) = call(&router, "GetWorkGroup", &probe).await;
        assert_eq!(
            first, second,
            "case {:?}: a read must not change what the next caller sees, got {first:?} then {second:?}",
            case.case
        );
    }
}

#[test]
fn the_work_group_states_this_service_stores_are_values_the_model_declares() {
    // `WORK_GROUP_STATE_VALUES` is the model's own `WorkGroupState` enum, so the two
    // named constants the handlers write cannot drift away from the wire values.
    for case in [WORK_GROUP_STATE_ENABLED, WORK_GROUP_STATE_DISABLED] {
        assert!(
            WORK_GROUP_STATE_VALUES.contains(&case),
            "case {case:?}: expected a member of {WORK_GROUP_STATE_VALUES:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_list_is_accepted_up_to_the_per_resource_limit_and_refused_past_it() {
    // Athena's documentation: "You can have up to 50 tags per resource."
    let (engine, router) = engine_and_router();

    // (case, how many tags, whether the create is accepted) — every published bound
    // and one past it.
    let cases: [(&str, usize, bool); 6] = [
        ("no tags", 0, true),
        ("one tag", 1, true),
        ("one short of the limit", MAX_TAGS_PER_RESOURCE - 1, true),
        ("exactly the limit", MAX_TAGS_PER_RESOURCE, true),
        ("one past the limit", MAX_TAGS_PER_RESOURCE + 1, false),
        ("far past the limit", MAX_TAGS_PER_RESOURCE * 20, false),
    ];

    for (case, count, accepted) in cases {
        let name = format!("tag-count-{count}");
        let tags: Vec<Value> = (0..count)
            .map(|index| json!({ "Key": format!("k{index}"), "Value": format!("v{index}") }))
            .collect();
        let (status, body) = call(
            &router,
            "CreateWorkGroup",
            &json!({ "Name": name, "Tags": tags }),
        )
        .await;

        let expected = if accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected CreateWorkGroup to answer {expected}, got {status} with {body:?}"
        );
        if !accepted {
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {case:?}: expected InvalidRequestException, got {body:?}"
            );
        }

        let store = state::service_state(&*engine).expect("this service's state");
        let stored = store.lock().work_groups.get(&name).map(|record| record.tags.len());
        assert_eq!(
            stored,
            accepted.then_some(count),
            "case {case:?}: expected the persisted tag count {:?}, got {stored:?}",
            accepted.then_some(count)
        );
    }
}

#[tokio::test]
async fn an_open_configuration_is_accepted_up_to_its_byte_bound_and_refused_past_it() {
    // The Spark properties, classifications and CloudWatch log types in a workgroup
    // configuration are open collections the model bounds in neither length nor count,
    // so the emulator bounds the bytes it retains for them.
    let (engine, router) = engine_and_router();

    /// A Spark property map whose keys and values total exactly `bytes` characters,
    /// which is what [`MAX_WORK_GROUP_CONFIGURATION_BYTES`] measures. Split across
    /// several entries, because one `ParametersMap` value is modelled at most 51200
    /// characters long and the boundary validator refuses a longer one before any
    /// handler runs.
    fn spark_properties(bytes: usize) -> Value {
        let mut map = serde_json::Map::new();
        let mut produced = 0usize;
        let mut index = 0usize;
        while produced < bytes {
            let key = format!("spark.emulator.blob.{index}");
            let value_len = (bytes - produced).saturating_sub(key.len()).min(4096);
            assert!(
                value_len > 0,
                "the generator must place {bytes} bytes in whole entries"
            );
            produced += key.len() + value_len;
            map.insert(key, Value::String("v".repeat(value_len)));
            index += 1;
        }
        Value::Object(map)
    }

    // (case, how many bytes of open configuration, whether the create is accepted)
    let cases: [(&str, usize, bool); 4] = [
        ("a small Spark property map", 1_024, true),
        (
            "one short of the byte bound",
            MAX_WORK_GROUP_CONFIGURATION_BYTES - 1,
            true,
        ),
        (
            "exactly the byte bound",
            MAX_WORK_GROUP_CONFIGURATION_BYTES,
            true,
        ),
        (
            "well past the byte bound",
            MAX_WORK_GROUP_CONFIGURATION_BYTES * 4,
            false,
        ),
    ];

    for (index, (case, bytes, accepted)) in cases.into_iter().enumerate() {
        let name = format!("open-configuration-{index}");
        let request = json!({
            "Name": name,
            "Configuration": {
                "EngineConfiguration": {
                    "CoordinatorDpuSize": 1,
                    "MaxConcurrentDpus": 20,
                    "DefaultExecutorDpuSize": 1,
                    "SparkProperties": spark_properties(bytes)
                }
            }
        });
        let (status, body) = call(&router, "CreateWorkGroup", &request).await;

        let expected = if accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected CreateWorkGroup to answer {expected}, got {status} with {body:?}"
        );
        if !accepted {
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {case:?}: expected InvalidRequestException, got {body:?}"
            );
        }

        let store = state::service_state(&*engine).expect("this service's state");
        let persisted = store.lock().work_groups.contains_key(&name);
        assert_eq!(
            persisted, accepted,
            "case {case:?}: expected the workgroup to be persisted = {accepted}, got {persisted}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateWorkGroup`: the members a request carries replace their
//! stored counterparts, a `Remove…` flag clears the member it names, and everything the
//! request leaves out stands — read back through `GetWorkGroup`, and, where the stored
//! value decides what a query may do, through `StartQueryExecution` and
//! `GetQueryExecution`. Driven over the wire (awsJson1_1,
//! `x-amz-target: AmazonAthena.<Operation>`) through the same router the emulator boots,
//! so every assertion is about what an SDK client observes.
//!
//! Each test states a rule and asserts it over a TABLE of inputs rather than one
//! recorded request: the rows differ along which member the update names, whether it
//! sets or removes it, whether the stored configuration already held it, the state
//! toggled to, and the engine version selected.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::ops::types::WORK_GROUP_STATE_VALUES;
use service_com_amazonaws_athena_amazonathena::state::{
    self, DEFAULT_EFFECTIVE_ENGINE_VERSION, ENGINE_VERSION_AUTO,
    MAX_WORK_GROUP_CONFIGURATION_BYTES, QUERY_EXECUTION_COMPLETION, WORK_GROUP_STATE_DISABLED,
    WORK_GROUP_STATE_ENABLED,
};
use smithy_server_runtime::MultiServiceRouter;

// The wire harness is shared by the suites of this test binary.
use crate::harness::{call, router as build_router};

/// The workgroup every test below updates.
const WORK_GROUP: &str = "update-work-group-suite";

/// Where the queries in this suite write their results when the request names a
/// location. Nothing is written to it; only which location is reported matters.
const CLIENT_OUTPUT_LOCATION: &str = "s3://athena-results/client-side/";

/// The location a workgroup's own result configuration names, so the two are told apart
/// in the enforcement test.
const GROUP_OUTPUT_LOCATION: &str = "s3://athena-results/work-group/";

/// A role ARN inside the modelled `RoleArn` pattern
/// (`^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`).
const EXECUTION_ROLE: &str = "arn:aws:iam::123456789012:role/athena-execution";

/// The smallest `BytesScannedCutoffValue` the model allows (`range` min 10000000).
const CUTOFF_MINIMUM: i64 = 10_000_000;

/// The `BytesScannedCutoffPerQuery` an update raises the cutoff to.
const CUTOFF_RAISED: i64 = 20_000_000;

/// How long a test waits past [`QUERY_EXECUTION_COMPLETION`] for a started query to be
/// terminal: the span is owed from the submit, and this leaves room for the engine run
/// and the request walk around it.
const SETTLE_GRACE: Duration = Duration::from_millis(600);

/// The wire error code (`__type`) of an error body, with the shape-id prefix the
/// awsJson protocols may carry stripped.
fn error_code(body: &Value) -> String {
    let raw = body["__type"].as_str().unwrap_or("<no __type>");
    raw.rsplit(['#', '/']).next().unwrap_or(raw).to_string()
}

/// The configuration the workgroup is created with: every member a row below sets or
/// removes is already present here, so the "a member the request does not name stands"
/// half of the rule is asserted against a configuration that really holds something.
fn baseline_configuration() -> Value {
    json!({
        "ResultConfiguration": {
            "OutputLocation": CLIENT_OUTPUT_LOCATION,
            "EncryptionConfiguration": { "EncryptionOption": "SSE_S3" },
            "ExpectedBucketOwner": "123456789012",
            "AclConfiguration": { "S3AclOption": "BUCKET_OWNER_FULL_CONTROL" },
        },
        "EnforceWorkGroupConfiguration": false,
        "PublishCloudWatchMetricsEnabled": false,
        "RequesterPaysEnabled": false,
        "EnableMinimumEncryptionConfiguration": false,
        "BytesScannedCutoffPerQuery": CUTOFF_MINIMUM,
        "CustomerContentEncryptionConfiguration": {
            "KmsKey": "arn:aws:kms:us-east-1:123456789012:key/baseline",
        },
        "AdditionalConfiguration": "{\"baseline\":true}",
        "ExecutionRole": EXECUTION_ROLE,
        "EngineVersion": { "SelectedEngineVersion": ENGINE_VERSION_AUTO },
    })
}

/// Create [`WORK_GROUP`] with [`baseline_configuration`] and return the `WorkGroup` a
/// read reports for it.
async fn work_group_with_baseline(router: &MultiServiceRouter) -> Value {
    let (status, created) = call(
        router,
        "CreateWorkGroup",
        &json!({
            "Name": WORK_GROUP,
            "Description": "the suite's workgroup",
            "Configuration": baseline_configuration(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the suite's workgroup must be created, got {status} with {created:?}"
    );
    stored(router, WORK_GROUP).await
}

/// The `WorkGroup` `GetWorkGroup` reports for `name`.
async fn stored(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, body) = call(router, "GetWorkGroup", &json!({ "WorkGroup": name })).await;
    assert_eq!(
        status, 200,
        "GetWorkGroup({name}) must answer 200, got {status} with {body:?}"
    );
    body["WorkGroup"].clone()
}

/// Apply one `UpdateWorkGroup` body to [`WORK_GROUP`] and return `(status, body)`.
async fn update(router: &MultiServiceRouter, mut body: Value) -> (u16, Value) {
    body["WorkGroup"] = json!(WORK_GROUP);
    call(router, "UpdateWorkGroup", &body).await
}

/// Set or remove the member `pointer` names in a JSON document, creating the objects
/// along the way: a row's expectation is the baseline read with only the members it
/// names changed, which is what "everything the request leaves out stands" means.
fn change(document: &mut Value, pointer: &str, value: Option<Value>) {
    let mut names: Vec<&str> = pointer.split('/').filter(|part| !part.is_empty()).collect();
    let last = names.pop().expect("a pointer names at least one member");
    let mut cursor = document;
    for name in names {
        if !cursor[name].is_object() {
            cursor[name] = json!({});
        }
        cursor = cursor.get_mut(name).expect("the member was just created");
    }
    match value {
        Some(value) => cursor[last] = value,
        None => {
            if let Some(object) = cursor.as_object_mut() {
                object.remove(last);
            }
        }
    }
}

/// One row of the member-by-member table: the update to send, and the members of the
/// read-back record it must change. Everything else must read exactly as it did before.
struct UpdateCase {
    case: &'static str,
    request: Value,
    changes: Vec<(&'static str, Option<Value>)>,
}

/// The rows: each names a different member of the update shape — a scalar, a nested
/// result-configuration member, a whole sub-structure, a member the stored
/// configuration does not hold yet — and half of them REMOVE rather than set, which is
/// the other half of the rule.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "the description alone",
            request: json!({ "Description": "updated description" }),
            changes: vec![("/Description", Some(json!("updated description")))],
        },
        UpdateCase {
            case: "the state alone",
            request: json!({ "State": WORK_GROUP_STATE_DISABLED }),
            changes: vec![("/State", Some(json!(WORK_GROUP_STATE_DISABLED)))],
        },
        UpdateCase {
            case: "a boolean the configuration already holds",
            request: json!({
                "ConfigurationUpdates": { "EnforceWorkGroupConfiguration": true },
            }),
            changes: vec![(
                "/Configuration/EnforceWorkGroupConfiguration",
                Some(json!(true)),
            )],
        },
        UpdateCase {
            case: "two booleans at once",
            request: json!({
                "ConfigurationUpdates": {
                    "PublishCloudWatchMetricsEnabled": true,
                    "RequesterPaysEnabled": true,
                },
            }),
            changes: vec![
                (
                    "/Configuration/PublishCloudWatchMetricsEnabled",
                    Some(json!(true)),
                ),
                ("/Configuration/RequesterPaysEnabled", Some(json!(true))),
            ],
        },
        UpdateCase {
            case: "the bytes-scanned cutoff raised",
            request: json!({
                "ConfigurationUpdates": { "BytesScannedCutoffPerQuery": CUTOFF_RAISED },
            }),
            changes: vec![(
                "/Configuration/BytesScannedCutoffPerQuery",
                Some(json!(CUTOFF_RAISED)),
            )],
        },
        UpdateCase {
            case: "the bytes-scanned cutoff removed",
            request: json!({
                "ConfigurationUpdates": { "RemoveBytesScannedCutoffPerQuery": true },
            }),
            changes: vec![("/Configuration/BytesScannedCutoffPerQuery", None)],
        },
        UpdateCase {
            case: "one nested result-configuration member set",
            request: json!({
                "ConfigurationUpdates": {
                    "ResultConfigurationUpdates": { "OutputLocation": GROUP_OUTPUT_LOCATION },
                },
            }),
            changes: vec![(
                "/Configuration/ResultConfiguration/OutputLocation",
                Some(json!(GROUP_OUTPUT_LOCATION)),
            )],
        },
        UpdateCase {
            case: "one nested result-configuration member removed",
            request: json!({
                "ConfigurationUpdates": {
                    "ResultConfigurationUpdates": { "RemoveExpectedBucketOwner": true },
                },
            }),
            changes: vec![("/Configuration/ResultConfiguration/ExpectedBucketOwner", None)],
        },
        UpdateCase {
            case: "two nested result-configuration members removed at once",
            request: json!({
                "ConfigurationUpdates": {
                    "ResultConfigurationUpdates": {
                        "RemoveEncryptionConfiguration": true,
                        "RemoveAclConfiguration": true,
                    },
                },
            }),
            changes: vec![
                (
                    "/Configuration/ResultConfiguration/EncryptionConfiguration",
                    None,
                ),
                ("/Configuration/ResultConfiguration/AclConfiguration", None),
            ],
        },
        UpdateCase {
            case: "a nested member set while another is removed",
            request: json!({
                "ConfigurationUpdates": {
                    "ResultConfigurationUpdates": {
                        "EncryptionConfiguration": {
                            "EncryptionOption": "SSE_KMS",
                            "KmsKey": "arn:aws:kms:us-east-1:123456789012:key/updated",
                        },
                        "RemoveAclConfiguration": true,
                    },
                },
            }),
            changes: vec![
                (
                    "/Configuration/ResultConfiguration/EncryptionConfiguration",
                    Some(json!({
                        "EncryptionOption": "SSE_KMS",
                        "KmsKey": "arn:aws:kms:us-east-1:123456789012:key/updated",
                    })),
                ),
                ("/Configuration/ResultConfiguration/AclConfiguration", None),
            ],
        },
        UpdateCase {
            case: "every result-configuration member removed",
            request: json!({
                "ConfigurationUpdates": {
                    "ResultConfigurationUpdates": {
                        "RemoveOutputLocation": true,
                        "RemoveEncryptionConfiguration": true,
                        "RemoveExpectedBucketOwner": true,
                        "RemoveAclConfiguration": true,
                    },
                },
            }),
            changes: vec![("/Configuration/ResultConfiguration", None)],
        },
        UpdateCase {
            case: "the customer content encryption configuration removed",
            request: json!({
                "ConfigurationUpdates": {
                    "RemoveCustomerContentEncryptionConfiguration": true,
                },
            }),
            changes: vec![(
                "/Configuration/CustomerContentEncryptionConfiguration",
                None,
            )],
        },
        UpdateCase {
            case: "a member the stored configuration does not hold yet",
            request: json!({
                "ConfigurationUpdates": {
                    "ManagedQueryResultsConfigurationUpdates": { "Enabled": true },
                },
            }),
            changes: vec![(
                "/Configuration/ManagedQueryResultsConfiguration",
                Some(json!({ "Enabled": true })),
            )],
        },
        UpdateCase {
            case: "an open collection the stored configuration does not hold yet",
            request: json!({
                "ConfigurationUpdates": {
                    "EngineConfiguration": {
                        "MaxConcurrentDpus": 20,
                        "SparkProperties": { "spark.sql.shuffle.partitions": "8" },
                    },
                },
            }),
            changes: vec![(
                "/Configuration/EngineConfiguration",
                Some(json!({
                    "MaxConcurrentDpus": 20,
                    "SparkProperties": { "spark.sql.shuffle.partitions": "8" },
                })),
            )],
        },
        UpdateCase {
            case: "a scalar and a nested member and the description together",
            request: json!({
                "Description": "one call, three members",
                "ConfigurationUpdates": {
                    "ExecutionRole": "arn:aws:iam::123456789012:role/athena-updated",
                    "ResultConfigurationUpdates": { "ExpectedBucketOwner": "210987654321" },
                },
            }),
            changes: vec![
                ("/Description", Some(json!("one call, three members"))),
                (
                    "/Configuration/ExecutionRole",
                    Some(json!("arn:aws:iam::123456789012:role/athena-updated")),
                ),
                (
                    "/Configuration/ResultConfiguration/ExpectedBucketOwner",
                    Some(json!("210987654321")),
                ),
            ],
        },
        UpdateCase {
            case: "an update that names no member at all",
            request: json!({}),
            changes: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn an_update_replaces_the_members_it_names_and_leaves_every_other_one_standing() {
    for case in update_cases() {
        // A fresh registry per row, so each row is applied to the same baseline.
        let router = build_router();
        let before = work_group_with_baseline(&router).await;

        let (status, answer) = update(&router, case.request.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: UpdateWorkGroup({}) must be accepted, got {status} with {answer:?}",
            case.case, case.request
        );

        // The record a read must now report: the one read before this call, with only
        // the members the request named changed.
        let mut expected = before.clone();
        for (pointer, value) in &case.changes {
            change(&mut expected, pointer, value.clone());
        }

        let after = stored(&router, WORK_GROUP).await;
        assert_eq!(
            after, expected,
            "case {:?}: after UpdateWorkGroup({}) the stored workgroup must be \
             {expected:?}, got {after:?}",
            case.case, case.request
        );
    }
}

#[tokio::test]
async fn an_update_is_read_back_the_same_way_however_often_it_is_read() {
    let router = build_router();
    work_group_with_baseline(&router).await;
    let (status, answer) = update(
        &router,
        json!({
            "Description": "read me twice",
            "State": WORK_GROUP_STATE_DISABLED,
            "ConfigurationUpdates": { "PublishCloudWatchMetricsEnabled": true },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the update must be accepted, got {status} with {answer:?}"
    );

    let first = stored(&router, WORK_GROUP).await;
    let second = stored(&router, WORK_GROUP).await;
    assert_eq!(
        first, second,
        "a read must not change what the next caller sees: first {first:?}, then {second:?}"
    );
}

/// One row of the state table: the state to update to, and whether a query submitted
/// afterwards runs.
struct StateCase {
    case: &'static str,
    state: &'static str,
    query_runs: bool,
}

#[tokio::test]
async fn the_state_an_update_stores_decides_whether_a_query_may_run_in_the_work_group() {
    // Toggled both ways, twice, so neither direction is a one-way door and the second
    // pass is judged by the state the second update stored rather than the first.
    let cases = [
        StateCase {
            case: "disabled",
            state: WORK_GROUP_STATE_DISABLED,
            query_runs: false,
        },
        StateCase {
            case: "enabled again",
            state: WORK_GROUP_STATE_ENABLED,
            query_runs: true,
        },
        StateCase {
            case: "disabled again",
            state: WORK_GROUP_STATE_DISABLED,
            query_runs: false,
        },
    ];

    let router = build_router();
    work_group_with_baseline(&router).await;
    for case in cases {
        let (status, answer) = update(&router, json!({ "State": case.state })).await;
        assert_eq!(
            status, 200,
            "case {:?}: the state update must be accepted, got {status} with {answer:?}",
            case.case
        );
        let reported = stored(&router, WORK_GROUP).await["State"].clone();
        assert_eq!(
            reported,
            json!(case.state),
            "case {:?}: the stored state must be {:?}, got {reported:?}",
            case.case,
            case.state
        );

        let (status, body) = call(
            &router,
            "StartQueryExecution",
            &json!({
                "QueryString": "SELECT 1",
                "WorkGroup": WORK_GROUP,
                "ResultConfiguration": { "OutputLocation": CLIENT_OUTPUT_LOCATION },
            }),
        )
        .await;
        if case.query_runs {
            assert_eq!(
                status, 200,
                "case {:?}: a query in an enabled workgroup must be accepted, got \
                 {status} with {body:?}",
                case.case
            );
        } else {
            assert_eq!(
                status, 400,
                "case {:?}: a query in a disabled workgroup must be refused, got \
                 {status} with {body:?}",
                case.case
            );
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {:?}: expected InvalidRequestException, got {body:?}",
                case.case
            );
            assert_eq!(
                body["Message"].as_str(),
                Some(state::work_group_disabled(WORK_GROUP).as_str()),
                "case {:?}: expected the disabled-workgroup sentence, got {body:?}",
                case.case
            );
        }
    }
}

/// One row of the enforcement table: whether the update enforces the workgroup's own
/// result configuration, and the output location a query must then report.
struct EnforcementCase {
    case: &'static str,
    enforce: bool,
    client_names_a_location: bool,
    expected_location: &'static str,
}

#[tokio::test]
async fn the_result_configuration_an_update_stores_is_what_a_query_runs_under() {
    let cases = [
        EnforcementCase {
            case: "enforced, client names its own location",
            enforce: true,
            client_names_a_location: true,
            expected_location: GROUP_OUTPUT_LOCATION,
        },
        EnforcementCase {
            case: "enforced, client names none",
            enforce: true,
            client_names_a_location: false,
            expected_location: GROUP_OUTPUT_LOCATION,
        },
        EnforcementCase {
            case: "not enforced, client names its own location",
            enforce: false,
            client_names_a_location: true,
            expected_location: CLIENT_OUTPUT_LOCATION,
        },
        EnforcementCase {
            case: "not enforced, client names none",
            enforce: false,
            client_names_a_location: false,
            expected_location: GROUP_OUTPUT_LOCATION,
        },
    ];

    for case in cases {
        let router = build_router();
        work_group_with_baseline(&router).await;
        // The workgroup's own location and whether it overrides the client's are both
        // stored by this one update, and nothing else tells the query where to write.
        let (status, answer) = update(
            &router,
            json!({
                "ConfigurationUpdates": {
                    "EnforceWorkGroupConfiguration": case.enforce,
                    "ResultConfigurationUpdates": { "OutputLocation": GROUP_OUTPUT_LOCATION },
                },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the update must be accepted, got {status} with {answer:?}",
            case.case
        );

        let mut request = json!({ "QueryString": "SELECT 1", "WorkGroup": WORK_GROUP });
        if case.client_names_a_location {
            request["ResultConfiguration"] = json!({ "OutputLocation": CLIENT_OUTPUT_LOCATION });
        }
        let (status, started) = call(&router, "StartQueryExecution", &request).await;
        assert_eq!(
            status, 200,
            "case {:?}: the query must be accepted, got {status} with {started:?}",
            case.case
        );
        let id = started["QueryExecutionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: expected an id, got {started:?}", case.case))
            .to_string();

        let (status, body) = call(
            &router,
            "GetQueryExecution",
            &json!({ "QueryExecutionId": id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: GetQueryExecution must answer 200, got {status} with {body:?}",
            case.case
        );
        let reported = body["QueryExecution"]["ResultConfiguration"]["OutputLocation"]
            .as_str()
            .map(str::to_string);
        assert_eq!(
            reported.as_deref(),
            Some(case.expected_location),
            "case {:?}: the query must run under {:?}, got {reported:?}",
            case.case,
            case.expected_location
        );
    }
}

#[tokio::test]
async fn the_effective_engine_version_is_resolved_on_update_rather_than_echoed() {
    // The rows differ along what the request selects, including a request that tries to
    // set the read-only effective version itself.
    let cases: [(&str, Value, &str, &str); 4] = [
        (
            "AUTO selected",
            json!({ "SelectedEngineVersion": ENGINE_VERSION_AUTO }),
            ENGINE_VERSION_AUTO,
            DEFAULT_EFFECTIVE_ENGINE_VERSION,
        ),
        (
            "an explicit version",
            json!({ "SelectedEngineVersion": "Athena engine version 2" }),
            "Athena engine version 2",
            "Athena engine version 2",
        ),
        (
            "an effective version the caller tried to set",
            json!({
                "SelectedEngineVersion": ENGINE_VERSION_AUTO,
                "EffectiveEngineVersion": "Athena engine version 99",
            }),
            ENGINE_VERSION_AUTO,
            DEFAULT_EFFECTIVE_ENGINE_VERSION,
        ),
        (
            "nothing selected",
            json!({}),
            ENGINE_VERSION_AUTO,
            DEFAULT_EFFECTIVE_ENGINE_VERSION,
        ),
    ];

    for (case, requested, selected, effective) in cases {
        let router = build_router();
        work_group_with_baseline(&router).await;
        let (status, answer) = update(
            &router,
            json!({ "ConfigurationUpdates": { "EngineVersion": requested.clone() } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update must be accepted, got {status} with {answer:?}"
        );

        let reported = stored(&router, WORK_GROUP).await["Configuration"]["EngineVersion"].clone();
        assert_eq!(
            reported,
            json!({
                "SelectedEngineVersion": selected,
                "EffectiveEngineVersion": effective,
            }),
            "case {case:?}: EngineVersion({requested}) must resolve to {selected:?}/\
             {effective:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_update_of_a_work_group_the_registry_does_not_hold_is_refused() {
    let router = build_router();
    work_group_with_baseline(&router).await;
    // The rows differ along how the absent name misses: never created, one character
    // off, differing only in case, and the longest name the modelled `WorkGroupName`
    // pattern admits.
    let cases = [
        "never-created",
        "update-work-group-suit",
        "UPDATE-WORK-GROUP-SUITE",
        &"a".repeat(128),
    ];
    for case in cases {
        let (status, body) = call(
            &router,
            "UpdateWorkGroup",
            &json!({ "WorkGroup": case, "Description": "nothing to update" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an update of an absent workgroup must be refused, got \
             {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {body:?}"
        );
        assert_eq!(
            body["Message"].as_str(),
            Some(state::WORK_GROUP_NOT_FOUND),
            "case {case:?}: expected the not-found sentence, got {body:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_update_leaves_the_stored_work_group_exactly_as_it_was() {
    // One row per way an update is refused: a state outside the model's enum, which the
    // request walk refuses before the handler runs, and a configuration past the byte
    // bound this emulator holds a workgroup to, which the handler refuses itself. The
    // bound is reached with two properties because the model caps one property's value
    // well below it.
    let half: String = "s".repeat(MAX_WORK_GROUP_CONFIGURATION_BYTES / 2);
    let cases: [(&str, Value, &str); 2] = [
        (
            "a state the model does not declare",
            json!({ "State": "SUSPENDED" }),
            "ValidationException",
        ),
        (
            "an open collection past the byte bound",
            json!({
                "ConfigurationUpdates": {
                    "EngineConfiguration": {
                        "SparkProperties": {
                            "spark.sql.extensions": half,
                            "spark.sql.catalogImplementation": half,
                        },
                    },
                },
            }),
            "InvalidRequestException",
        ),
    ];

    for (case, request, expected_code) in cases {
        let router = build_router();
        let before = work_group_with_baseline(&router).await;
        let (status, body) = update(&router, request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the update must be refused, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body:?}"
        );
        let after = stored(&router, WORK_GROUP).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must persist nothing: expected {before:?}, \
             got {after:?}"
        );
    }
}

#[tokio::test]
async fn the_state_values_an_update_accepts_are_the_ones_the_model_declares() {
    // Every member of the model's own `WorkGroupState` enum is accepted, and the stored
    // state is the one the request named — the catalog is the model's, not a list
    // written here.
    for case in WORK_GROUP_STATE_VALUES {
        let router = build_router();
        work_group_with_baseline(&router).await;
        let (status, answer) = update(&router, json!({ "State": case })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the state update must be accepted, got {status} with {answer:?}"
        );
        let reported = stored(&router, WORK_GROUP).await["State"].clone();
        assert_eq!(
            reported,
            json!(case),
            "case {case:?}: the stored state must be {case:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn concurrent_updates_are_each_accepted_and_the_registry_holds_one_of_them() {
    // The rule is that the registry serialises them: every call is answered, and the
    // record left behind is one whole update rather than a mixture of several. The rows
    // differ along how many callers race.
    for racers in [2_usize, 8] {
        let router = Arc::new(build_router());
        work_group_with_baseline(&router).await;

        let mut racing = Vec::new();
        for racer in 0..racers {
            let router = Arc::clone(&router);
            racing.push(tokio::spawn(async move {
                let body = json!({
                    "WorkGroup": WORK_GROUP,
                    "Description": format!("racer {racer}"),
                    "ConfigurationUpdates": {
                        "ResultConfigurationUpdates": {
                            "OutputLocation": format!("s3://athena-results/racer-{racer}/"),
                        },
                    },
                });
                call(&router, "UpdateWorkGroup", &body).await
            }));
        }

        let mut accepted = 0usize;
        for (racer, task) in racing.into_iter().enumerate() {
            let (status, body) = task.await.expect("the update task runs to completion");
            assert_eq!(
                status, 200,
                "case {racers} racers: racer {racer} must be accepted, got {status} \
                 with {body:?}"
            );
            accepted += 1;
        }
        assert_eq!(
            accepted, racers,
            "case {racers} racers: every concurrent update must be accepted, {accepted} were"
        );

        // The record holds ONE racer's update whole: the description and the output
        // location came from the same call, not from two different ones.
        let after = stored(&router, WORK_GROUP).await;
        let description = after["Description"].as_str().unwrap_or_default().to_string();
        let location = after["Configuration"]["ResultConfiguration"]["OutputLocation"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let winner = description
            .strip_prefix("racer ")
            .unwrap_or_else(|| panic!("case {racers} racers: expected a racer's description, got {description:?}"))
            .to_string();
        assert_eq!(
            location,
            format!("s3://athena-results/racer-{winner}/"),
            "case {racers} racers: the stored record must come whole from racer \
             {winner:?}, got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_query_started_before_a_work_group_is_disabled_still_completes() {
    // Disabling a workgroup refuses the NEXT submission; it is not a cancellation of
    // work already running, which no documented behaviour of this operation performs.
    let router = build_router();
    work_group_with_baseline(&router).await;
    let (status, started) = call(
        &router,
        "StartQueryExecution",
        &json!({
            "QueryString": "SELECT 1",
            "WorkGroup": WORK_GROUP,
            "ResultConfiguration": { "OutputLocation": CLIENT_OUTPUT_LOCATION },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the query must be accepted, got {status} with {started:?}"
    );
    let id = started["QueryExecutionId"]
        .as_str()
        .expect("the start answers with an id")
        .to_string();

    let (status, answer) = update(&router, json!({ "State": WORK_GROUP_STATE_DISABLED })).await;
    assert_eq!(
        status, 200,
        "the disabling update must be accepted, got {status} with {answer:?}"
    );

    tokio::time::sleep(QUERY_EXECUTION_COMPLETION + SETTLE_GRACE).await;
    let (status, body) = call(
        &router,
        "GetQueryExecution",
        &json!({ "QueryExecutionId": id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetQueryExecution must answer 200, got {status} with {body:?}"
    );
    let reported = body["QueryExecution"]["Status"]["State"]
        .as_str()
        .map(str::to_string);
    assert_eq!(
        reported.as_deref(),
        Some(state::QUERY_STATE_SUCCEEDED),
        "the query submitted before the workgroup was disabled must still succeed, got \
         {reported:?}"
    );
}

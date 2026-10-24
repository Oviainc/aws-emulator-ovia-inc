// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeDefaultParameters`: the DAX caching software's
//! system-defined parameter catalog, over the wire.
//!
//! Every case drives `POST /` with `X-Amz-Target: AmazonDAXV3.DescribeDefaultParameters`
//! through the same `MultiServiceRouter` the production router builds, so what is
//! asserted is the HTTP behaviour a DAX SDK client sees — not an internal function call.
//!
//! The tables below span the CLASS each rule covers: every parameter the catalog
//! defines (no single cited row), each published `MaxResults` bound plus one inside and
//! one past each, each form of continuation token, and the catalog resumed at every one
//! of its names.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_dax_amazondaxv3::ops::types::{
    CHANGE_TYPE_VALUES, IS_MODIFIABLE_VALUES, PARAMETER_TYPE_VALUES,
};
use service_com_amazonaws_dax_amazondaxv3::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// A role ARN to pass as `CreateCluster`'s required `IamRoleArn`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/DAXRole";

/// A node type to pass as `CreateCluster`'s required `NodeType`.
const NODE_TYPE: &str = "dax.r4.large";

/// A router carrying only this service, over its own engine, so each test starts from a
/// fresh store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return its HTTP status and parsed JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AmazonDAXV3.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/dax/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
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

/// The wire error code of a failed call: the `__type` member, without any namespace.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The reported parameters, keyed by `ParameterName`.
fn by_name(body: &Value) -> BTreeMap<String, Value> {
    body["Parameters"]
        .as_array()
        .map(|parameters| {
            parameters
                .iter()
                .map(|parameter| (reported_name(parameter), parameter.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// The `ParameterName` one reported parameter carries.
fn reported_name(parameter: &Value) -> String {
    parameter["ParameterName"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `ParameterName`s the response lists, in wire order.
fn reported_names(body: &Value) -> Vec<String> {
    body["Parameters"]
        .as_array()
        .map(|parameters| parameters.iter().map(reported_name).collect())
        .unwrap_or_default()
}

/// A continuation token as this service encodes one: base64 of the name a page resumes
/// at (`state::page` / `state::decode_token`).
fn token_for(name: &str) -> String {
    aws_smithy_types::base64::encode(name)
}

// ---------------------------------------------------------------------------
// Rule: the operation reports EVERY parameter the DAX software defines, each rendered
// from this service's one catalog (`state::DEFAULT_PARAMETERS`) with the value the
// system defines for it.
// Source: `DescribeDefaultParameters` ("Returns the default system parameter
// information for the DAX caching software") and `Parameter`'s members.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_defined_parameter_is_reported_from_the_catalog() {
    let router = emulator();
    let (status, body) = call(&router, "DescribeDefaultParameters", json!({})).await;
    assert_eq!(status, 200, "DescribeDefaultParameters: got {body:?}");

    let reported = by_name(&body);
    assert_eq!(
        reported.len(),
        state::DEFAULT_PARAMETERS.len(),
        "expected all {} defined parameters, got {body:?}",
        state::DEFAULT_PARAMETERS.len()
    );

    // Rows: every entry of the catalog. Each is asserted member by member against its
    // definition, so no parameter is reported with another's rendering.
    for definition in state::DEFAULT_PARAMETERS {
        let case = definition.name;
        let parameter = reported
            .get(case)
            .unwrap_or_else(|| panic!("case {case}: expected it reported, got {body:?}"));
        assert_eq!(
            parameter["ParameterValue"],
            json!(definition.default_value),
            "case {case}: expected the system default {:?}, got {parameter:?}",
            definition.default_value
        );
        assert_eq!(
            parameter["Description"],
            json!(definition.description),
            "case {case}: expected the catalog description, got {parameter:?}"
        );
        assert_eq!(
            parameter["DataType"],
            json!(definition.data_type),
            "case {case}: expected DataType {:?}, got {parameter:?}",
            definition.data_type
        );
        assert_eq!(
            parameter["AllowedValues"],
            json!(definition.allowed_values),
            "case {case}: expected AllowedValues {:?}, got {parameter:?}",
            definition.allowed_values
        );
        assert_eq!(
            parameter["IsModifiable"],
            json!(definition.is_modifiable),
            "case {case}: expected IsModifiable {:?}, got {parameter:?}",
            definition.is_modifiable
        );
        assert_eq!(
            parameter["ChangeType"],
            json!(definition.change_type),
            "case {case}: expected ChangeType {:?}, got {parameter:?}",
            definition.change_type
        );
        assert_eq!(
            parameter["Source"],
            json!(definition.source),
            "case {case}: expected the Source its catalog entry publishes, got {parameter:?}"
        );
        assert!(
            state::PARAMETER_SOURCES.contains(&definition.source),
            "case {case}: expected a Source from {:?}, got {:?}",
            state::PARAMETER_SOURCES,
            definition.source
        );
        assert_eq!(
            parameter["ParameterType"],
            json!(state::PARAMETER_TYPE_DEFAULT),
            "case {case}: a parameter with no per-node-type value is DEFAULT, got {parameter:?}"
        );
        assert_eq!(
            parameter["NodeTypeSpecificValues"],
            json!([]),
            "case {case}: expected no node-type-specific values, got {parameter:?}"
        );
    }
}

/// Rule: a reported `Parameter`'s `Description` and `Source` are properties of the
/// CATALOG ENTRY it is rendered from — carried through verbatim — not values chosen
/// where the parameter is projected. Rows span both `Source` values the member's
/// documentation names and descriptions of different shapes, so a projection that
/// substituted a constant for either would fail on at least one row.
///
/// Source: `Parameter$Source` ("How the parameter is defined. For example, system
/// denotes a system-defined parameter.") and `Parameter$Description`, via
/// `state::ParameterDefinition::default_view` — the one projection every parameter view
/// in this service goes through.
#[test]
fn a_parameter_is_projected_with_its_own_catalog_description_and_source() {
    let rows = [
        ("system-defined", state::PARAMETER_SOURCE_SYSTEM, "Set by the service"),
        ("user-defined", state::PARAMETER_SOURCE_USER, "Set by the parameter group"),
        (
            "a description with punctuation",
            state::PARAMETER_SOURCE_USER,
            "Duration in milliseconds (Default: 0 = infinite)",
        ),
    ];

    for (case, source, description) in rows {
        let definition = state::ParameterDefinition {
            name: "a-parameter",
            default_value: "1",
            description,
            source,
            data_type: state::DATA_TYPE_INTEGER,
            allowed_values: "0-",
            is_modifiable: state::IS_MODIFIABLE_TRUE,
            change_type: state::CHANGE_TYPE_IMMEDIATE,
        };
        let view = definition.default_view();
        assert_eq!(
            view.source.as_deref(),
            Some(source),
            "case {case}: expected the entry's own Source {source:?}, got {:?}",
            view.source
        );
        assert_eq!(
            view.description.as_deref(),
            Some(description),
            "case {case}: expected the entry's own Description {description:?}, got {:?}",
            view.description
        );
    }

    // And every entry of the real catalog publishes a `Source` from the documented
    // vocabulary, so none is rendered with a value outside it.
    for definition in state::DEFAULT_PARAMETERS {
        assert!(
            state::PARAMETER_SOURCES.contains(&definition.source),
            "case {}: expected a Source from {:?}, got {:?}",
            definition.name,
            state::PARAMETER_SOURCES,
            definition.source
        );
    }
}

/// Rule (invariant over the whole catalog): every reported parameter carries values the
/// model admits — a `ParameterType`/`IsModifiable`/`ChangeType` from its enum, a
/// non-empty name and description, and a value that parses as its own `DataType` and
/// sits inside its own `AllowedValues` range.
///
/// Sources: the `ParameterType`, `IsModifiable` and `ChangeType` enums as
/// `src/ops/types/shared_types.rs` re-exports them from the model, and
/// `Parameter$AllowedValues` ("A range of values within which the parameter can be
/// set").
#[tokio::test]
async fn every_reported_parameter_satisfies_the_modelled_value_sets() {
    let router = emulator();
    let (status, body) = call(&router, "DescribeDefaultParameters", json!({})).await;
    assert_eq!(status, 200, "DescribeDefaultParameters: got {body:?}");

    let parameters = body["Parameters"]
        .as_array()
        .expect("Parameters is a list")
        .clone();
    assert!(
        !parameters.is_empty(),
        "the catalog must not be empty, got {body:?}"
    );

    for parameter in &parameters {
        let case = reported_name(parameter);
        assert!(
            !case.is_empty(),
            "case {parameter:?}: every parameter must report a name"
        );
        assert!(
            !parameter["Description"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {case}: expected a description, got {parameter:?}"
        );
        let parameter_type = parameter["ParameterType"].as_str().unwrap_or_default();
        assert!(
            PARAMETER_TYPE_VALUES.contains(&parameter_type),
            "case {case}: expected a ParameterType from {PARAMETER_TYPE_VALUES:?}, \
             got {parameter_type:?}"
        );
        let is_modifiable = parameter["IsModifiable"].as_str().unwrap_or_default();
        assert!(
            IS_MODIFIABLE_VALUES.contains(&is_modifiable),
            "case {case}: expected an IsModifiable from {IS_MODIFIABLE_VALUES:?}, \
             got {is_modifiable:?}"
        );
        let change_type = parameter["ChangeType"].as_str().unwrap_or_default();
        assert!(
            CHANGE_TYPE_VALUES.contains(&change_type),
            "case {case}: expected a ChangeType from {CHANGE_TYPE_VALUES:?}, \
             got {change_type:?}"
        );
        let source = parameter["Source"].as_str().unwrap_or_default();
        assert!(
            state::PARAMETER_SOURCES.contains(&source),
            "case {case}: expected a Source from {:?}, got {source:?}",
            state::PARAMETER_SOURCES
        );

        // A value is reported in its own data type and inside its own allowed range: an
        // `integer` parameter parses as a whole number, and an open-ended `<min>-` range
        // is satisfied by a value at or above `min`.
        let value = parameter["ParameterValue"].as_str().unwrap_or_default();
        if parameter["DataType"] == json!(state::DATA_TYPE_INTEGER) {
            let parsed = value.parse::<i64>();
            assert!(
                parsed.is_ok(),
                "case {case}: an integer parameter must report a whole number, got {value:?}"
            );
            let allowed = parameter["AllowedValues"].as_str().unwrap_or_default();
            if let Some(min) = allowed
                .strip_suffix('-')
                .and_then(|min| min.parse::<i64>().ok())
            {
                let parsed = parsed.unwrap_or_default();
                assert!(
                    parsed >= min,
                    "case {case}: expected a value at or above {min}, got {parsed}"
                );
            }
        }
    }

    // The catalog pages in the one stable order `state::page` needs: sorted by name and
    // free of duplicates.
    let names = reported_names(&body);
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        names, sorted,
        "the catalog must page in sorted, duplicate-free name order, got {names:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule: the paginated default-parameter read accepts a `MaxResults` inside the
// documented 20..=100 window and refuses anything outside it with the modelled
// `InvalidParameterValueException` (HTTP 400).
// Source: `DescribeDefaultParametersRequest$MaxResults` ("The value for MaxResults
// must be between 20 and 100").
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_documented_max_results_window_is_honoured() {
    let router = emulator();
    // Rows: each published bound, one inside each bound, and one past each bound.
    let accepted = [
        state::MIN_MAX_RESULTS,
        state::MIN_MAX_RESULTS + 1,
        state::MAX_MAX_RESULTS - 1,
        state::MAX_MAX_RESULTS,
    ];
    let refused = [
        state::MIN_MAX_RESULTS - 1,
        1,
        0,
        -1,
        state::MAX_MAX_RESULTS + 1,
        1000,
    ];

    for max_results in accepted {
        let (status, body) = call(
            &router,
            "DescribeDefaultParameters",
            json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case MaxResults={max_results}: expected it accepted, got {status} {body:?}"
        );
        assert_eq!(
            by_name(&body).len(),
            state::DEFAULT_PARAMETERS.len(),
            "case MaxResults={max_results}: the catalog fits in one page this size, got {body:?}"
        );
        assert!(
            body["NextToken"].is_null(),
            "case MaxResults={max_results}: a complete page carries no token, got {body:?}"
        );
    }

    for max_results in refused {
        let (status, body) = call(
            &router,
            "DescribeDefaultParameters",
            json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case MaxResults={max_results}: expected a refusal, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case MaxResults={max_results}: expected InvalidParameterValueException, got {body:?}"
        );
        assert_eq!(
            body["message"],
            json!(state::max_results_out_of_range_message()),
            "case MaxResults={max_results}: expected the fault's own template, got {body:?}"
        );
    }
}

/// Rule: a `NextToken` this service did not issue is refused with HTTP 400, never
/// answered with some arbitrary page and a 200.
#[tokio::test]
async fn a_continuation_token_this_service_did_not_issue_is_refused() {
    let router = emulator();
    // Rows: not base64, base64 of non-UTF-8 bytes, base64 of the empty string.
    let tokens = ["not-a-token!!", "/w==", ""];

    for token in tokens {
        let (status, body) = call(
            &router,
            "DescribeDefaultParameters",
            json!({ "MaxResults": state::MIN_MAX_RESULTS, "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case NextToken={token:?}: expected a refusal, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case NextToken={token:?}: expected InvalidParameterValueException, got {body:?}"
        );
    }
}

/// Rule: a continuation token resumes the catalog AT the name it carries — the page
/// holds the parameters at or after it, and a token past the last parameter yields an
/// empty final page with no further token.
#[tokio::test]
async fn a_continuation_token_resumes_the_catalog_at_its_name() {
    let router = emulator();
    let all = state::default_parameter_names();

    // Rows: resume at each parameter in the catalog, so the first row is the whole
    // catalog and the last row is a single parameter.
    for (index, name) in all.iter().enumerate() {
        let (status, body) = call(
            &router,
            "DescribeDefaultParameters",
            json!({ "NextToken": token_for(name) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case NextToken={name}: expected it accepted, got {status} {body:?}"
        );
        let expected: Vec<String> = all[index..]
            .iter()
            .map(|name| (*name).to_string())
            .collect();
        assert_eq!(
            reported_names(&body),
            expected,
            "case NextToken={name}: expected the catalog from {name} on, got {body:?}"
        );
    }

    // And one row past the end: a name sorting after every parameter is an empty page
    // with no further token.
    let (status, body) = call(
        &router,
        "DescribeDefaultParameters",
        json!({ "NextToken": token_for("zzz-past-the-end") }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case NextToken past the end: expected it accepted, got {status} {body:?}"
    );
    assert_eq!(
        reported_names(&body),
        Vec::<String>::new(),
        "case NextToken past the end: expected an empty page, got {body:?}"
    );
    assert!(
        body["NextToken"].is_null(),
        "case NextToken past the end: expected no further token, got {body:?}"
    );
}

/// Rule: the read reports the same system catalog however often it is called and
/// whatever else the store holds — it is a report, so it changes nothing a later caller
/// sees.
#[tokio::test]
async fn the_read_is_repeatable_and_independent_of_the_rest_of_the_store() {
    let router = emulator();
    let (status, first) = call(&router, "DescribeDefaultParameters", json!({})).await;
    assert_eq!(status, 200, "first read: got {first:?}");

    // Other state this service persists, created between the reads: a parameter group
    // of its own and a cluster.
    let (status, created) = call(
        &router,
        "CreateParameterGroup",
        json!({ "ParameterGroupName": "params", "Description": "a group of its own" }),
    )
    .await;
    assert_eq!(status, 200, "CreateParameterGroup: got {created:?}");
    let (status, created) = call(
        &router,
        "CreateCluster",
        json!({
            "ClusterName": "params",
            "NodeType": NODE_TYPE,
            "ReplicationFactor": 1,
            "IamRoleArn": ROLE_ARN,
            "ParameterGroupName": "params",
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateCluster: got {created:?}");

    // Rows: the same read repeated, the same read after that other state exists, and
    // the same read with an explicit page size.
    for (case, body) in [
        ("repeated", json!({})),
        ("after other state", json!({})),
        (
            "explicit page size",
            json!({ "MaxResults": state::MAX_MAX_RESULTS }),
        ),
    ] {
        let (status, again) = call(&router, "DescribeDefaultParameters", body).await;
        assert_eq!(status, 200, "case {case}: got {again:?}");
        assert_eq!(
            again, first,
            "case {case}: expected the same catalog as the first read {first:?}, got {again:?}"
        );
    }
}

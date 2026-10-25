// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the stage VARIABLES an `UpdateStage` patch writes:
//! `PATCH /restapis/{restApiId}/stages/{stageName}` with an op whose path names
//! `/variables/<name>`.
//!
//! This is what a deployment workflow drives when it feeds a value another service
//! produced — a function ARN, an endpoint url, a table name — into a variable of an
//! already deployed stage: a single `op=replace, path=/variables/<name>` patch, followed
//! by a read of the stage.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the PATCH builder and the
//! `CreateRestApi` / `CreateDeployment` / `GetStage` helpers are this crate's existing
//! harness, reused from `rest_api_lifecycle`, `patch_updates` and `stage_lifecycle`.
//!
//! The RULE under test, asserted over a TABLE of inputs rather than one recorded call:
//! `UpdateStage` applies every entry of `patchOperations` to the stored stage and answers
//! the modelled HTTP 200 carrying the updated `Stage`, so an op naming `/variables/<name>`
//! sets (`add`, `replace`) or drops (`remove`) exactly that one entry of the map for any
//! value the member's documented value set admits, leaves the stage's other variables
//! alone, and is read back unchanged by `GetStage` — and a `/variables` op the service
//! cannot apply is refused with one of the operation's MODELLED errors, leaving the stored
//! map as it was.
//!
//! The sibling `update_stage` suite holds the per-member patch tables for the rest of the
//! `Stage` shape; this suite is only the variables map, over the forms of value and the
//! op/entry combinations a workflow sends.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::MAX_STRING_CHARS;

use crate::patch_updates::patch;
use crate::rest_api_lifecycle::{assert_failure, create_api, Gateway};
use crate::stage_lifecycle::{create_deployment, get_stage};

/// One row: the case name, the variables the stage carries BEFORE the patch, the ops the
/// row sends, and the whole `variables` map the stage must report afterwards (`Null` when
/// no variable is left, which is how this service reports an empty map).
type Row = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    Vec<Value>,
    Value,
);

/// Deploy a fresh API to `stage` carrying `variables`, and answer the API id — the
/// starting point every row patches.
async fn deployed_stage(
    gateway: &Gateway,
    case: &str,
    stage: &str,
    variables: &[(&str, &str)],
) -> String {
    let (api, _) = create_api(gateway, case).await;
    let mut request = json!({ "stageName": stage });
    if !variables.is_empty() {
        let carried: Value = variables
            .iter()
            .map(|(name, value)| ((*name).to_string(), Value::String((*value).to_string())))
            .collect::<serde_json::Map<String, Value>>()
            .into();
        request["variables"] = carried;
    }
    create_deployment(gateway, &api, case, request).await;
    api
}

/// The `UpdateStage` URI for `stage` of `api`.
fn stage_uri(api: &str, stage: &str) -> String {
    format!("/restapis/{api}/stages/{stage}")
}

#[tokio::test]
async fn a_variables_patch_sets_the_named_entry_and_the_stage_reads_back() {
    // Rows differ along what the rule generalises over: the FORM of the value (an ARN,
    // an endpoint url, a comma-separated list, the punctuation of the value set, a single
    // character), whether the name patched already has a value, how many ops the one call
    // carries, which ops they are, and how the names are spelled. The values stay inside
    // the set `Stage$variables` documents for a stage variable — alphanumeric and
    // underscore names, values matching `[A-Za-z0-9-._~:/?#&=,]+`
    // (smithy_query(query="doc", shape="Stage", member="variables")).
    let rows: Vec<Row> = vec![
        (
            "a function ARN written into a name the stage has no variable for",
            vec![],
            vec![json!({
                "op": "replace",
                "path": "/variables/lambdaArn",
                "value": "arn:aws:lambda:us-east-1:000000000000:function:orders-handler",
            })],
            json!({ "lambdaArn": "arn:aws:lambda:us-east-1:000000000000:function:orders-handler" }),
        ),
        (
            "a replace over a name that already has a value",
            vec![("lambdaArn", "arn:aws:lambda:us-east-1:000000000000:function:old")],
            vec![json!({
                "op": "replace",
                "path": "/variables/lambdaArn",
                "value": "arn:aws:lambda:us-east-1:000000000000:function:new",
            })],
            json!({ "lambdaArn": "arn:aws:lambda:us-east-1:000000000000:function:new" }),
        ),
        (
            "an endpoint url carrying slashes, a query and a fragment",
            vec![],
            vec![json!({
                "op": "add",
                "path": "/variables/endpoint",
                "value": "https://orders.example.com/v1/orders?limit=10&page=2#top",
            })],
            json!({ "endpoint": "https://orders.example.com/v1/orders?limit=10&page=2#top" }),
        ),
        (
            "a comma-separated list value",
            vec![],
            vec![json!({ "op": "add", "path": "/variables/regions", "value": "us-east-1,us-west-2" })],
            json!({ "regions": "us-east-1,us-west-2" }),
        ),
        (
            "every punctuation character the value set admits",
            vec![],
            vec![json!({ "op": "replace", "path": "/variables/opaque", "value": "v1-._~:/?#&=,9" })],
            json!({ "opaque": "v1-._~:/?#&=,9" }),
        ),
        (
            "a single-character value",
            vec![],
            vec![json!({ "op": "replace", "path": "/variables/stage", "value": "a" })],
            json!({ "stage": "a" }),
        ),
        (
            "a name spelled with underscores and digits",
            vec![],
            vec![json!({
                "op": "replace",
                "path": "/variables/lambda_arn_2",
                "value": "arn:aws:lambda:eu-west-1:000000000000:function:handler-2",
            })],
            json!({ "lambda_arn_2": "arn:aws:lambda:eu-west-1:000000000000:function:handler-2" }),
        ),
        (
            "one name set beside the variables the stage was deployed with",
            vec![("table", "pets"), ("region", "us-east-1")],
            vec![json!({
                "op": "replace",
                "path": "/variables/lambdaArn",
                "value": "arn:aws:lambda:us-east-1:000000000000:function:pets",
            })],
            json!({
                "table": "pets",
                "region": "us-east-1",
                "lambdaArn": "arn:aws:lambda:us-east-1:000000000000:function:pets",
            }),
        ),
        (
            "three names set in one call",
            vec![],
            vec![
                json!({ "op": "add", "path": "/variables/table", "value": "pets" }),
                json!({ "op": "add", "path": "/variables/region", "value": "us-west-2" }),
                json!({ "op": "replace", "path": "/variables/queueUrl",
                        "value": "https://sqs.us-west-2.amazonaws.com/000000000000/orders" }),
            ],
            json!({
                "table": "pets",
                "region": "us-west-2",
                "queueUrl": "https://sqs.us-west-2.amazonaws.com/000000000000/orders",
            }),
        ),
        (
            "one name dropped and another set in the same call",
            vec![("table", "pets"), ("stale", "yes")],
            vec![
                json!({ "op": "remove", "path": "/variables/stale" }),
                json!({ "op": "replace", "path": "/variables/table", "value": "pets-v2" }),
            ],
            json!({ "table": "pets-v2" }),
        ),
        (
            "a name set and dropped again within one call",
            vec![],
            vec![
                json!({ "op": "add", "path": "/variables/temp", "value": "x" }),
                json!({ "op": "remove", "path": "/variables/temp" }),
            ],
            Value::Null,
        ),
        (
            "the stage's only variable removed",
            vec![("table", "pets")],
            vec![json!({ "op": "remove", "path": "/variables/table" })],
            Value::Null,
        ),
        (
            "a remove of a name the stage has no variable for leaves the map alone",
            vec![("table", "pets")],
            vec![json!({ "op": "remove", "path": "/variables/absent" })],
            json!({ "table": "pets" }),
        ),
    ];

    let gateway = Gateway::new();
    for (index, (case, before, ops, expected)) in rows.iter().enumerate() {
        let stage = format!("prod_{index}");
        let api = deployed_stage(&gateway, case, &stage, before).await;

        let answer = patch(&gateway, &stage_uri(&api, &stage), ops.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the modelled 200 from UpdateStage, got {} ({:?}) with {:?}",
            answer.status, answer.error, answer.body
        );
        assert_eq!(
            &answer.body["variables"], expected,
            "case {case:?}: UpdateStage reported variables {:?}, expected {expected:?}",
            answer.body["variables"]
        );

        let read = get_stage(&gateway, &api, &stage).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetStage, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            &read.body["variables"], expected,
            "case {case:?}: GetStage read back variables {:?}, expected {expected:?}",
            read.body["variables"]
        );
        assert_eq!(
            read.body["stageName"],
            json!(stage),
            "case {case:?}: GetStage answered stage {:?}, expected {stage:?}",
            read.body["stageName"]
        );
    }
}

#[tokio::test]
async fn a_variable_value_is_stored_up_to_the_length_bound_and_refused_past_it() {
    // The bound is this emulator's own stored-string guard, exported by the service crate
    // as `state::MAX_STRING_CHARS`, so the rows are computed from that constant rather
    // than written out here: one inside it, each published bound, and one past it.
    let lengths = [1, MAX_STRING_CHARS - 1, MAX_STRING_CHARS, MAX_STRING_CHARS + 1];
    let gateway = Gateway::new();
    for (index, length) in lengths.iter().enumerate() {
        let case = format!("a value of {length} characters");
        let stage = format!("bounded_{index}");
        let api = deployed_stage(&gateway, &case, &stage, &[("table", "pets")]).await;
        let value = "v".repeat(*length);
        let ops = vec![json!({
            "op": "replace",
            "path": "/variables/lambdaArn",
            "value": value,
        })];

        let answer = patch(&gateway, &stage_uri(&api, &stage), ops).await;
        let expected: Value = if *length > MAX_STRING_CHARS {
            json!({ "table": "pets" })
        } else {
            json!({ "table": "pets", "lambdaArn": value })
        };
        if *length > MAX_STRING_CHARS {
            assert_failure(&answer, 400, "BadRequestException", &case);
        } else {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the modelled 200, got {} ({:?}) with {:?}",
                answer.status, answer.error, answer.body
            );
        }

        let read = get_stage(&gateway, &api, &stage).await;
        assert_eq!(
            &read.body["variables"], &expected,
            "case {case:?}: GetStage read back variables {:?}, expected {expected:?}",
            read.body["variables"]
        );
    }
}

#[tokio::test]
async fn a_variables_op_the_service_cannot_apply_is_a_modelled_refusal_that_changes_nothing() {
    // Rows differ along WHY the op cannot be applied: an op outside the three a resource
    // of this service supports, a path that names the map without naming one key, a set
    // with no value to set, and a stage that does not exist. Each refusal must be one of
    // `UpdateStage`'s modelled errors — the status and `x-amzn-errortype` a client
    // branches on — rather than an unmodelled one.
    let rows: Vec<(&str, &str, Vec<Value>, u16, &str)> = vec![
        (
            "a move between two variable names",
            "live",
            vec![json!({ "op": "move", "from": "/variables/table", "path": "/variables/other" })],
            400,
            "BadRequestException",
        ),
        (
            "a copy between two variable names",
            "live",
            vec![json!({ "op": "copy", "from": "/variables/table", "path": "/variables/other" })],
            400,
            "BadRequestException",
        ),
        (
            "a test of a variable's value",
            "live",
            vec![json!({ "op": "test", "path": "/variables/table", "value": "pets" })],
            400,
            "BadRequestException",
        ),
        (
            "the map named without one key",
            "live",
            vec![json!({ "op": "replace", "path": "/variables", "value": "pets" })],
            400,
            "BadRequestException",
        ),
        (
            "a replace carrying no value to set",
            "live",
            vec![json!({ "op": "replace", "path": "/variables/table" })],
            400,
            "BadRequestException",
        ),
        (
            "a supported op applied after an unsupported one in the same call",
            "live",
            vec![
                json!({ "op": "replace", "path": "/variables/region", "value": "us-west-2" }),
                json!({ "op": "move", "from": "/variables/table", "path": "/variables/other" }),
            ],
            400,
            "BadRequestException",
        ),
        (
            "a variable written to a stage this API does not have",
            "absent",
            vec![json!({ "op": "replace", "path": "/variables/table", "value": "pets" })],
            404,
            "NotFoundException",
        ),
    ];

    let gateway = Gateway::new();
    for (index, (case, target, ops, status, code)) in rows.iter().enumerate() {
        let stage = format!("live_{index}");
        let api = deployed_stage(&gateway, case, &stage, &[("table", "pets")]).await;
        let addressed = if *target == "live" {
            stage.clone()
        } else {
            format!("no_such_{index}")
        };

        let answer = patch(&gateway, &stage_uri(&api, &addressed), ops.clone()).await;
        assert_failure(&answer, *status, code, case);

        // A refused list leaves the stored map exactly as the stage was deployed with it,
        // including the entry an op EARLIER in the same list would have set.
        let read = get_stage(&gateway, &api, &stage).await;
        assert_eq!(
            &read.body["variables"],
            &json!({ "table": "pets" }),
            "case {case:?}: a refused patch changed the stored variables to {:?}",
            read.body["variables"]
        );
    }
}

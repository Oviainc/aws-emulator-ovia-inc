// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests pinning what `UpdateAction` does with an `ActionName` the store
//! no longer holds, and what a later `CreateAction` under that name reads back.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The RULE these tests assert, over a TABLE of inputs rather than one call: an update of
//! a lineage action resolves its `ActionName` against the Region the request signed for
//! and applies only to a record the store HOLDS, so a name nothing is stored under —
//! never created, or released by `DeleteAction` — is refused with the operation's modelled
//! `ResourceNotFound` (HTTP 400) naming the identifier resolved to its ARN, and the
//! refused call writes nothing: the released name stays free, and an action recreated
//! under it carries none of the deleted record's values.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// One emulator: the shared engine plus the SageMaker service mounted on the router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not SigV4; the request below carries a
        // credential scope only so the ARN region is the one the caller signed for.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `target` with a SigV4 credential scope naming `region`.
    async fn call_in(&self, region: &str, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/sagemaker/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `target` in `region` and require a 200, returning the parsed output.
    async fn ok_in(&self, region: &str, target: &str, body: &Value) -> Value {
        let (status, parsed) = self.call_in(region, target, body).await;
        assert_eq!(
            status, 200,
            "case {target} {body}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    async fn ok(&self, target: &str, body: &Value) -> Value {
        self.ok_in(HOME_REGION, target, body).await
    }
}

/// The region every case signs for unless it is exercising region scoping.
const HOME_REGION: &str = "us-east-1";

/// The `__type` an error body carries, with any `namespace#` prefix stripped.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The minimal `CreateAction` body: the three members the model marks required
/// (`ActionName`, `ActionType`, `Source` — smithy_query operation=CreateAction).
fn minimal_action(name: &str) -> Value {
    json!({
        "ActionName": name,
        "ActionType": "ModelDeployment",
        "Source": { "SourceUri": format!("https://example.test/actions/{name}") },
    })
}

/// A name of exactly `len` characters that satisfies `ExperimentEntityName`'s pattern
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$` (smithy_query shape=ExperimentEntityName).
fn name_of_length(len: usize) -> String {
    "a".repeat(len)
}

/// How a row's `ActionName` comes to name nothing in the Region the update signs for.
#[derive(Debug, Clone, Copy)]
enum Missing {
    /// No `CreateAction` ever ran for the name.
    NeverCreated,
    /// Created, then released by `DeleteAction`.
    Deleted,
    /// Created, updated, then released — so the record the update would have found had
    /// been modified before it went.
    UpdatedThenDeleted,
    /// Created, released, created again and released again: a name that cycled.
    Cycled,
    /// Created in another Region and still live there, so only the signing Region misses
    /// it.
    LiveInAnotherRegion,
}

/// The Region a `LiveInAnotherRegion` row creates its action in.
const OTHER_REGION: &str = "eu-west-2";

/// One row: a name, how it comes to be missing, and the update body sent for it.
struct Row {
    case: &'static str,
    name: String,
    missing: Missing,
    /// The update body without the `ActionName` the test fills in.
    update: Value,
}

/// RULE: `UpdateAction` applies only to an action the store HOLDS in the Region the
/// request signed for; an `ActionName` nothing is stored under is refused with the
/// modelled `ResourceNotFound` at HTTP 400, naming the identifier resolved to its ARN,
/// and the refused call writes nothing.
#[tokio::test]
async fn update_action_refuses_every_name_the_store_no_longer_holds() {
    // Rows differ along HOW the name misses (never created, deleted, deleted after an
    // update, cycled, live only in another Region), the members the refused update
    // carries (each editable member, every combination, none at all), and the name's
    // shape — including both `ExperimentEntityName` bounds, 1 and 120 characters
    // (smithy_query shape=ExperimentEntityName: len 1..120).
    let rows = vec![
        Row {
            case: "deleted-then-updated-with-a-description",
            name: "deleted-description".to_string(),
            missing: Missing::Deleted,
            update: json!({ "Description": "updated" }),
        },
        Row {
            case: "deleted-then-updated-with-a-status",
            name: "deleted-status".to_string(),
            missing: Missing::Deleted,
            update: json!({ "Status": "Completed" }),
        },
        Row {
            case: "deleted-then-updated-with-properties",
            name: "deleted-properties".to_string(),
            missing: Missing::Deleted,
            update: json!({ "Properties": { "stage": "prod" } }),
        },
        Row {
            case: "deleted-then-updated-with-properties-to-remove",
            name: "deleted-pruning".to_string(),
            missing: Missing::Deleted,
            update: json!({ "PropertiesToRemove": ["stage"] }),
        },
        Row {
            case: "deleted-then-updated-with-every-editable-member",
            name: "deleted-everything".to_string(),
            missing: Missing::Deleted,
            update: json!({
                "Description": "updated",
                "Status": "Stopped",
                "Properties": { "a": "1" },
                "PropertiesToRemove": ["b"],
            }),
        },
        Row {
            case: "deleted-then-updated-with-no-editable-member",
            name: "deleted-bare".to_string(),
            missing: Missing::Deleted,
            update: json!({}),
        },
        Row {
            case: "updated-then-deleted-then-updated-again",
            name: "modified-then-gone".to_string(),
            missing: Missing::UpdatedThenDeleted,
            update: json!({ "Description": "updated twice" }),
        },
        Row {
            case: "a-name-that-cycled-through-create-and-delete-twice",
            name: "cycled".to_string(),
            missing: Missing::Cycled,
            update: json!({ "Status": "Failed" }),
        },
        Row {
            case: "never-created",
            name: "absent".to_string(),
            missing: Missing::NeverCreated,
            update: json!({ "Description": "updated" }),
        },
        Row {
            case: "never-created-at-the-shortest-modelled-name-length",
            name: name_of_length(1),
            missing: Missing::NeverCreated,
            update: json!({ "Properties": { "a": "1" } }),
        },
        Row {
            case: "deleted-at-the-longest-modelled-name-length",
            name: name_of_length(120),
            missing: Missing::Deleted,
            update: json!({ "Description": "updated" }),
        },
        Row {
            case: "live-in-another-region-only",
            name: "elsewhere".to_string(),
            missing: Missing::LiveInAnotherRegion,
            update: json!({ "Description": "updated" }),
        },
    ];

    for row in &rows {
        let emulator = Emulator::new();
        let case = row.case;
        let name = &row.name;
        // A live action in the signing Region, so a refusal below is about the name that
        // misses and not about an empty store.
        emulator.ok("CreateAction", &minimal_action("bystander")).await;

        match row.missing {
            Missing::NeverCreated => {}
            Missing::Deleted => {
                emulator.ok("CreateAction", &minimal_action(name)).await;
                emulator
                    .ok("DeleteAction", &json!({ "ActionName": name }))
                    .await;
            }
            Missing::UpdatedThenDeleted => {
                emulator.ok("CreateAction", &minimal_action(name)).await;
                emulator
                    .ok(
                        "UpdateAction",
                        &json!({
                            "ActionName": name,
                            "Description": "updated once",
                            "Properties": { "stage": "test" },
                        }),
                    )
                    .await;
                emulator
                    .ok("DeleteAction", &json!({ "ActionName": name }))
                    .await;
            }
            Missing::Cycled => {
                for _ in 0..2 {
                    emulator.ok("CreateAction", &minimal_action(name)).await;
                    emulator
                        .ok("DeleteAction", &json!({ "ActionName": name }))
                        .await;
                }
            }
            Missing::LiveInAnotherRegion => {
                emulator
                    .ok_in(OTHER_REGION, "CreateAction", &minimal_action(name))
                    .await;
            }
        }

        let mut body = row
            .update
            .as_object()
            .expect("every row's update body is an object")
            .clone();
        body.insert("ActionName".to_string(), json!(name));
        let (status, parsed) = emulator
            .call_in(HOME_REGION, "UpdateAction", &Value::Object(body))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a name nothing is stored under must be refused, \
             got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {case:?}: expected the modelled ResourceNotFound, got {parsed:?}"
        );
        assert_eq!(
            message(&parsed),
            format!("Action 'arn:aws:sagemaker:{HOME_REGION}:{ACCOUNT}:action/{name}' does not exist."),
            "case {case:?}: the refusal sentence, got {parsed:?}"
        );

        // The refused update minted nothing: the reader still refuses the same name.
        let (read_status, read) = emulator
            .call_in(HOME_REGION, "DescribeAction", &json!({ "ActionName": name }))
            .await;
        assert_eq!(
            read_status, 400,
            "case {case:?}: a refused update must mint nothing, got {read_status} {read}"
        );

        // The live action beside it is untouched and still updatable.
        let bystander = emulator
            .ok("DescribeAction", &json!({ "ActionName": "bystander" }))
            .await;
        assert_eq!(
            bystander.get("Description"),
            None,
            "case {case:?}: the live action must be unchanged, got {bystander:?}"
        );
        emulator
            .ok(
                "UpdateAction",
                &json!({ "ActionName": "bystander", "Description": "updated" }),
            )
            .await;

        // A row whose action is live in another Region keeps it: the refusal in the
        // signing Region changed nothing there.
        if matches!(row.missing, Missing::LiveInAnotherRegion) {
            let elsewhere = emulator
                .ok_in(OTHER_REGION, "DescribeAction", &json!({ "ActionName": name }))
                .await;
            assert_eq!(
                elsewhere.get("Description"),
                None,
                "case {case:?}: the action in {OTHER_REGION} must be unchanged, \
                 got {elsewhere:?}"
            );
            assert_eq!(
                elsewhere.get("ActionArn").and_then(Value::as_str),
                Some(
                    format!("arn:aws:sagemaker:{OTHER_REGION}:{ACCOUNT}:action/{name}").as_str()
                ),
                "case {case:?}: the action in {OTHER_REGION} keeps its own ARN, \
                 got {elsewhere:?}"
            );
        }
    }
}

/// RULE: a released `ActionName` is free again, and the action created under it is a NEW
/// record — it carries only what its own create sent, so none of the deleted action's
/// description, status or properties survives, and it is updatable from that fresh state.
#[tokio::test]
async fn an_action_recreated_under_a_released_name_starts_from_its_own_create() {
    // Rows differ along what the FIRST action held when it was deleted (description,
    // status, properties, every member, none), so each one states a different set of
    // values that must not reappear.
    let rows: Vec<(&str, Value)> = vec![
        ("held-a-description", json!({ "Description": "the first action" })),
        ("held-a-status", json!({ "Status": "InProgress" })),
        ("held-properties", json!({ "Properties": { "stage": "test", "owner": "ml" } })),
        (
            "held-every-editable-member",
            json!({
                "Description": "the first action",
                "Status": "Stopping",
                "Properties": { "stage": "test" },
            }),
        ),
        ("held-no-optional-member", json!({})),
    ];

    for (case, held) in &rows {
        let emulator = Emulator::new();
        let name = "recreated";
        let mut body = minimal_action(name);
        for (member, value) in held.as_object().expect("an object of members") {
            body[member] = value.clone();
        }
        emulator.ok("CreateAction", &body).await;
        // The update that ran before the delete, so the deleted record held values from
        // an update as well as from its create.
        emulator
            .ok(
                "UpdateAction",
                &json!({
                    "ActionName": name,
                    "Description": "edited before the delete",
                    "Properties": { "edited": "yes" },
                }),
            )
            .await;
        emulator
            .ok("DeleteAction", &json!({ "ActionName": name }))
            .await;

        // The name is free: the create under it succeeds and reads back only its own
        // members.
        emulator.ok("CreateAction", &minimal_action(name)).await;
        let described = emulator
            .ok("DescribeAction", &json!({ "ActionName": name }))
            .await;
        assert_eq!(
            described.get("Description"),
            None,
            "case {case:?}: the recreated action carries no earlier description, \
             got {described:?}"
        );
        assert_eq!(
            described.get("Status"),
            None,
            "case {case:?}: the recreated action carries no earlier status, \
             got {described:?}"
        );
        assert_eq!(
            described
                .get("Properties")
                .and_then(Value::as_object)
                .map(Map::len)
                .unwrap_or(0),
            0,
            "case {case:?}: the recreated action carries no earlier properties, \
             got {described:?}"
        );

        // And it is updatable from that fresh state, which the reader confirms.
        emulator
            .ok(
                "UpdateAction",
                &json!({
                    "ActionName": name,
                    "Description": "the second action",
                    "Status": "Completed",
                    "Properties": { "stage": "prod" },
                }),
            )
            .await;
        let described = emulator
            .ok("DescribeAction", &json!({ "ActionName": name }))
            .await;
        assert_eq!(
            described.get("Description").and_then(Value::as_str),
            Some("the second action"),
            "case {case:?}: the recreated action takes the new description, \
             got {described:?}"
        );
        assert_eq!(
            described.get("Status").and_then(Value::as_str),
            Some("Completed"),
            "case {case:?}: the recreated action takes the new status, got {described:?}"
        );
        assert_eq!(
            described.get("Properties"),
            Some(&json!({ "stage": "prod" })),
            "case {case:?}: the recreated action takes the new properties, \
             got {described:?}"
        );
    }
}

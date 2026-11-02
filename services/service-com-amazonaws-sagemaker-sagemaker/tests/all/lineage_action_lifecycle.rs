// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's ML Lineage Tracking actions:
//! `CreateAction` -> `DescribeAction` -> `DeleteAction`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the
//! `action/<name>` ARN form for every name shape and region; the create/describe round
//! trip for every optional member and every modelled `ActionStatus`; the name-or-ARN
//! identifier the reader accepts; the not-found refusal of the reader and the delete;
//! and the uniqueness of the name within the store.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The lineage group every lineage entity belongs to (`state::DEFAULT_LINEAGE_GROUP`).
const DEFAULT_LINEAGE_GROUP: &str = "sagemaker-default-lineage-group";

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
        self.dispatch(request).await
    }

    /// Call `target` unsigned: no credential scope, so the ARN region falls back to the
    /// emulator default.
    async fn call_unsigned(&self, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.dispatch(request).await
    }

    async fn dispatch(&self, request: http::Request<bytes::Bytes>) -> (u16, Value) {
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

    async fn call(&self, target: &str, body: &Value) -> (u16, Value) {
        self.call_in("us-east-1", target, body).await
    }

    /// Call `target` and require a 200, returning the parsed output.
    async fn ok(&self, target: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(target, body).await;
        assert_eq!(
            status, 200,
            "case {target} {body}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }
}

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

fn str_field(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string member {member} in {body}"))
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
        "Source": { "SourceUri": "https://example.test/deployments/1" },
    })
}

/// A name of exactly `len` characters that satisfies `ExperimentEntityName`'s pattern
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$` (smithy_query shape=ExperimentEntityName).
fn name_of_length(len: usize) -> String {
    "a".repeat(len)
}

/// RULE: every action ARN this service mints is
/// `arn:aws:sagemaker:<signed region>:<account>:action/<ActionName>`, whatever the
/// name's shape and whatever region the caller signed for — and the emulator's default
/// region when the request carries no usable credential scope.
#[tokio::test]
async fn created_action_arns_name_the_action_kind_the_signed_region_and_the_stored_name() {
    // Rows differ along the name's shape (one character, digits, hyphens, mixed case)
    // and the `ExperimentEntityName` upper bound of 120 characters
    // (smithy_query shape=ExperimentEntityName: len 1..120).
    let names = vec![
        "a".to_string(),
        "Action1".to_string(),
        "deployment-action-with-hyphens".to_string(),
        "MixedCase123".to_string(),
        name_of_length(120),
    ];
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    for region in regions {
        for name in &names {
            let emulator = Emulator::new();
            let case = format!("region={region} name-len={}", name.len());

            let (status, body) = emulator
                .call_in(region, "CreateAction", &minimal_action(name))
                .await;
            assert_eq!(
                status, 200,
                "case {case}: CreateAction, got {status} with {body}"
            );
            assert_eq!(
                str_field(&body, "ActionArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:action/{name}"),
                "case {case}: ActionArn, got {body:?}"
            );

            // The ARN the create minted is the ARN every later call reports.
            let described = emulator
                .call_in(region, "DescribeAction", &json!({ "ActionName": name }))
                .await;
            assert_eq!(
                str_field(&described.1, "ActionArn"),
                str_field(&body, "ActionArn"),
                "case {case}: DescribeAction must replay the minted ARN, got {:?}",
                described.1
            );
        }
    }

    // A credential scope whose region is not a region token cannot enter an ARN (the
    // region component is positional), so the emulator default stands in; so does an
    // unsigned request, which names no region at all.
    for malformed in ["us east 1", "US-EAST-1", "us:east/1"] {
        let emulator = Emulator::new();
        let (status, body) = emulator
            .call_in(malformed, "CreateAction", &minimal_action("odd-region"))
            .await;
        assert_eq!(
            status, 200,
            "case malformed-region={malformed}: CreateAction, got {status} {body}"
        );
        assert_eq!(
            str_field(&body, "ActionArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:action/odd-region"),
            "case malformed-region={malformed}: ActionArn must fall back to the default \
             region, got {body:?}"
        );
    }

    let emulator = Emulator::new();
    let (status, body) = emulator
        .call_unsigned("CreateAction", &minimal_action("unsigned-action"))
        .await;
    assert_eq!(status, 200, "case unsigned: CreateAction, got {status} {body}");
    assert_eq!(
        str_field(&body, "ActionArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:action/unsigned-action"),
        "case unsigned: ActionArn, got {body:?}"
    );
}

/// RULE: `DescribeAction` replays every member `CreateAction` stored — present members
/// and absent ones alike — and reports the create instant as both `CreationTime` and,
/// until something modifies the action, `LastModifiedTime`.
#[tokio::test]
async fn describe_action_replays_exactly_what_create_action_stored() {
    let max_description = "d".repeat(3072);
    let max_action_type = "t".repeat(256);
    // 30 properties: `LineageEntityParameters` is `len 0..30`
    // (smithy_query shape=LineageEntityParameters), so this row sits on the upper bound.
    let many_properties: Value = Value::Object(
        (0..30)
            .map(|index| (format!("key{index}"), json!(format!("value{index}"))))
            .collect(),
    );

    // Rows differ along which optional members are present, the size of the map and
    // list members, and the bounds of the string members.
    let cases: Vec<(&str, Value)> = vec![
        ("required-members-only", json!({})),
        (
            "description-and-status",
            json!({ "Description": "a deployment of the fraud model", "Status": "InProgress" }),
        ),
        (
            "empty-strings-on-the-lower-bounds",
            json!({ "ActionType": "", "Description": "" }),
        ),
        (
            "string-members-on-their-upper-bounds",
            json!({ "ActionType": max_action_type, "Description": max_description }),
        ),
        (
            "one-property",
            json!({ "Properties": { "sole": "value" } }),
        ),
        ("thirty-properties", json!({ "Properties": many_properties })),
        (
            "empty-property-map",
            json!({ "Properties": {} }),
        ),
        (
            "metadata-properties",
            json!({
                "MetadataProperties": {
                    "CommitId": "0123456789abcdef",
                    "Repository": "https://example.test/repo",
                    "GeneratedBy": "pipeline",
                    "ProjectId": "p-123",
                },
            }),
        ),
        (
            "full-source-and-tags",
            json!({
                "Source": {
                    "SourceUri": "arn:aws:sagemaker:us-east-1:123456789012:endpoint/demo",
                    "SourceType": "Endpoint",
                    "SourceId": "demo",
                },
                "Tags": [{ "Key": "team", "Value": "ml" }, { "Key": "env", "Value": "test" }],
            }),
        ),
    ];

    for (case, extra) in cases {
        let emulator = Emulator::new();
        let mut body = minimal_action(case);
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            body[key] = value.clone();
        }

        let created = emulator.ok("CreateAction", &body).await;
        let described = emulator
            .ok("DescribeAction", &json!({ "ActionName": case }))
            .await;

        assert_eq!(
            str_field(&described, "ActionName"),
            case,
            "case {case}: ActionName, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "ActionArn"),
            str_field(&created, "ActionArn"),
            "case {case}: ActionArn, got {described:?}"
        );

        // Every member the request carried comes back byte-for-byte; every member it
        // did not carry is ABSENT, never defaulted — except `Properties`, the map the
        // action HOLDS, which an action created with none reports as the empty map.
        for member in [
            "ActionType",
            "Description",
            "Status",
            "MetadataProperties",
            "Source",
        ] {
            let sent = body.get(member);
            let read = described.get(member);
            assert_eq!(
                read, sent,
                "case {case}: {member} must replay what CreateAction stored, \
                 sent {sent:?}, got {read:?}"
            );
        }
        assert_eq!(
            described.get("Properties"),
            Some(&body.get("Properties").cloned().unwrap_or_else(|| json!({}))),
            "case {case}: Properties must be the map the action holds, got {described:?}"
        );

        // `Tags` is not a member of `DescribeActionResponse` (smithy_query
        // operation=DescribeAction), so the reader never reports it.
        assert!(
            described.get("Tags").is_none(),
            "case {case}: DescribeAction models no Tags member, got {described:?}"
        );

        let creation_time = described
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: expected a CreationTime in {described:?}"));
        let last_modified = described
            .get("LastModifiedTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: expected a LastModifiedTime in {described:?}"));
        assert!(
            (creation_time - last_modified).abs() < f64::EPSILON,
            "case {case}: an unmodified action reports LastModifiedTime == CreationTime, \
             got {last_modified:?} and {creation_time:?}"
        );

        // A read changes nothing: a second describe reports exactly the first answer.
        let again = emulator
            .ok("DescribeAction", &json!({ "ActionName": case }))
            .await;
        assert_eq!(
            again, described,
            "case {case}: DescribeAction must be stable across reads, got {again:?}"
        );

        // The emulator models no Studio user profile and no IAM identity details, so the
        // user context is reported with no members rather than invented ones.
        for member in ["CreatedBy", "LastModifiedBy"] {
            assert_eq!(
                described.get(member),
                Some(&json!({})),
                "case {case}: {member} must be the empty user context, got {described:?}"
            );
        }
        // Every lineage entity belongs to the Region's one default lineage group.
        assert_eq!(
            described.get("LineageGroupArn").and_then(Value::as_str),
            Some(
                format!(
                    "arn:aws:sagemaker:us-east-1:{ACCOUNT}:lineage-group/{DEFAULT_LINEAGE_GROUP}"
                )
                .as_str()
            ),
            "case {case}: LineageGroupArn, got {described:?}"
        );
    }
}

/// RULE: every modelled `ActionStatus` value round-trips unchanged — the status is the
/// caller's, and nothing in the lifecycle of a lineage action moves it.
#[tokio::test]
async fn every_modelled_action_status_round_trips() {
    // The full enum, from smithy_query shape=ActionStatus:
    // Unknown | InProgress | Completed | Failed | Stopping | Stopped.
    let statuses = [
        "Unknown",
        "InProgress",
        "Completed",
        "Failed",
        "Stopping",
        "Stopped",
    ];

    for status in statuses {
        let emulator = Emulator::new();
        let mut body = minimal_action("status-action");
        body["Status"] = json!(status);
        emulator.ok("CreateAction", &body).await;

        let described = emulator
            .ok("DescribeAction", &json!({ "ActionName": "status-action" }))
            .await;
        assert_eq!(
            str_field(&described, "Status"),
            status,
            "case status={status}: Status, got {described:?}"
        );
    }

    // An action created without a `Status` has none to report: absent, not invented.
    let emulator = Emulator::new();
    emulator
        .ok("CreateAction", &minimal_action("no-status-action"))
        .await;
    let described = emulator
        .ok("DescribeAction", &json!({ "ActionName": "no-status-action" }))
        .await;
    assert!(
        described.get("Status").is_none(),
        "case status=absent: an action created without a Status reports none, \
         got {described:?}"
    );
}

/// RULE: `DescribeAction`'s `ActionName` is an `ExperimentEntityNameOrArn`, so the
/// caller may identify the action by its name OR by its ARN, and both reach the same
/// stored record.
#[tokio::test]
async fn describe_action_accepts_the_name_or_the_arn_as_the_identifier() {
    // Rows differ along region (so the ARN's region component varies) and name shape.
    for (region, name) in [
        ("us-east-1", "arn-lookup"),
        ("eu-west-1", "a"),
        ("ap-southeast-2", "Action-9"),
    ] {
        let emulator = Emulator::new();
        let case = format!("region={region} name={name}");
        let created = emulator
            .call_in(region, "CreateAction", &minimal_action(name))
            .await;
        assert_eq!(created.0, 200, "case {case}: CreateAction {:?}", created.1);
        let arn = str_field(&created.1, "ActionArn");

        let by_name = emulator
            .call_in(region, "DescribeAction", &json!({ "ActionName": name }))
            .await;
        let by_arn = emulator
            .call_in(region, "DescribeAction", &json!({ "ActionName": arn }))
            .await;
        assert_eq!(
            by_arn.0, 200,
            "case {case}: DescribeAction by ARN, got {} with {:?}",
            by_arn.0, by_arn.1
        );
        assert_eq!(
            by_arn.1, by_name.1,
            "case {case}: an ARN identifies the same action as its name, got {:?} vs {:?}",
            by_arn.1, by_name.1
        );

        // An ARN is resolved as a WHOLE: one that differs in the entity kind, the
        // account, the partition or the Region addresses no action here, exactly as it
        // addresses none at the real endpoint.
        let foreign_arns = [
            format!("arn:aws:sagemaker:{region}:{ACCOUNT}:artifact/{name}"),
            format!("arn:aws:sagemaker:{region}:{ACCOUNT}:experiment/{name}"),
            format!("arn:aws:sagemaker:{region}:123456789012:action/{name}"),
            format!("arn:aws-us-gov:sagemaker:{region}:{ACCOUNT}:action/{name}"),
            format!("arn:aws:sagemaker:us-west-1:{ACCOUNT}:action/{name}"),
        ];
        for foreign in foreign_arns {
            let (status, body) = emulator
                .call_in(region, "DescribeAction", &json!({ "ActionName": foreign }))
                .await;
            assert_eq!(
                status, 400,
                "case {case} foreign-arn={foreign}: expected HTTP 400, got {status} \
                 with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFound",
                "case {case} foreign-arn={foreign}: error code, got {body:?}"
            );
        }
    }
}

/// RULE: an `ActionName` is unique to the account in a REGION, so the same name names a
/// different action in each Region: a create of it succeeds in both, each reader answers
/// from its own Region's record, and a delete in one Region leaves the other standing.
#[tokio::test]
async fn action_names_are_scoped_to_the_region_the_request_signed_for() {
    // Rows differ along the Region pair and the name's shape.
    for (here, there, name) in [
        ("us-east-1", "eu-west-1", "shared-name"),
        ("eu-central-1", "ap-southeast-2", "a"),
    ] {
        let emulator = Emulator::new();
        let case = format!("here={here} there={there} name={name}");

        let mut here_body = minimal_action(name);
        here_body["ActionType"] = json!("HereType");
        let mut there_body = minimal_action(name);
        there_body["ActionType"] = json!("ThereType");

        let created_here = emulator.call_in(here, "CreateAction", &here_body).await;
        let created_there = emulator.call_in(there, "CreateAction", &there_body).await;
        assert_eq!(
            created_here.0, 200,
            "case {case}: CreateAction in {here}, got {:?}",
            created_here.1
        );
        assert_eq!(
            created_there.0, 200,
            "case {case}: the same name in {there} is a different action and must be \
             accepted, got {} with {:?}",
            created_there.0, created_there.1
        );
        assert_ne!(
            str_field(&created_here.1, "ActionArn"),
            str_field(&created_there.1, "ActionArn"),
            "case {case}: the two Regions mint different ARNs"
        );

        for (region, expected_type) in [(here, "HereType"), (there, "ThereType")] {
            let described = emulator
                .call_in(region, "DescribeAction", &json!({ "ActionName": name }))
                .await;
            assert_eq!(
                described.0, 200,
                "case {case}: DescribeAction in {region}, got {:?}",
                described.1
            );
            assert_eq!(
                str_field(&described.1, "ActionType"),
                expected_type,
                "case {case}: the reader answers from {region}'s own record, got {:?}",
                described.1
            );
        }

        // A delete is scoped the same way: it releases only this Region's record.
        let deleted = emulator
            .call_in(here, "DeleteAction", &json!({ "ActionName": name }))
            .await;
        assert_eq!(deleted.0, 200, "case {case}: DeleteAction {:?}", deleted.1);
        let gone = emulator
            .call_in(here, "DescribeAction", &json!({ "ActionName": name }))
            .await;
        assert_eq!(
            gone.0, 400,
            "case {case}: the deleted record is gone in {here}, got {:?}",
            gone.1
        );
        let survivor = emulator
            .call_in(there, "DescribeAction", &json!({ "ActionName": name }))
            .await;
        assert_eq!(
            survivor.0, 200,
            "case {case}: {there}'s record survives a delete in {here}, got {:?}",
            survivor.1
        );
    }
}

/// RULE: a reader or a delete that names an action the store does not hold is refused
/// with the modelled `ResourceNotFound` (HTTP 400), never a 200 with empty members —
/// and that is exactly what a deleted action becomes.
#[tokio::test]
async fn describe_and_delete_refuse_an_unknown_action_with_resource_not_found() {
    // Rows differ along why the name is unknown: never created, a near-miss of a name
    // that does exist, a different case, and an ARN form for the reader.
    let emulator = Emulator::new();
    emulator.ok("CreateAction", &minimal_action("existing")).await;

    let unknown_for_reader = [
        "missing",
        "existing1",
        "Existing",
        &format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:action/missing"),
    ];
    for name in unknown_for_reader {
        let (status, body) = emulator
            .call("DescribeAction", &json!({ "ActionName": name }))
            .await;
        assert_eq!(
            status, 400,
            "case describe-unknown={name}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFound",
            "case describe-unknown={name}: error code, got {body:?}"
        );
        // The lineage readers report the identifier RESOLVED to its ARN, single-quoted:
        // a name is resolved against the Region the request signed for, an ARN stands as
        // it arrived.
        let resolved = if name.starts_with("arn:") {
            name.to_string()
        } else {
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:action/{name}")
        };
        assert_eq!(
            message(&body),
            format!("Action '{resolved}' does not exist."),
            "case describe-unknown={name}: the message must name the resolved ARN, \
             got {:?}",
            message(&body)
        );
    }

    for name in ["missing", "existing1", "Existing"] {
        let (status, body) = emulator
            .call("DeleteAction", &json!({ "ActionName": name }))
            .await;
        assert_eq!(
            status, 400,
            "case delete-unknown={name}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFound",
            "case delete-unknown={name}: error code, got {body:?}"
        );
        assert_eq!(
            message(&body),
            format!("Action 'arn:aws:sagemaker:us-east-1:{ACCOUNT}:action/{name}' does not exist."),
            "case delete-unknown={name}: the message must name the resolved ARN, got {:?}",
            message(&body)
        );
    }

    // The action that DOES exist is untouched by every refusal above.
    let described = emulator
        .ok("DescribeAction", &json!({ "ActionName": "existing" }))
        .await;
    assert_eq!(
        str_field(&described, "ActionName"),
        "existing",
        "case survivor: the stored action is untouched, got {described:?}"
    );

    // Deleting it answers the ARN the create minted, and the action is then as unknown
    // as any name that was never used.
    let deleted = emulator
        .ok("DeleteAction", &json!({ "ActionName": "existing" }))
        .await;
    assert_eq!(
        str_field(&deleted, "ActionArn"),
        str_field(&described, "ActionArn"),
        "case delete: DeleteAction answers the stored ARN, got {deleted:?}"
    );
    let (status, body) = emulator
        .call("DescribeAction", &json!({ "ActionName": "existing" }))
        .await;
    assert_eq!(
        status, 400,
        "case describe-after-delete: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFound",
        "case describe-after-delete: error code, got {body:?}"
    );
}

/// RULE: an `ActionName` is unique within the account and Region, so a create naming one
/// already in use is refused and leaves the stored action as it was; deleting the action
/// releases the name for a later create.
#[tokio::test]
async fn an_action_name_is_unique_until_the_action_is_deleted() {
    // Rows differ along the name's shape, including the 120-character upper bound.
    let names = vec![
        "a".to_string(),
        "duplicate-action".to_string(),
        name_of_length(120),
    ];

    for name in names {
        let emulator = Emulator::new();
        let case = format!("name-len={}", name.len());

        let first = emulator.ok("CreateAction", &minimal_action(&name)).await;

        let mut second = minimal_action(&name);
        second["ActionType"] = json!("SecondType");
        let (status, body) = emulator.call("CreateAction", &second).await;
        assert_eq!(
            status, 400,
            "case {case}: a duplicate name is refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: duplicate-name error code, got {body:?}"
        );
        assert!(
            message(&body).contains(&format!("\"{name}\"")),
            "case {case}: the message must quote the name passed, got {:?}",
            message(&body)
        );

        // The refused create wrote nothing: the first action still stands.
        let described = emulator
            .ok("DescribeAction", &json!({ "ActionName": &name }))
            .await;
        assert_eq!(
            str_field(&described, "ActionType"),
            "ModelDeployment",
            "case {case}: the refused create must not overwrite the stored action, \
             got {described:?}"
        );
        assert_eq!(
            str_field(&described, "ActionArn"),
            str_field(&first, "ActionArn"),
            "case {case}: ActionArn, got {described:?}"
        );

        // Deleting releases the name, and the next create of it stores the new values.
        emulator
            .ok("DeleteAction", &json!({ "ActionName": &name }))
            .await;
        let recreated = emulator.ok("CreateAction", &second).await;
        assert_eq!(
            str_field(&recreated, "ActionArn"),
            str_field(&first, "ActionArn"),
            "case {case}: the re-created action carries the same ARN, got {recreated:?}"
        );
        let described = emulator
            .ok("DescribeAction", &json!({ "ActionName": &name }))
            .await;
        assert_eq!(
            str_field(&described, "ActionType"),
            "SecondType",
            "case {case}: the re-created action is the new one, got {described:?}"
        );
    }
}

/// RULE: within one Region, distinct action names are independent records: each
/// `DescribeAction` answers from its own, and deleting one leaves the others readable.
#[tokio::test]
async fn distinct_action_names_are_independent_records() {
    let emulator = Emulator::new();
    // Rows differ along count and along which optional members each action carries.
    let actions = [
        ("first", "TrainingJob", Some("InProgress")),
        ("second", "ModelDeployment", Some("Completed")),
        ("third", "HPOJob", None),
        ("fourth", "", Some("Failed")),
    ];

    for (name, action_type, status) in actions {
        let mut body = minimal_action(name);
        body["ActionType"] = json!(action_type);
        match status {
            Some(value) => body["Status"] = json!(value),
            None => {
                body.as_object_mut().expect("an object body").remove("Status");
            }
        }
        emulator.ok("CreateAction", &body).await;
    }

    for (name, action_type, status) in actions {
        let described = emulator
            .ok("DescribeAction", &json!({ "ActionName": name }))
            .await;
        assert_eq!(
            str_field(&described, "ActionType"),
            action_type,
            "case {name}: ActionType, got {described:?}"
        );
        assert_eq!(
            described.get("Status").and_then(Value::as_str),
            status,
            "case {name}: Status, got {described:?}"
        );
    }

    // Deleting one leaves the others readable.
    emulator
        .ok("DeleteAction", &json!({ "ActionName": "second" }))
        .await;
    for name in ["first", "third", "fourth"] {
        let described = emulator
            .ok("DescribeAction", &json!({ "ActionName": name }))
            .await;
        assert_eq!(
            str_field(&described, "ActionName"),
            name,
            "case survivor={name}: ActionName, got {described:?}"
        );
    }
}

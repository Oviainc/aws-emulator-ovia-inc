// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's model-package-group registry:
//! `CreateModelPackageGroup` and the `DescribeModelPackageGroup` that reads it back.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the ARN
//! form for every name shape and region, the create/describe round trip for every
//! optional member, `ArnOrName` addressing for the reader, the refusal of a name already
//! in use, the refusal of a name no group is stored under, and the stability of a read.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
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

/// `ModelPackageGroupName` is `len 1..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`
/// (smithy_query operation=CreateModelPackageGroup), so these rows span the name shapes
/// the pattern admits, including both length bounds.
const NAMES: [&str; 5] = [
    "g",
    "group1",
    "demo-model-package-group",
    "Mixed-Case-0123",
    "a234567890123456789012345678901234567890123456789012345678901b",
];

/// RULE: `CreateModelPackageGroup` mints
/// `arn:aws:sagemaker:<signed region>:<account>:model-package-group/<name>` and returns it,
/// whatever the name's shape and whatever region the caller signed for (the emulator
/// default when the request is unsigned).
#[tokio::test]
async fn created_group_arn_names_the_signed_region_and_the_requested_name() {
    // Rows differ along the axes the rule generalises over: name shape and region.
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    for region in regions {
        for name in NAMES {
            let emulator = Emulator::new();
            let case = format!("region={region} name={name}");
            let (status, body) = emulator
                .call_in(region, "CreateModelPackageGroup", &json!({
                    "ModelPackageGroupName": name,
                }))
                .await;
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200 from CreateModelPackageGroup, got {status} \
                 with {body}"
            );
            assert_eq!(
                str_field(&body, "ModelPackageGroupArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:model-package-group/{name}"),
                "case {case}: ModelPackageGroupArn, got {body:?}"
            );
        }
    }

    // An unsigned request names no region, so the ARN carries the emulator's default.
    let emulator = Emulator::new();
    let (status, body) = emulator
        .call_unsigned(
            "CreateModelPackageGroup",
            &json!({ "ModelPackageGroupName": "unsigned-group" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case unsigned: expected HTTP 200, got {status} with {body}"
    );
    assert_eq!(
        str_field(&body, "ModelPackageGroupArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model-package-group/unsigned-group"),
        "case unsigned: ModelPackageGroupArn must carry the default region, got {body:?}"
    );
}

/// RULE: `DescribeModelPackageGroup` replays exactly what the matching
/// `CreateModelPackageGroup` stored — every member it was given, absent for every member it
/// was not — plus the ARN, a creation time and the `Completed` status a synchronously
/// created group carries.
#[tokio::test]
async fn describe_replays_every_member_create_stored() {
    // `ModelPackageGroupDescription` is `EntityDescription, len 0..1024, pattern
    // ^[\p{L}\p{M}\p{Z}\p{S}\p{N}\p{P}]*$`; `ManagedConfiguration.ManagedStorageType` is an
    // enum whose only value is `Restricted` (smithy_query
    // shape=ManagedStorageType). The rows therefore span: no optional member at all, an
    // EMPTY description (distinguishable from an absent one), a unicode description
    // exercising the letter/mark/separator/symbol/punctuation classes the pattern admits,
    // the 1024-character upper bound, tags, and the managed configuration.
    let unicode_description = "Grupo de modelos — ñ 日本語 ✨ 42";
    let longest_description = "a".repeat(1024);
    let cases: [(&str, Value, Option<&str>, Option<&str>); 6] = [
        ("bare", json!({}), None, None),
        (
            "empty-description",
            json!({ "ModelPackageGroupDescription": "" }),
            Some(""),
            None,
        ),
        (
            "unicode-description",
            json!({ "ModelPackageGroupDescription": unicode_description }),
            Some(unicode_description),
            None,
        ),
        (
            "max-length-description",
            json!({ "ModelPackageGroupDescription": longest_description.clone() }),
            Some(&longest_description),
            None,
        ),
        (
            "tags",
            json!({ "Tags": [{ "Key": "team", "Value": "ml" }] }),
            None,
            None,
        ),
        (
            "managed-configuration",
            json!({
                "ModelPackageGroupDescription": "managed",
                "ManagedConfiguration": { "ManagedStorageType": "Restricted" },
            }),
            Some("managed"),
            Some("Restricted"),
        ),
    ];

    for (case, extra, expected_description, expected_storage_type) in cases {
        for name in NAMES {
            let emulator = Emulator::new();
            let case = format!("{case} name={name}");
            let mut request = json!({ "ModelPackageGroupName": name });
            for (member, value) in extra.as_object().expect("the extra members are an object") {
                request
                    .as_object_mut()
                    .expect("the request is an object")
                    .insert(member.clone(), value.clone());
            }

            let created = emulator.ok("CreateModelPackageGroup", &request).await;
            let arn = str_field(&created, "ModelPackageGroupArn");

            let described = emulator
                .ok(
                    "DescribeModelPackageGroup",
                    &json!({ "ModelPackageGroupName": name }),
                )
                .await;

            assert_eq!(
                str_field(&described, "ModelPackageGroupName"),
                name,
                "case {case}: ModelPackageGroupName, got {described:?}"
            );
            assert_eq!(
                str_field(&described, "ModelPackageGroupArn"),
                arn,
                "case {case}: DescribeModelPackageGroup must replay the ARN create minted, \
                 got {described:?}"
            );
            assert_eq!(
                described
                    .get("ModelPackageGroupDescription")
                    .and_then(Value::as_str),
                expected_description,
                "case {case}: ModelPackageGroupDescription, got {described:?}"
            );
            assert_eq!(
                described
                    .get("ManagedConfiguration")
                    .and_then(|managed| managed.get("ManagedStorageType"))
                    .and_then(Value::as_str),
                expected_storage_type,
                "case {case}: ManagedConfiguration.ManagedStorageType, got {described:?}"
            );
            assert_eq!(
                str_field(&described, "ModelPackageGroupStatus"),
                "Completed",
                "case {case}: a synchronously created model group is already in the terminal \
                 Completed status, got {described:?}"
            );
            let creation_time = described
                .get("CreationTime")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("case {case}: CreationTime, got {described:?}"));
            assert!(
                creation_time > 0.0,
                "case {case}: expected an epoch CreationTime after 1970, got {creation_time}"
            );
            // `CreatedBy` is modelled **required**, so it is always present, and it names the
            // IAM identity the request signed with.
            let created_by = described
                .get("CreatedBy")
                .unwrap_or_else(|| panic!("case {case}: CreatedBy, got {described:?}"));
            assert_eq!(
                created_by
                    .get("IamIdentity")
                    .and_then(|identity| identity.get("Arn"))
                    .and_then(Value::as_str),
                Some(format!("arn:aws:iam::{ACCOUNT}:user/AKIDEXAMPLE").as_str()),
                "case {case}: CreatedBy.IamIdentity.Arn, got {created_by:?}"
            );
        }
    }
}

/// RULE: a reader whose input is modelled `ArnOrName` addresses the same resource whether it
/// is given the resource's name or its full ARN.
#[tokio::test]
async fn describe_accepts_the_group_name_or_its_arn() {
    for name in NAMES {
        for region in ["us-east-1", "eu-central-1"] {
            let emulator = Emulator::new();
            let case = format!("region={region} name={name}");
            let created = emulator
                .call_in(
                    region,
                    "CreateModelPackageGroup",
                    &json!({ "ModelPackageGroupName": name, "ModelPackageGroupDescription": "d" }),
                )
                .await;
            assert_eq!(
                created.0, 200,
                "case {case}: CreateModelPackageGroup, got {:?}",
                created.1
            );
            let arn = str_field(&created.1, "ModelPackageGroupArn");

            let by_name = emulator
                .call_in(
                    region,
                    "DescribeModelPackageGroup",
                    &json!({ "ModelPackageGroupName": name }),
                )
                .await;
            let by_arn = emulator
                .call_in(
                    region,
                    "DescribeModelPackageGroup",
                    &json!({ "ModelPackageGroupName": arn }),
                )
                .await;
            assert_eq!(
                by_name.0, 200,
                "case {case}: describe by name, got {:?}",
                by_name.1
            );
            assert_eq!(
                by_arn.0, 200,
                "case {case}: describe by ARN {arn}, got {:?}",
                by_arn.1
            );
            assert_eq!(
                by_arn.1, by_name.1,
                "case {case}: describing by ARN must answer exactly what describing by name \
                 answers, got {:?}",
                by_arn.1
            );
        }
    }
}

/// RULE: a create whose name is already in use is refused (HTTP 400 `ValidationException`)
/// and leaves the stored group untouched, rather than overwriting it with a 200.
#[tokio::test]
async fn creating_an_existing_group_name_is_refused_and_changes_nothing() {
    for name in NAMES {
        let emulator = Emulator::new();
        let first = emulator
            .ok(
                "CreateModelPackageGroup",
                &json!({ "ModelPackageGroupName": name, "ModelPackageGroupDescription": "first" }),
            )
            .await;
        let before = emulator
            .ok(
                "DescribeModelPackageGroup",
                &json!({ "ModelPackageGroupName": name }),
            )
            .await;

        let (status, body) = emulator
            .call(
                "CreateModelPackageGroup",
                &json!({ "ModelPackageGroupName": name, "ModelPackageGroupDescription": "second" }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case name={name}: expected HTTP 400 for a duplicate name, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case name={name}: error code, got {body:?}"
        );
        assert!(
            body.get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .contains(&str_field(&first, "ModelPackageGroupArn")),
            "case name={name}: the refusal must name the group's ARN, got {body:?}"
        );

        let after = emulator
            .ok(
                "DescribeModelPackageGroup",
                &json!({ "ModelPackageGroupName": name }),
            )
            .await;
        assert_eq!(
            after, before,
            "case name={name}: the refused create must leave the stored group untouched, \
             got {after:?}"
        );
    }
}

/// RULE: a reader given a name no resource is stored under refuses the call (HTTP 400
/// `ValidationException`) rather than answering a 200 with empty members.
#[tokio::test]
async fn describing_an_absent_group_is_refused() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateModelPackageGroup",
            &json!({ "ModelPackageGroupName": "present-group" }),
        )
        .await;

    // Rows differ along what "absent" can look like: never created, a near-miss of a name
    // that does exist, a differently cased name, and an ARN for a group that was never
    // created.
    let absent = [
        "missing-group".to_string(),
        "present-group-2".to_string(),
        "Present-Group".to_string(),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model-package-group/missing-group"),
    ];
    for name in absent {
        let (status, body) = emulator
            .call(
                "DescribeModelPackageGroup",
                &json!({ "ModelPackageGroupName": name }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case name={name}: expected HTTP 400 for an absent group, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case name={name}: error code, got {body:?}"
        );
    }
}

/// RULE: a read never changes what a later caller sees — repeated describes of one group
/// answer byte-identical bodies, including its status, creation time and creator.
#[tokio::test]
async fn repeated_describes_answer_the_same_group() {
    let emulator = Emulator::new();
    for name in NAMES {
        emulator
            .ok(
                "CreateModelPackageGroup",
                &json!({ "ModelPackageGroupName": name, "ModelPackageGroupDescription": "stable" }),
            )
            .await;
    }

    for name in NAMES {
        let first = emulator
            .ok(
                "DescribeModelPackageGroup",
                &json!({ "ModelPackageGroupName": name }),
            )
            .await;
        // Long enough for any clock-owed transition the emulator schedules (the longest is
        // `state::ENDPOINT_IN_SERVICE_AFTER`, 300 ms) to have come due: a model group is
        // created ready, so nothing about it may move.
        tokio::time::sleep(Duration::from_millis(600)).await;
        let second = emulator
            .ok(
                "DescribeModelPackageGroup",
                &json!({ "ModelPackageGroupName": name }),
            )
            .await;
        assert_eq!(
            second, first,
            "case name={name}: a second DescribeModelPackageGroup must answer what the first \
             did, got {second:?}"
        );
    }
}

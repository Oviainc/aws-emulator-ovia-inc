// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's model-package-group registry:
//! `CreateModelPackageGroup`, the `DescribeModelPackageGroup` that reads it back and the
//! `DeleteModelPackageGroup` that removes it.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the ARN
//! form for every name shape and region, the create/describe round trip for every
//! optional member, `ArnOrName` addressing for the reader, the refusal of a name already
//! in use, the refusal of a name no group is stored under, the stability of a read, and
//! — for the delete — that it removes exactly the group it names, refuses a group that
//! still holds registered model package versions, succeeds once `DeleteModelPackage` has
//! removed them, and refuses a name no group is stored under.

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

/// The `message` an error body carries.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The two forms a caller may address a resource by when the input member is modelled
/// `ArnOrName`.
#[derive(Debug, Clone, Copy)]
enum Address {
    Name,
    Arn,
}

impl Address {
    /// The identifier to send for the group called `name` whose ARN is `arn`.
    fn identifier(self, name: &str, arn: &str) -> String {
        match self {
            Self::Name => name.to_string(),
            Self::Arn => arn.to_string(),
        }
    }
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
        // Long enough that a clock-owed transition, had this resource been given one,
        // would have come due: a model package group is created ready, so nothing about it
        // may move however long the caller waits.
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

/// RULE: a delete removes exactly the resource its identifier resolves to — afterwards the
/// reader refuses that resource, every other stored resource of the kind still reads back
/// unchanged, and the freed name can be taken again.
#[tokio::test]
async fn deleting_a_group_removes_only_that_group_and_frees_its_name() {
    // Rows differ along the name's shape and along the two `ArnOrName` addressing forms
    // the delete's `ModelPackageGroupName` admits.
    for address in [Address::Name, Address::Arn] {
        for name in NAMES {
            let emulator = Emulator::new();
            let case = format!("address={address:?} name={name}");

            // A second group, so the delete is shown to remove only the one it names.
            let sibling = emulator
                .ok(
                    "CreateModelPackageGroup",
                    &json!({ "ModelPackageGroupName": "sibling", "ModelPackageGroupDescription": "kept" }),
                )
                .await;
            let created = emulator
                .ok(
                    "CreateModelPackageGroup",
                    &json!({ "ModelPackageGroupName": name, "ModelPackageGroupDescription": "first" }),
                )
                .await;
            let arn = str_field(&created, "ModelPackageGroupArn");

            let (status, deleted) = emulator
                .call(
                    "DeleteModelPackageGroup",
                    &json!({ "ModelPackageGroupName": address.identifier(name, &arn) }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200 from DeleteModelPackageGroup, got {status} \
                 with {deleted}"
            );

            // The group is gone: the reader refuses it rather than replaying a record.
            let (describe_status, described) = emulator
                .call(
                    "DescribeModelPackageGroup",
                    &json!({ "ModelPackageGroupName": name }),
                )
                .await;
            assert_eq!(
                describe_status, 400,
                "case {case}: DescribeModelPackageGroup after the delete must be refused, got \
                 {describe_status} with {described}"
            );
            assert_eq!(
                error_code(&described),
                "ValidationException",
                "case {case}: error code of the read-after-delete, got {described:?}"
            );

            // The group the delete did not name is untouched.
            let sibling_after = emulator
                .ok(
                    "DescribeModelPackageGroup",
                    &json!({ "ModelPackageGroupName": "sibling" }),
                )
                .await;
            assert_eq!(
                str_field(&sibling_after, "ModelPackageGroupArn"),
                str_field(&sibling, "ModelPackageGroupArn"),
                "case {case}: the sibling group must survive the delete, got {sibling_after:?}"
            );
            assert_eq!(
                sibling_after
                    .get("ModelPackageGroupDescription")
                    .and_then(Value::as_str),
                Some("kept"),
                "case {case}: the sibling group's members must be unchanged, got \
                 {sibling_after:?}"
            );

            // The name is free again, and what reads back is the NEW record, not the
            // deleted one.
            let recreated = emulator
                .ok(
                    "CreateModelPackageGroup",
                    &json!({ "ModelPackageGroupName": name, "ModelPackageGroupDescription": "second" }),
                )
                .await;
            assert_eq!(
                str_field(&recreated, "ModelPackageGroupArn"),
                arn,
                "case {case}: re-creating the freed name mints the same ARN, got {recreated:?}"
            );
            let after = emulator
                .ok(
                    "DescribeModelPackageGroup",
                    &json!({ "ModelPackageGroupName": name }),
                )
                .await;
            assert_eq!(
                after
                    .get("ModelPackageGroupDescription")
                    .and_then(Value::as_str),
                Some("second"),
                "case {case}: the re-created group must be the new record, got {after:?}"
            );
        }
    }
}

/// RULE: deleting a container while it still holds members is refused (HTTP 400
/// `ValidationException`) and removes nothing, whatever the identifier form and however
/// many members it holds.
#[tokio::test]
async fn deleting_a_group_that_still_holds_model_package_versions_is_refused() {
    // Rows differ along the number of versions registered into the group (one, a few,
    // several) and along the two `ArnOrName` addressing forms.
    for address in [Address::Name, Address::Arn] {
        for versions in [1_usize, 2, 5] {
            let emulator = Emulator::new();
            let case = format!("address={address:?} versions={versions}");
            let created = emulator
                .ok(
                    "CreateModelPackageGroup",
                    &json!({ "ModelPackageGroupName": "versioned" }),
                )
                .await;
            let arn = str_field(&created, "ModelPackageGroupArn");
            for _ in 0..versions {
                emulator
                    .ok(
                        "CreateModelPackage",
                        &json!({ "ModelPackageGroupName": "versioned" }),
                    )
                    .await;
            }
            let before = emulator
                .ok(
                    "DescribeModelPackageGroup",
                    &json!({ "ModelPackageGroupName": "versioned" }),
                )
                .await;

            let (status, refused) = emulator
                .call(
                    "DeleteModelPackageGroup",
                    &json!({ "ModelPackageGroupName": address.identifier("versioned", &arn) }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400 for a group that still holds versions, got \
                 {status} with {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {case}: error code, got {refused:?}"
            );
            assert!(
                message(&refused).contains(&arn),
                "case {case}: the refusal must name the group, got {:?}",
                message(&refused)
            );
            // The whole message, which is the same for every row: the resolved group ARN
            // and the fixed clause. The number of versions the group holds differs across
            // the rows and appears in none of them — a count is not part of this refusal,
            // and the group's contents are what the model package readers report.
            assert_eq!(
                message(&refused),
                format!(
                    "Model Package Group {arn} cannot be deleted because it still contains \
                     Model Packages."
                ),
                "case {case}: the refusal's message, got {refused:?}"
            );

            let after = emulator
                .ok(
                    "DescribeModelPackageGroup",
                    &json!({ "ModelPackageGroupName": "versioned" }),
                )
                .await;
            assert_eq!(
                after, before,
                "case {case}: a refused delete must leave the group stored and unchanged, got \
                 {after:?}"
            );
            // Every version is still registered: its own reader still answers for it.
            for version in 1..=versions {
                let package = emulator
                    .ok(
                        "DescribeModelPackage",
                        &json!({ "ModelPackageName": format!("versioned/{version}") }),
                    )
                    .await;
                assert_eq!(
                    package.get("ModelPackageVersion").and_then(Value::as_i64),
                    Some(version as i64),
                    "case {case}: version {version} must still be registered, got {package:?}"
                );
            }

            // What blocked the delete was the versions, so the SAME call succeeds once
            // they are deleted — and the last version is what releases the group, so the
            // delete is retried after each one rather than only at the end.
            for version in 1..=versions {
                emulator
                    .ok(
                        "DeleteModelPackage",
                        &json!({ "ModelPackageName": format!("versioned/{version}") }),
                    )
                    .await;
                // The version is really gone: its reader refuses it.
                let (status, gone) = emulator
                    .call(
                        "DescribeModelPackage",
                        &json!({ "ModelPackageName": format!("versioned/{version}") }),
                    )
                    .await;
                assert_eq!(
                    (status, error_code(&gone).as_str()),
                    (400, "ValidationException"),
                    "case {case}: version {version} must be gone, got {status} with {gone}"
                );

                let (status, answer) = emulator
                    .call(
                        "DeleteModelPackageGroup",
                        &json!({
                            "ModelPackageGroupName": address.identifier("versioned", &arn),
                        }),
                    )
                    .await;
                let last = version == versions;
                assert_eq!(
                    status == 200,
                    last,
                    "case {case}: after deleting {version} of {versions} versions the group \
                     delete must {} , got {status} with {answer}",
                    if last { "succeed" } else { "still be refused" }
                );
            }
            let (status, gone) = emulator
                .call(
                    "DescribeModelPackageGroup",
                    &json!({ "ModelPackageGroupName": "versioned" }),
                )
                .await;
            assert_eq!(
                (status, error_code(&gone).as_str()),
                (400, "ValidationException"),
                "case {case}: the emptied group is deleted, got {status} with {gone}"
            );
        }
    }
}

/// RULE: what blocks a container's deletion is the members it HOLDS, not resources whose
/// name merely coincides with its own — an unversioned model package belongs to no model
/// package group, so one named like the group leaves the group deletable and survives it.
#[tokio::test]
async fn an_unversioned_package_sharing_the_groups_name_does_not_block_the_delete() {
    for name in NAMES {
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreateModelPackageGroup",
                &json!({ "ModelPackageGroupName": name }),
            )
            .await;
        // `CreateModelPackage` with `ModelPackageName` registers an UNVERSIONED package,
        // which records no model package group.
        emulator
            .ok("CreateModelPackage", &json!({ "ModelPackageName": name }))
            .await;

        let (status, deleted) = emulator
            .call(
                "DeleteModelPackageGroup",
                &json!({ "ModelPackageGroupName": name }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case name={name}: an unversioned package of the same name must not block the \
             group's deletion, got {status} with {deleted}"
        );

        let package = emulator
            .ok("DescribeModelPackage", &json!({ "ModelPackageName": name }))
            .await;
        assert_eq!(
            str_field(&package, "ModelPackageName"),
            name,
            "case name={name}: the unversioned package must survive the group's deletion, got \
             {package:?}"
        );
    }
}

/// RULE: a delete given a name no resource is stored under is refused (HTTP 400
/// `ValidationException`) rather than answering an empty 200, so a repeat of a delete that
/// already succeeded is refused too.
#[tokio::test]
async fn deleting_a_group_no_record_is_stored_under_is_refused() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateModelPackageGroup",
            &json!({ "ModelPackageGroupName": "present-group" }),
        )
        .await;
    emulator
        .ok(
            "CreateModelPackageGroup",
            &json!({ "ModelPackageGroupName": "gone-group" }),
        )
        .await;
    let (status, body) = emulator
        .call(
            "DeleteModelPackageGroup",
            &json!({ "ModelPackageGroupName": "gone-group" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"first delete of gone-group\": expected HTTP 200, got {status} with {body}"
    );

    // Rows differ along what "absent" can look like: never created, a near-miss of a name
    // that does exist, a differently cased name, an ARN for a group never created, and a
    // group whose delete already succeeded.
    let absent = [
        ("never created", "missing-group".to_string()),
        ("near-miss of a stored name", "present-group-2".to_string()),
        ("differently cased", "Present-Group".to_string()),
        (
            "ARN of a group never created",
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model-package-group/missing-group"),
        ),
        ("already deleted", "gone-group".to_string()),
    ];
    for (case, identifier) in absent {
        let (status, refused) = emulator
            .call(
                "DeleteModelPackageGroup",
                &json!({ "ModelPackageGroupName": identifier }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?} ({identifier}): expected HTTP 400 for an absent group, got {status} \
             with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?} ({identifier}): error code, got {refused:?}"
        );
        let resolved = identifier.rsplit('/').next().unwrap_or(&identifier);
        assert!(
            message(&refused).contains(resolved),
            "case {case:?} ({identifier}): the refusal must name the group asked for, got {:?}",
            message(&refused)
        );
    }

    // The group nobody deleted is still there.
    let present = emulator
        .ok(
            "DescribeModelPackageGroup",
            &json!({ "ModelPackageGroupName": "present-group" }),
        )
        .await;
    assert_eq!(
        str_field(&present, "ModelPackageGroupName"),
        "present-group",
        "case \"present-group after the refusals\": got {present:?}"
    );
}

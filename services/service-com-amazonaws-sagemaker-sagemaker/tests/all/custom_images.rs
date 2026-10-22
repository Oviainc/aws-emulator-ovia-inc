// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker AI's custom-image lifecycle:
//! `CreateImage` -> `CreateImageVersion` -> `DescribeImage` / `DescribeImageVersion`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: a
//! version is minted only for an image that exists and is numbered from the image's own
//! counter, an unknown image is refused with `ResourceNotFound` whatever else the
//! request carries, every stored member is replayed, an alias names at most one version,
//! a repeated `ClientToken` mints nothing new, and the created-status transition the
//! clock owes is materialised.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Long enough to pass `state::IMAGE_CREATED_AFTER` (300 ms).
const PAST_CREATED_DUE: Duration = Duration::from_millis(600);

/// A `RoleArn` satisfying the modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/service-role/SageMakerExecution";

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

    /// `CreateImage` for `name`, required to succeed.
    async fn create_image(&self, name: &str) -> Value {
        self.ok(
            "CreateImage",
            &json!({ "ImageName": name, "RoleArn": ROLE_ARN }),
        )
        .await
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

fn i64_field(body: &Value, member: &str) -> i64 {
    body.get(member)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("expected an integer member {member} in {body}"))
}

/// A `CreateImageVersion` request body for `image_name`, with `extra`'s members merged in.
fn create_version_body(image_name: &str, token: &str, base_image: &str, extra: &Value) -> Value {
    let mut body = json!({
        "ImageName": image_name,
        "ClientToken": token,
        "BaseImage": base_image,
    });
    if let Some(members) = extra.as_object() {
        for (key, value) in members {
            body[key] = value.clone();
        }
    }
    body
}

/// An ECR registry path of the documented form
/// `<acct-id>.dkr.ecr.<region>.amazonaws.com/<repo-name[:tag] or [@digest]>`.
fn base_image(repo: &str) -> String {
    format!("123456789012.dkr.ecr.us-east-1.amazonaws.com/{repo}")
}

/// RULE: `CreateImageVersion` mints the next version of the named image and returns
/// `arn:aws:sagemaker:<signed region>:<account>:image-version/<image name, lowercased>/
/// <version>`, numbering from 1 upwards within each image independently.
#[tokio::test]
async fn image_version_arns_carry_the_lowercased_name_the_signed_region_and_the_next_number() {
    let emulator = Emulator::new();

    // Rows differ along the axes the rule generalises over: the name's shape (short,
    // mixed case, digits, hyphen, dot, and the 63-character modelled maximum), the
    // signed region, and how many versions the image already has.
    let long_name = format!("a{}", "b".repeat(62));
    let cases: Vec<(&str, &str)> = vec![
        ("img", "us-east-1"),
        ("MixedCaseImage", "eu-west-3"),
        ("image-2024", "ap-southeast-2"),
        ("my.image.name", "us-gov-west-1"),
        ("a", "sa-east-1"),
        (long_name.as_str(), "eu-central-1"),
    ];

    for (index, (name, region)) in cases.into_iter().enumerate() {
        let created = emulator
            .call_in(
                region,
                "CreateImage",
                &json!({ "ImageName": name, "RoleArn": ROLE_ARN }),
            )
            .await;
        assert_eq!(
            created.0, 200,
            "case {name}/{region}: CreateImage, got {:?}",
            created.1
        );
        assert_eq!(
            str_field(&created.1, "ImageArn"),
            format!("arn:aws:sagemaker:{region}:{ACCOUNT}:image/{name}"),
            "case {name}/{region}: ImageArn, got {:?}",
            created.1
        );

        // Three versions in a row: the number is the image's own counter, not a global one.
        for expected_version in 1..=3 {
            let body = create_version_body(
                name,
                // `ClientToken` is modelled `len 1..36, pattern ^[a-zA-Z0-9-]+$`, so the
                // token is built from the row index rather than from the image name.
                &format!("token-{index}-{expected_version}"),
                &base_image(&format!("repo:{expected_version}")),
                &json!({}),
            );
            let version = emulator.call_in(region, "CreateImageVersion", &body).await;
            assert_eq!(
                version.0, 200,
                "case {name}/{region}/v{expected_version}: CreateImageVersion, got {:?}",
                version.1
            );
            assert_eq!(
                str_field(&version.1, "ImageVersionArn"),
                format!(
                    "arn:aws:sagemaker:{region}:{ACCOUNT}:image-version/{}/{expected_version}",
                    name.to_lowercase()
                ),
                "case {name}/{region}/v{expected_version}: ImageVersionArn, got {:?}",
                version.1
            );

            let described = emulator
                .call_in(
                    region,
                    "DescribeImageVersion",
                    &json!({ "ImageName": name, "Version": expected_version }),
                )
                .await;
            assert_eq!(
                described.0, 200,
                "case {name}/{region}/v{expected_version}: DescribeImageVersion, got {:?}",
                described.1
            );
            assert_eq!(
                i64_field(&described.1, "Version"),
                i64::from(expected_version),
                "case {name}/{region}/v{expected_version}: Version read back, got {:?}",
                described.1
            );
            assert_eq!(
                str_field(&described.1, "ImageArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:image/{name}"),
                "case {name}/{region}/v{expected_version}: the version names its image, \
                 got {:?}",
                described.1
            );
        }
    }

    // An unsigned request names no region, so the ARN carries the emulator's default.
    let (status, body) = emulator
        .call_unsigned(
            "CreateImage",
            &json!({ "ImageName": "unsigned-image", "RoleArn": ROLE_ARN }),
        )
        .await;
    assert_eq!(status, 200, "case unsigned: CreateImage, got {status} {body}");
    let (status, body) = emulator
        .call_unsigned(
            "CreateImageVersion",
            &create_version_body(
                "unsigned-image",
                "token-unsigned",
                &base_image("repo:1"),
                &json!({}),
            ),
        )
        .await;
    assert_eq!(
        status, 200,
        "case unsigned: CreateImageVersion, got {status} {body}"
    );
    assert_eq!(
        str_field(&body, "ImageVersionArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:image-version/unsigned-image/1"),
        "case unsigned: ImageVersionArn must fall back to the default region, got {body:?}"
    );
}

/// RULE: a version exists only as a version OF an image, so `CreateImageVersion` for an
/// `ImageName` no image is stored under is refused with `ResourceNotFound` (HTTP 400) and
/// persists nothing — whatever else the request carries.
#[tokio::test]
async fn creating_a_version_of_an_unknown_image_is_refused_as_resource_not_found() {
    let emulator = Emulator::new();

    // A neighbouring image exists, so the refusal is about the NAMED image rather than
    // about an empty store.
    emulator.create_image("existing").await;

    // Rows differ along the axes the rule generalises over: the unknown name's shape, the
    // base image's form (tag, digest, bare repo), and which optional members are present.
    let cases: Vec<(&str, &str, Value)> = vec![
        ("missing", "repo:latest", json!({})),
        (
            "Existing",
            "repo@sha256:0000000000000000000000000000000000000000000000000000000000000000",
            json!({ "JobType": "TRAINING" }),
        ),
        ("existing2", "repo", json!({ "Aliases": ["one", "two"] })),
        (
            "deleted.image",
            "team/repo:1.2.3",
            json!({ "VendorGuidance": "STABLE", "Horovod": true }),
        ),
        ("x", "repo:x", json!({ "ReleaseNotes": "first cut" })),
    ];

    for (index, (name, repo, extra)) in cases.iter().enumerate() {
        let body = create_version_body(
            name,
            &format!("token-missing-{index}"),
            &base_image(repo),
            extra,
        );
        let (status, parsed) = emulator.call("CreateImageVersion", &body).await;
        assert_eq!(
            status, 400,
            "case {name}: expected HTTP 400 for an unknown image, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {name}: expected ResourceNotFound, got {parsed:?}"
        );

        // Nothing was persisted: the version is not readable either.
        let (status, parsed) = emulator
            .call("DescribeImageVersion", &json!({ "ImageName": name }))
            .await;
        assert_eq!(
            status, 400,
            "case {name}: DescribeImageVersion after the refusal, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {name}: expected ResourceNotFound from DescribeImageVersion, got {parsed:?}"
        );
    }

    // The refusals consumed no version number from the image that does exist.
    let minted = emulator
        .ok(
            "CreateImageVersion",
            &create_version_body("existing", "token-after", &base_image("repo:1"), &json!({})),
        )
        .await;
    assert_eq!(
        str_field(&minted, "ImageVersionArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:image-version/existing/1"),
        "case after-refusals: the first version of the existing image is still 1, got {minted:?}"
    );

    // `DescribeImage` applies the same rule to the image itself.
    let (status, parsed) = emulator
        .call("DescribeImage", &json!({ "ImageName": "missing" }))
        .await;
    assert_eq!(
        status, 400,
        "case describe-unknown-image: got {status} with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceNotFound",
        "case describe-unknown-image: expected ResourceNotFound, got {parsed:?}"
    );
}

/// RULE: `DescribeImageVersion` replays every member `CreateImageVersion` stored —
/// present members and absent ones alike — and a version created without
/// `VendorGuidance` reports `NOT_PROVIDED`.
#[tokio::test]
async fn describe_image_version_replays_every_stored_member() {
    let emulator = Emulator::new();
    emulator.create_image("roundtrip").await;

    // The enum catalogs come from the model (smithy_query operation=CreateImageVersion):
    // JobType one of INFERENCE | NOTEBOOK_KERNEL | TRAINING, Processor one of CPU | GPU,
    // VendorGuidance one of ARCHIVED | NOT_PROVIDED | STABLE | TO_BE_ARCHIVED. Every
    // member of each appears in some row below, plus the all-absent and all-present rows.
    let cases: Vec<(&str, Value)> = vec![
        ("no-optional-members", json!({})),
        (
            "every-member",
            json!({
                "VendorGuidance": "STABLE",
                "JobType": "INFERENCE",
                "MLFramework": "TensorFlow 2.9",
                "ProgrammingLang": "Python 3.10",
                "Processor": "GPU",
                "Horovod": true,
                "ReleaseNotes": "first release",
            }),
        ),
        (
            "notebook-kernel-cpu",
            json!({
                "JobType": "NOTEBOOK_KERNEL",
                "Processor": "CPU",
                "Horovod": false,
                "VendorGuidance": "TO_BE_ARCHIVED",
            }),
        ),
        (
            "training-archived",
            json!({
                "JobType": "TRAINING",
                "VendorGuidance": "ARCHIVED",
                "MLFramework": "PyTorch 2.1.0",
                "ProgrammingLang": "Python 3.11",
            }),
        ),
        (
            "explicitly-not-provided",
            json!({ "VendorGuidance": "NOT_PROVIDED", "ReleaseNotes": "unmaintained" }),
        ),
    ];

    /// The optional members `CreateImageVersion` stores and `DescribeImageVersion` replays.
    const ECHOED: [&str; 7] = [
        "VendorGuidance",
        "JobType",
        "MLFramework",
        "ProgrammingLang",
        "Processor",
        "Horovod",
        "ReleaseNotes",
    ];

    for (index, (case, extra)) in cases.iter().enumerate() {
        let version = i32::try_from(index + 1).expect("the row index fits an i32");
        let registry_path = base_image(&format!("roundtrip:{case}"));
        let created = emulator
            .ok(
                "CreateImageVersion",
                &create_version_body(
                    "roundtrip",
                    &format!("token-{index}"),
                    &registry_path,
                    extra,
                ),
            )
            .await;
        let version_arn = str_field(&created, "ImageVersionArn");

        let described = emulator
            .ok(
                "DescribeImageVersion",
                &json!({ "ImageName": "roundtrip", "Version": version }),
            )
            .await;
        assert_eq!(
            str_field(&described, "ImageVersionArn"),
            version_arn,
            "case {case}: the described version is the created one, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "BaseImage"),
            registry_path,
            "case {case}: BaseImage read back, got {described:?}"
        );

        for member in ECHOED {
            match extra.get(member) {
                Some(sent) => assert_eq!(
                    described.get(member),
                    Some(sent),
                    "case {case}: {member} must be replayed as sent, got {:?}",
                    described.get(member)
                ),
                // A version created without `VendorGuidance` reports the member the model
                // documents as "the maintainers did not provide a status".
                None if member == "VendorGuidance" => assert_eq!(
                    described.get(member).and_then(Value::as_str),
                    Some("NOT_PROVIDED"),
                    "case {case}: VendorGuidance defaults to NOT_PROVIDED, got {:?}",
                    described.get(member)
                ),
                None => assert!(
                    described.get(member).is_none(),
                    "case {case}: {member} was not sent, so it must be absent, got {:?}",
                    described.get(member)
                ),
            }
        }
    }
}

/// RULE: `ClientToken` is an `@idempotencyToken`, so a repeat of the same create answers
/// with the version the first call minted and mints nothing new; a different token mints
/// the next version.
#[tokio::test]
async fn repeating_a_client_token_mints_no_second_version() {
    let emulator = Emulator::new();
    emulator.create_image("idem").await;

    // Rows differ along the axes the rule generalises over: how many times the call is
    // repeated, and whether the repeat carries the same or different other members.
    let cases: Vec<(&str, usize, Value)> = vec![
        ("same-request-twice", 2, json!({})),
        ("same-request-five-times", 5, json!({ "JobType": "TRAINING" })),
        (
            "same-token-other-members",
            3,
            json!({ "Processor": "CPU", "Horovod": true }),
        ),
    ];

    let mut expected_version = 0;
    for (case, repeats, extra) in cases {
        expected_version += 1;
        let token = format!("token-{case}");
        let mut arns = Vec::new();
        for attempt in 0..repeats {
            let body = create_version_body(
                "idem",
                &token,
                &base_image(&format!("idem:{case}-{attempt}")),
                &extra,
            );
            let created = emulator.ok("CreateImageVersion", &body).await;
            arns.push(str_field(&created, "ImageVersionArn"));
        }
        let expected_arn = format!(
            "arn:aws:sagemaker:us-east-1:{ACCOUNT}:image-version/idem/{expected_version}"
        );
        assert!(
            arns.iter().all(|arn| *arn == expected_arn),
            "case {case:?}: every retry must answer with {expected_arn}, got {arns:?}"
        );

        // And the retries left exactly one version: the next number is the one after it.
        let described = emulator
            .ok(
                "DescribeImageVersion",
                &json!({ "ImageName": "idem", "Version": expected_version }),
            )
            .await;
        assert_eq!(
            str_field(&described, "BaseImage"),
            base_image(&format!("idem:{case}-0")),
            "case {case:?}: the stored version is the one the FIRST call created, got \
             {described:?}"
        );
        let (status, parsed) = emulator
            .call(
                "DescribeImageVersion",
                &json!({ "ImageName": "idem", "Version": expected_version + 1 }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the retries must not have minted version \
             {}, got {parsed:?}",
            expected_version + 1
        );
    }
}

/// RULE: an alias names at most one version of an image — `DescribeImageVersion(Alias)`
/// resolves to the version carrying it, and a create reusing an alias already attached to
/// another version of the same image is refused with `ResourceInUse` and mints nothing.
#[tokio::test]
async fn an_alias_names_one_version_and_a_reused_alias_is_refused() {
    let emulator = Emulator::new();
    emulator.create_image("aliased").await;
    emulator.create_image("other").await;

    // Rows differ along the axes the rule generalises over: the number of aliases on one
    // version and the alias's own shape (letters, digits, hyphen, underscore, dot), all
    // within the modelled pattern ^(?!^[.-])^([a-zA-Z0-9-_.]+)$.
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("single", vec!["latest"]),
        ("two", vec!["1.0", "stable_1"]),
        ("three", vec!["gpu-cu11", "tf2", "a"]),
    ];

    for (index, (case, aliases)) in cases.iter().enumerate() {
        let version = i32::try_from(index + 1).expect("the row index fits an i32");
        emulator
            .ok(
                "CreateImageVersion",
                &create_version_body(
                    "aliased",
                    &format!("alias-token-{index}"),
                    &base_image(&format!("aliased:{case}")),
                    &json!({ "Aliases": aliases }),
                ),
            )
            .await;

        for alias in aliases {
            let described = emulator
                .ok(
                    "DescribeImageVersion",
                    &json!({ "ImageName": "aliased", "Alias": alias }),
                )
                .await;
            assert_eq!(
                i64_field(&described, "Version"),
                i64::from(version),
                "case {case}/{alias}: the alias must resolve to the version it was \
                 created with, got {described:?}"
            );
        }
    }

    // Reusing an alias of ANY version of the same image is refused, and nothing is minted.
    for (index, alias) in ["latest", "1.0", "a"].into_iter().enumerate() {
        let (status, parsed) = emulator
            .call(
                "CreateImageVersion",
                &create_version_body(
                    "aliased",
                    &format!("reuse-token-{index}"),
                    &base_image("aliased:reuse"),
                    &json!({ "Aliases": [alias, "fresh-alias"] }),
                ),
            )
            .await;
        assert_eq!(
            status, 400,
            "case reuse-{alias}: expected HTTP 400, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceInUse",
            "case reuse-{alias}: expected ResourceInUse, got {parsed:?}"
        );
        let (status, parsed) = emulator
            .call(
                "DescribeImageVersion",
                &json!({ "ImageName": "aliased", "Alias": "fresh-alias" }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case reuse-{alias}: the refused create must have stored no alias, got \
             {status} with {parsed}"
        );
    }

    // An alias is scoped to its image: the same text is free on another image, and an
    // alias of one image never resolves on another.
    emulator
        .ok(
            "CreateImageVersion",
            &create_version_body(
                "other",
                "other-token",
                &base_image("other:1"),
                &json!({ "Aliases": ["latest"] }),
            ),
        )
        .await;
    let described = emulator
        .ok(
            "DescribeImageVersion",
            &json!({ "ImageName": "other", "Alias": "latest" }),
        )
        .await;
    assert_eq!(
        str_field(&described, "ImageVersionArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:image-version/other/1"),
        "case cross-image-alias: the alias resolves within its own image, got {described:?}"
    );
    let (status, parsed) = emulator
        .call(
            "DescribeImageVersion",
            &json!({ "ImageName": "other", "Alias": "tf2" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case foreign-alias: an alias of another image must not resolve, got {status} \
         with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceNotFound",
        "case foreign-alias: expected ResourceNotFound, got {parsed:?}"
    );

    // `Aliases` carries no modelled length, so the service applies its own bound
    // (`state::MAX_ALIASES_PER_IMAGE_VERSION` = 50): the bound itself is accepted and one
    // past it is refused with the modelled limit error, minting nothing.
    emulator.create_image("capped").await;
    let at_bound: Vec<String> = (0..50).map(|index| format!("alias-{index}")).collect();
    let accepted = emulator
        .ok(
            "CreateImageVersion",
            &create_version_body(
                "capped",
                "capped-token-0",
                &base_image("capped:1"),
                &json!({ "Aliases": at_bound }),
            ),
        )
        .await;
    assert_eq!(
        str_field(&accepted, "ImageVersionArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:image-version/capped/1"),
        "case aliases-at-the-bound: 50 aliases must be accepted, got {accepted:?}"
    );

    let past_bound: Vec<String> = (0..51).map(|index| format!("over-{index}")).collect();
    let (status, parsed) = emulator
        .call(
            "CreateImageVersion",
            &create_version_body(
                "capped",
                "capped-token-1",
                &base_image("capped:2"),
                &json!({ "Aliases": past_bound }),
            ),
        )
        .await;
    assert_eq!(
        status, 400,
        "case aliases-past-the-bound: expected HTTP 400, got {status} with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceLimitExceeded",
        "case aliases-past-the-bound: expected ResourceLimitExceeded, got {parsed:?}"
    );
    let (status, parsed) = emulator
        .call(
            "DescribeImageVersion",
            &json!({ "ImageName": "capped", "Version": 2 }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case aliases-past-the-bound: the refused create must have minted nothing, got \
         {status} with {parsed}"
    );
}

/// RULE: with neither `Version` nor `Alias`, `DescribeImageVersion` describes the latest
/// version of the image; `Version` and `Alias` together select nothing and are refused.
#[tokio::test]
async fn describe_defaults_to_the_latest_version_and_refuses_two_selectors() {
    let emulator = Emulator::new();
    emulator.create_image("latest").await;

    // The latest version moves with every create, for each count in turn.
    for expected_latest in 1..=4 {
        emulator
            .ok(
                "CreateImageVersion",
                &create_version_body(
                    "latest",
                    &format!("latest-token-{expected_latest}"),
                    &base_image(&format!("latest:{expected_latest}")),
                    &json!({}),
                ),
            )
            .await;
        let described = emulator
            .ok("DescribeImageVersion", &json!({ "ImageName": "latest" }))
            .await;
        assert_eq!(
            i64_field(&described, "Version"),
            i64::from(expected_latest),
            "case after-{expected_latest}-versions: the default selection is the latest, \
             got {described:?}"
        );
        assert_eq!(
            str_field(&described, "BaseImage"),
            base_image(&format!("latest:{expected_latest}")),
            "case after-{expected_latest}-versions: BaseImage of the latest, got \
             {described:?}"
        );
    }

    // An image with no version yet has no latest version to describe.
    emulator.create_image("versionless").await;
    let (status, parsed) = emulator
        .call("DescribeImageVersion", &json!({ "ImageName": "versionless" }))
        .await;
    assert_eq!(
        status, 400,
        "case versionless: got {status} with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceNotFound",
        "case versionless: expected ResourceNotFound, got {parsed:?}"
    );

    // Two selectors name no single version, whichever pair is sent.
    for (version, alias) in [(1, "latest"), (4, "missing-alias"), (99, "x")] {
        let (status, parsed) = emulator
            .call(
                "DescribeImageVersion",
                &json!({ "ImageName": "latest", "Version": version, "Alias": alias }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case version-{version}-and-alias-{alias}: got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case version-{version}-and-alias-{alias}: expected ValidationException, got \
             {parsed:?}"
        );
    }
}

/// RULE: a created image and image version are reported `CREATING` until the instant the
/// clock owes them the move to `CREATED`, and the `LastModifiedTime` the transition
/// records is stable across later reads.
#[tokio::test]
async fn a_created_image_and_version_move_from_creating_to_created() {
    let emulator = Emulator::new();
    emulator.create_image("clock").await;
    emulator
        .ok(
            "CreateImageVersion",
            &create_version_body("clock", "clock-token", &base_image("clock:1"), &json!({})),
        )
        .await;

    let image = emulator
        .ok("DescribeImage", &json!({ "ImageName": "clock" }))
        .await;
    assert_eq!(
        str_field(&image, "ImageStatus"),
        "CREATING",
        "case immediately-after-create: ImageStatus, got {image:?}"
    );
    let version = emulator
        .ok("DescribeImageVersion", &json!({ "ImageName": "clock" }))
        .await;
    assert_eq!(
        str_field(&version, "ImageVersionStatus"),
        "CREATING",
        "case immediately-after-create: ImageVersionStatus, got {version:?}"
    );

    tokio::time::sleep(PAST_CREATED_DUE).await;

    let settled_image = emulator
        .ok("DescribeImage", &json!({ "ImageName": "clock" }))
        .await;
    assert_eq!(
        str_field(&settled_image, "ImageStatus"),
        "CREATED",
        "case after-the-due-instant: ImageStatus, got {settled_image:?}"
    );
    let settled_version = emulator
        .ok("DescribeImageVersion", &json!({ "ImageName": "clock" }))
        .await;
    assert_eq!(
        str_field(&settled_version, "ImageVersionStatus"),
        "CREATED",
        "case after-the-due-instant: ImageVersionStatus, got {settled_version:?}"
    );

    // Read again: the materialised transition is a fact about the past, so neither the
    // status nor the time it stamped moves with the reader.
    let reread_image = emulator
        .ok("DescribeImage", &json!({ "ImageName": "clock" }))
        .await;
    let reread_version = emulator
        .ok("DescribeImageVersion", &json!({ "ImageName": "clock" }))
        .await;
    for (case, first, second) in [
        ("image", &settled_image, &reread_image),
        ("version", &settled_version, &reread_version),
    ] {
        assert_eq!(
            first.get("LastModifiedTime"),
            second.get("LastModifiedTime"),
            "case {case}: LastModifiedTime must be stable across reads, got {:?} then {:?}",
            first.get("LastModifiedTime"),
            second.get("LastModifiedTime")
        );
        assert_eq!(
            first.get("CreationTime"),
            second.get("CreationTime"),
            "case {case}: CreationTime must be stable across reads, got {:?} then {:?}",
            first.get("CreationTime"),
            second.get("CreationTime")
        );
    }
}

/// RULE: `ImageName` is unique to the account, so `CreateImage` refuses a name already in
/// use with `ResourceInUse` and leaves the stored image untouched; `DescribeImage`
/// replays every member the accepted create stored.
#[tokio::test]
async fn create_image_round_trips_and_refuses_a_name_already_in_use() {
    let emulator = Emulator::new();

    // Rows differ along the axes the rule generalises over: which optional members the
    // create carried.
    let cases: Vec<(&str, Value)> = vec![
        ("bare", json!({})),
        ("described", json!({ "Description": "a demo image" })),
        (
            "named-and-described",
            json!({ "Description": "both members", "DisplayName": "Demo Image" }),
        ),
        ("displayed", json!({ "DisplayName": "Display Only" })),
        (
            "tagged",
            json!({ "Tags": [{ "Key": "team", "Value": "ml" }] }),
        ),
    ];

    for (case, extra) in &cases {
        let mut body = json!({ "ImageName": case, "RoleArn": ROLE_ARN });
        for (key, value) in extra.as_object().expect("the row is an object") {
            body[key] = value.clone();
        }
        let created = emulator.ok("CreateImage", &body).await;
        assert_eq!(
            str_field(&created, "ImageArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:image/{case}"),
            "case {case}: ImageArn, got {created:?}"
        );

        let described = emulator
            .ok("DescribeImage", &json!({ "ImageName": case }))
            .await;
        assert_eq!(
            str_field(&described, "ImageName"),
            *case,
            "case {case}: ImageName read back, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "RoleArn"),
            ROLE_ARN,
            "case {case}: RoleArn read back, got {described:?}"
        );
        for member in ["Description", "DisplayName"] {
            assert_eq!(
                described.get(member),
                extra.get(member),
                "case {case}: {member} must be replayed exactly as sent, got {:?}",
                described.get(member)
            );
        }

        // The second create of the same name is refused and changes nothing.
        let (status, parsed) = emulator
            .call(
                "CreateImage",
                &json!({ "ImageName": case, "RoleArn": ROLE_ARN, "Description": "overwrite" }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: the duplicate create must be refused, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceInUse",
            "case {case}: expected ResourceInUse, got {parsed:?}"
        );
        let reread = emulator
            .ok("DescribeImage", &json!({ "ImageName": case }))
            .await;
        assert_eq!(
            reread.get("Description"),
            extra.get("Description"),
            "case {case}: the refused create must not have overwritten Description, got \
             {reread:?}"
        );
    }
}

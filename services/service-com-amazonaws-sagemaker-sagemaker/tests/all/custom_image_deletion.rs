// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `DeleteImage`: releasing a SageMaker AI custom image.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: an
//! accepted delete releases the image together with EVERY version of it whatever the
//! version count, name shape, region or version status, a name no image is stored under
//! is refused with `ResourceNotFound`, and an image a Studio app still runs on is in use
//! and refused with `ResourceInUse` while staying readable.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Long enough to pass `state::IMAGE_CREATED_AFTER` (300 ms), so a version the clock owes
/// the `CREATING` -> `CREATED` move has been moved by the time it is read.
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
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.body();
        let parsed = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(raw).unwrap_or(Value::Null)
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

    /// `CreateImage` for `name` in `region`, required to succeed.
    async fn create_image_in(&self, region: &str, name: &str) -> Value {
        let (status, parsed) = self
            .call_in(
                region,
                "CreateImage",
                &json!({ "ImageName": name, "RoleArn": ROLE_ARN }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {name}/{region}: expected HTTP 200 from CreateImage, got {status} with {parsed}"
        );
        parsed
    }

    /// `CreateImageVersion` for `name` in `region`, required to succeed.
    async fn create_version_in(&self, region: &str, name: &str, token: &str) -> Value {
        let (status, parsed) = self
            .call_in(
                region,
                "CreateImageVersion",
                &json!({
                    "ImageName": name,
                    "ClientToken": token,
                    // An ECR registry path of the documented form
                    // `<acct-id>.dkr.ecr.<region>.amazonaws.com/<repo-name[:tag]>`.
                    "BaseImage": format!("123456789012.dkr.ecr.{region}.amazonaws.com/repo:{token}"),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {name}/{region}/{token}: expected HTTP 200 from CreateImageVersion, \
             got {status} with {parsed}"
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

fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The sentence the service refuses with for an image no store holds, for an image named
/// `name` in `region` — the ARN the request resolves to inside the service's wording.
fn image_gone_message(region: &str, name: &str) -> String {
    format!("Image with ARN arn:aws:sagemaker:{region}:{ACCOUNT}:image/{name} does not exist")
}

fn str_field(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string member {member} in {body}"))
        .to_string()
}

/// RULE: an accepted `DeleteImage` releases the named image together with EVERY version of
/// it, so neither the image nor any of its versions is readable afterwards and the name is
/// free for a new image whose versions number from 1 again.
#[tokio::test]
async fn deleting_an_image_releases_it_with_every_version_of_it() {
    let emulator = Emulator::new();

    // Rows differ along the axes the rule generalises over: how many versions the image
    // holds (none, one, several), the name's shape (short, mixed case, digits, hyphen,
    // dot, and the 63-character modelled maximum), the signed region, and whether the
    // versions are still `CREATING` or have been moved to `CREATED` by the clock.
    let longest_name = format!("a{}", "b".repeat(62));
    let cases: Vec<(&str, u32, &str, bool)> = vec![
        ("img", 0, "us-east-1", false),
        ("MixedCaseImage", 1, "eu-west-3", false),
        ("image-2024", 3, "ap-southeast-2", false),
        ("my.image.name", 2, "us-gov-west-1", true),
        ("a", 1, "sa-east-1", true),
        (longest_name.as_str(), 1, "eu-central-1", false),
    ];

    for (index, (name, version_count, region, settled)) in cases.into_iter().enumerate() {
        emulator.create_image_in(region, name).await;
        for version in 1..=version_count {
            emulator
                .create_version_in(region, name, &format!("token-{index}-{version}"))
                .await;
        }
        if settled {
            tokio::time::sleep(PAST_CREATED_DUE).await;
            let described = emulator
                .call_in(
                    region,
                    "DescribeImageVersion",
                    &json!({ "ImageName": name, "Version": 1 }),
                )
                .await;
            assert_eq!(
                str_field(&described.1, "ImageVersionStatus"),
                "CREATED",
                "case {name}/{region}: a version past its due instant reads CREATED \
                 before the delete, got {:?}",
                described.1
            );
        }

        let (status, parsed) = emulator
            .call_in(region, "DeleteImage", &json!({ "ImageName": name }))
            .await;
        assert_eq!(
            status, 200,
            "case {name}/{region} with {version_count} version(s): expected HTTP 200 from \
             DeleteImage, got {status} with {parsed}"
        );

        let (status, parsed) = emulator
            .call_in(region, "DescribeImage", &json!({ "ImageName": name }))
            .await;
        assert_eq!(
            status, 400,
            "case {name}/{region}: DescribeImage after the delete, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {name}/{region}: expected ResourceNotFound from DescribeImage, got {parsed:?}"
        );
        assert_eq!(
            message(&parsed),
            image_gone_message(region, name),
            "case {name}/{region}: the refusal names the image it could not find, \
             got {parsed:?}"
        );

        // Every version went with the image, whatever its number or status.
        for version in 1..=version_count {
            let (status, parsed) = emulator
                .call_in(
                    region,
                    "DescribeImageVersion",
                    &json!({ "ImageName": name, "Version": version }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {name}/{region}/v{version}: DescribeImageVersion after the delete, \
                 got {status} with {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "ResourceNotFound",
                "case {name}/{region}/v{version}: expected ResourceNotFound from \
                 DescribeImageVersion, got {parsed:?}"
            );
        }
        // The version-less form of the reader is refused too: the image holds no latest
        // version any more.
        let (status, parsed) = emulator
            .call_in(region, "DescribeImageVersion", &json!({ "ImageName": name }))
            .await;
        assert_eq!(
            status, 400,
            "case {name}/{region}: DescribeImageVersion with no Version after the delete, \
             got {status} with {parsed}"
        );

        // The name is free again, and the new image's versions number from 1: nothing of
        // the released image was inherited.
        emulator.create_image_in(region, name).await;
        let minted = emulator
            .create_version_in(region, name, &format!("token-{index}-again"))
            .await;
        assert_eq!(
            str_field(&minted, "ImageVersionArn"),
            format!(
                "arn:aws:sagemaker:{region}:{ACCOUNT}:image-version/{}/1",
                name.to_lowercase()
            ),
            "case {name}/{region}: the re-created image's first version, got {minted:?}"
        );
    }
}

/// RULE: `DeleteImage` names one image by `ImageName`, so a name no image is stored under
/// — never created, already released, or differing only in case — is refused with
/// `ResourceNotFound` (HTTP 400) and releases nothing else.
#[tokio::test]
async fn deleting_an_image_no_store_holds_is_refused_as_resource_not_found() {
    let emulator = Emulator::new();

    // A neighbouring image with a version exists throughout, so each refusal is about the
    // NAMED image rather than about an empty store.
    emulator.create_image_in("us-east-1", "existing").await;
    emulator
        .create_version_in("us-east-1", "existing", "token-neighbour")
        .await;
    // One image that WAS there and was released: a repeated delete is a delete of a name
    // nothing is stored under.
    emulator.create_image_in("us-east-1", "gone").await;
    emulator.ok("DeleteImage", &json!({ "ImageName": "gone" })).await;

    // Rows differ along the axes the rule generalises over: a name never created, one a
    // delete already released, one differing from a stored name only in case, and names at
    // the shortest and longest modelled lengths.
    let longest_unknown = format!("z{}", "y".repeat(62));
    let cases: Vec<&str> = vec![
        "missing",
        "gone",
        "Existing",
        "x",
        longest_unknown.as_str(),
    ];

    for name in cases {
        let (status, parsed) = emulator
            .call("DeleteImage", &json!({ "ImageName": name }))
            .await;
        assert_eq!(
            status, 400,
            "case {name}: expected HTTP 400 for an unknown image, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {name}: expected ResourceNotFound, got {parsed:?}"
        );
        assert_eq!(
            message(&parsed),
            image_gone_message("us-east-1", name),
            "case {name}: the refusal names the image it could not find, got {parsed:?}"
        );

        // The refusal released nothing: the neighbour and its version are still readable.
        let described = emulator
            .ok("DescribeImage", &json!({ "ImageName": "existing" }))
            .await;
        assert_eq!(
            str_field(&described, "ImageName"),
            "existing",
            "case {name}: the neighbouring image survives the refusal, got {described:?}"
        );
        let version = emulator
            .ok(
                "DescribeImageVersion",
                &json!({ "ImageName": "existing", "Version": 1 }),
            )
            .await;
        assert_eq!(
            str_field(&version, "ImageVersionArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:image-version/existing/1"),
            "case {name}: the neighbour's version survives the refusal, got {version:?}"
        );
    }
}

/// RULE: an image a Studio app is still running on is in use, so `DeleteImage` is refused
/// with `ResourceInUse` (HTTP 400) and the image and its versions stay readable; once no
/// app names it, the same delete is accepted.
#[tokio::test]
async fn an_image_a_studio_app_runs_on_is_in_use_and_refused() {
    let emulator = Emulator::new();

    // Rows differ along the axis the rule generalises over: WHICH member of the app's
    // `ResourceSpec` names the image — the image itself or one of its versions — both of
    // which are ways an app runs on it (smithy_query shape=ResourceSpec).
    let cases: Vec<(&str, &str, bool)> = vec![
        ("named-by-image-arn", "shared-by-arn", false),
        ("named-by-version-arn", "shared-by-version", true),
    ];

    for (case, image_name, by_version) in cases {
        emulator.create_image_in("us-east-1", image_name).await;
        emulator
            .create_version_in("us-east-1", image_name, &format!("token-{image_name}"))
            .await;

        let image_arn = format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:image/{image_name}");
        let version_arn =
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:image-version/{image_name}/1");
        let resource_spec = if by_version {
            json!({ "SageMakerImageVersionArn": version_arn })
        } else {
            json!({ "SageMakerImageArn": image_arn })
        };
        let app = json!({
            "DomainId": "d-abc123",
            "UserProfileName": "studio-user",
            "AppType": "KernelGateway",
            "AppName": format!("app-{case}"),
        });
        let mut create_app = app
            .as_object()
            .expect("the identifying members form an object")
            .clone();
        create_app.insert("ResourceSpec".to_string(), resource_spec);
        emulator
            .ok("CreateApp", &Value::Object(create_app))
            .await;

        let (status, parsed) = emulator
            .call("DeleteImage", &json!({ "ImageName": image_name }))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400 while an app runs on the image, \
             got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceInUse",
            "case {case}: expected ResourceInUse, got {parsed:?}"
        );

        // The refused delete released nothing: image and version are both still readable.
        let described = emulator
            .ok("DescribeImage", &json!({ "ImageName": image_name }))
            .await;
        assert_eq!(
            str_field(&described, "ImageArn"),
            image_arn,
            "case {case}: the image survives the refusal, got {described:?}"
        );
        let version = emulator
            .ok(
                "DescribeImageVersion",
                &json!({ "ImageName": image_name, "Version": 1 }),
            )
            .await;
        assert_eq!(
            str_field(&version, "ImageVersionArn"),
            version_arn,
            "case {case}: the version survives the refusal, got {version:?}"
        );

        // With the app released, nothing runs on the image any more and the delete goes
        // through.
        emulator.ok("DeleteApp", &app).await;
        let (status, parsed) = emulator
            .call("DeleteImage", &json!({ "ImageName": image_name }))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 once no app names the image, \
             got {status} with {parsed}"
        );
        let (status, parsed) = emulator
            .call("DescribeImage", &json!({ "ImageName": image_name }))
            .await;
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {case}: the image is gone after the accepted delete, got {status} {parsed:?}"
        );
    }

    // An app running on ANOTHER image does not make this one in use: the check is about
    // the image the request names, not about any app existing.
    emulator.create_image_in("us-east-1", "unused-image").await;
    emulator
        .ok(
            "CreateApp",
            &json!({
                "DomainId": "d-abc123",
                "UserProfileName": "studio-user",
                "AppType": "JupyterLab",
                "AppName": "app-on-other-image",
                "ResourceSpec": {
                    "SageMakerImageArn":
                        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:image/some-other-image"),
                },
            }),
        )
        .await;
    let (status, parsed) = emulator
        .call("DeleteImage", &json!({ "ImageName": "unused-image" }))
        .await;
    assert_eq!(
        status, 200,
        "case app-on-another-image: expected HTTP 200, got {status} with {parsed}"
    );
}

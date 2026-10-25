// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `DeleteImageVersion`: releasing ONE version of a SageMaker AI
//! custom image.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: an
//! accepted delete releases exactly the version the request's `Version` or `Alias`
//! selects and nothing else, a selection that resolves to no stored version is refused
//! with `ResourceNotFound`, a request naming neither or both selectors is refused with
//! `ValidationException`, and a version a Studio app still runs on is in use and refused
//! with `ResourceInUse` while staying readable.

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
    async fn create_image_in(&self, region: &str, name: &str) {
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
    }

    /// `CreateImageVersion` for `name` in `region` with `aliases` attached, required to
    /// succeed; answers the minted `ImageVersionArn`.
    async fn create_version_in(
        &self,
        region: &str,
        name: &str,
        token: &str,
        aliases: &[&str],
    ) -> String {
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
                    "Aliases": aliases,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {name}/{region}/{token}: expected HTTP 200 from CreateImageVersion, \
             got {status} with {parsed}"
        );
        str_field(&parsed, "ImageVersionArn")
    }

    /// `DescribeImageVersion` for one version number of `name` in `region`.
    async fn describe_version_in(&self, region: &str, name: &str, version: i64) -> (u16, Value) {
        self.call_in(
            region,
            "DescribeImageVersion",
            &json!({ "ImageName": name, "Version": version }),
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

fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn str_field(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string member {member} in {body}"))
        .to_string()
}

/// The ARN a version of `name` numbered `version` carries in `region`: the image name
/// lowercased, as the modelled `ImageVersionArn` pattern spells it.
fn version_arn(region: &str, name: &str, version: i64) -> String {
    format!(
        "arn:aws:sagemaker:{region}:{ACCOUNT}:image-version/{}/{version}",
        name.to_lowercase()
    )
}

/// How the request named the one version to delete. The two members are alternatives,
/// so one row of a table picks one of them (smithy_query operation=DeleteImageVersion:
/// `Version` and `Alias` beside the required `ImageName`).
#[derive(Debug, Clone, Copy)]
enum By {
    Version(i64),
    Alias(&'static str),
}

impl By {
    /// The `DeleteImageVersion` body selecting this version of `image_name`.
    fn request(&self, image_name: &str) -> Value {
        match self {
            Self::Version(version) => json!({ "ImageName": image_name, "Version": version }),
            Self::Alias(alias) => json!({ "ImageName": image_name, "Alias": alias }),
        }
    }
}

/// RULE: an accepted `DeleteImageVersion` releases exactly ONE version of the image — the
/// one its `Version` names, or the one its `Alias` is attached to — so that version stops
/// being readable while the image and every other version of it stay readable, and the
/// released number is never handed to a later version.
#[tokio::test]
async fn deleting_a_version_releases_only_the_selected_version() {
    let emulator = Emulator::new();

    // Rows differ along the axes the rule generalises over: WHICH selector names the
    // version (number or alias), WHICH of the image's versions it picks (first, middle,
    // last), how many versions the image holds, the image name's shape (mixed case,
    // digits, hyphen, dot, the 1- and 63-character modelled bounds), the signed region,
    // and whether the version is still `CREATING` or has been moved to `CREATED` by the
    // clock.
    let longest_name = format!("a{}", "b".repeat(62));
    let cases: Vec<(&str, i64, By, i64, &str, bool)> = vec![
        ("MixedCaseImage", 1, By::Version(1), 1, "eu-west-3", false),
        ("image-2024", 3, By::Version(2), 2, "ap-southeast-2", false),
        ("my.image.name", 3, By::Version(3), 3, "us-gov-west-1", true),
        ("a", 2, By::Alias("alias-of-1"), 1, "sa-east-1", false),
        (
            longest_name.as_str(),
            3,
            By::Alias("alias-of-3"),
            3,
            "eu-central-1",
            true,
        ),
        ("dotted.one", 4, By::Alias("alias-of-2"), 2, "us-west-2", false),
    ];

    for (index, (name, version_count, selector, released, region, settled)) in
        cases.into_iter().enumerate()
    {
        emulator.create_image_in(region, name).await;
        for version in 1..=version_count {
            // Every version carries its own alias, so an alias-selected row picks a
            // version by something other than its number.
            emulator
                .create_version_in(
                    region,
                    name,
                    &format!("token-{index}-{version}"),
                    &[&format!("alias-of-{version}")],
                )
                .await;
        }
        if settled {
            tokio::time::sleep(PAST_CREATED_DUE).await;
        }

        let (status, parsed) = emulator
            .call_in(region, "DeleteImageVersion", &selector.request(name))
            .await;
        assert_eq!(
            status, 200,
            "case {name}/{region} selecting {selector:?}: expected HTTP 200 from \
             DeleteImageVersion, got {status} with {parsed}"
        );
        assert_eq!(
            parsed,
            json!({}),
            "case {name}/{region} selecting {selector:?}: DeleteImageVersion models an \
             empty output, got {parsed}"
        );

        // The selected version is gone, by number and by its alias.
        let (status, parsed) = emulator.describe_version_in(region, name, released).await;
        assert_eq!(
            status, 400,
            "case {name}/{region}: DescribeImageVersion of the released version, \
             got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {name}/{region}: the released version is not found, got {parsed:?}"
        );
        assert_eq!(
            message(&parsed),
            format!(
                "ImageVersion with ARN {} does not exist",
                version_arn(region, name, released)
            ),
            "case {name}/{region}: the reader names the version it could not find by its \
             own ARN, got {parsed:?}"
        );
        let (status, parsed) = emulator
            .call_in(
                region,
                "DescribeImageVersion",
                &json!({ "ImageName": name, "Alias": format!("alias-of-{released}") }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {name}/{region}: the released version's alias resolves to nothing, \
             got {status} with {parsed}"
        );

        // Every OTHER version of the image survives, with the ARN it was minted with.
        for version in (1..=version_count).filter(|version| *version != released) {
            let (status, parsed) = emulator.describe_version_in(region, name, version).await;
            assert_eq!(
                status, 200,
                "case {name}/{region}/v{version}: a version the delete did not select \
                 stays readable, got {status} with {parsed}"
            );
            assert_eq!(
                str_field(&parsed, "ImageVersionArn"),
                version_arn(region, name, version),
                "case {name}/{region}/v{version}: the surviving version keeps its ARN, \
                 got {parsed:?}"
            );
        }

        // The image itself is untouched.
        let described = emulator
            .call_in(region, "DescribeImage", &json!({ "ImageName": name }))
            .await;
        assert_eq!(
            described.0, 200,
            "case {name}/{region}: the image survives the version delete, got {described:?}"
        );

        // A version number is part of a version's ARN, so the released number is never
        // reused: the next create continues past the highest number ever minted.
        let minted = emulator
            .create_version_in(region, name, &format!("token-{index}-next"), &[])
            .await;
        assert_eq!(
            minted,
            version_arn(region, name, version_count + 1),
            "case {name}/{region}: the next version numbers past the highest minted, \
             got {minted}"
        );
    }
}

/// RULE: `DeleteImageVersion` resolves its selection against the versions stored for the
/// request's `ImageName`, so a selection resolving to no stored version — an unknown
/// image, an unknown version number, an unknown alias, or a version a previous delete
/// already released — is refused with `ResourceNotFound` (HTTP 400) that names the
/// resource the request identified, and releases nothing.
#[tokio::test]
async fn a_selection_that_resolves_to_nothing_is_refused_as_resource_not_found() {
    let emulator = Emulator::new();

    // A neighbouring image with two aliased versions exists throughout, so each refusal
    // is about the NAMED selection rather than about an empty store.
    emulator.create_image_in("us-east-1", "existing").await;
    emulator
        .create_version_in("us-east-1", "existing", "token-1", &["stable"])
        .await;
    emulator
        .create_version_in("us-east-1", "existing", "token-2", &["candidate"])
        .await;
    // One version that WAS there and was released: deleting it again is a selection that
    // resolves to nothing.
    emulator.create_image_in("us-east-1", "released").await;
    emulator
        .create_version_in("us-east-1", "released", "token-3", &["gone-alias"])
        .await;
    emulator
        .ok(
            "DeleteImageVersion",
            &json!({ "ImageName": "released", "Version": 1 }),
        )
        .await;

    // Rows differ along the axes the rule generalises over: which part of the selection
    // misses (the image, the number, the alias), whether the miss is a never-created or
    // an already-released resource, a number at the modelled `range 0..` floor, a number
    // past the highest minted, an alias held by a version of ANOTHER image, a name
    // differing from a stored one only in case, and the modelled name-length bounds.
    let longest_unknown = format!("z{}", "y".repeat(62));
    let cases: Vec<(&str, &str, By, bool)> = vec![
        ("unknown image", "missing", By::Version(1), false),
        ("unknown image, by alias", "missing", By::Alias("stable"), false),
        ("image differing in case", "Existing", By::Version(1), false),
        ("shortest unknown image", "x", By::Version(1), false),
        (
            "longest unknown image",
            longest_unknown.as_str(),
            By::Version(1),
            false,
        ),
        ("version at the range floor", "existing", By::Version(0), true),
        ("version past the highest", "existing", By::Version(3), true),
        ("far version", "existing", By::Version(4_096), true),
        ("unknown alias", "existing", By::Alias("nope"), true),
        (
            "alias held by another image",
            "released",
            By::Alias("stable"),
            true,
        ),
        ("already released version", "released", By::Version(1), true),
        (
            "already released alias",
            "released",
            By::Alias("gone-alias"),
            true,
        ),
    ];

    for (case, name, selector, image_exists) in cases {
        let (status, parsed) = emulator
            .call("DeleteImageVersion", &selector.request(name))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400 for a selection resolving to nothing, \
             got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {case}: expected ResourceNotFound, got {parsed:?}"
        );
        let refusal = message(&parsed);
        let image_arn = format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:image/{name}");
        // The refusal names the resource the REQUEST identified, so what it names follows
        // from the selection rather than from the row's values: the image's ARN when no
        // image is stored under the name, the version's OWN ARN when the request named it
        // by number, and the image's ARN beside the alias when the alias determines no
        // version ARN at all.
        let expected = match (image_exists, selector) {
            (false, _) => format!("Image with ARN {image_arn} does not exist"),
            (true, By::Version(version)) => format!(
                "ImageVersion with ARN {} does not exist",
                version_arn("us-east-1", name, version)
            ),
            (true, By::Alias(alias)) => {
                format!("Could not find alias \"{alias}\" of image \"{image_arn}\".")
            }
        };
        assert_eq!(
            refusal, expected,
            "case {case}: the refusal names the resource the request identified, \
             got {refusal:?}"
        );

        // The refusal released nothing: both versions of the neighbour are readable.
        for version in 1..=2 {
            let (status, parsed) = emulator
                .describe_version_in("us-east-1", "existing", version)
                .await;
            assert_eq!(
                status, 200,
                "case {case}: the neighbour's version {version} survives the refusal, \
                 got {status} with {parsed}"
            );
        }
    }
}

/// RULE: `Version` and `Alias` are two ways to name ONE version and this operation
/// documents no default, so a request carrying BOTH or NEITHER selects no single version
/// and is refused with a 400 `ValidationException` that releases nothing.
#[tokio::test]
async fn a_request_naming_neither_or_both_selectors_is_refused_as_invalid() {
    let emulator = Emulator::new();

    emulator.create_image_in("us-east-1", "selectors").await;
    for version in 1..=2 {
        emulator
            .create_version_in(
                "us-east-1",
                "selectors",
                &format!("token-{version}"),
                &[&format!("alias-{version}")],
            )
            .await;
    }

    // Rows differ along the axis the rule generalises over: neither member present, and
    // both present in each combination of a resolvable and an unresolvable value — so no
    // row is refused merely because the value is unknown.
    let cases: Vec<(&str, Value)> = vec![
        ("neither", json!({ "ImageName": "selectors" })),
        (
            "both, both resolvable",
            json!({ "ImageName": "selectors", "Version": 1, "Alias": "alias-1" }),
        ),
        (
            "both, disagreeing",
            json!({ "ImageName": "selectors", "Version": 1, "Alias": "alias-2" }),
        ),
        (
            "both, alias unknown",
            json!({ "ImageName": "selectors", "Version": 2, "Alias": "nope" }),
        ),
        (
            "both, version unknown",
            json!({ "ImageName": "selectors", "Version": 9, "Alias": "alias-1" }),
        ),
    ];

    for (case, body) in cases {
        let (status, parsed) = emulator.call("DeleteImageVersion", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400 for a request selecting no single version, \
             got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: expected ValidationException, got {parsed:?}"
        );
        let refusal = message(&parsed);
        for member in ["Version", "Alias"] {
            assert!(
                refusal.contains(member),
                "case {case}: the refusal names the member {member} the caller must fix, \
                 got {refusal:?}"
            );
        }

        // Nothing was released: both versions are still readable.
        for version in 1..=2 {
            let (status, parsed) = emulator
                .describe_version_in("us-east-1", "selectors", version)
                .await;
            assert_eq!(
                status, 200,
                "case {case}: version {version} survives the refusal, \
                 got {status} with {parsed}"
            );
        }
    }
}

/// RULE: a version a stored Studio app's `ResourceSpec` names by its
/// `SageMakerImageVersionArn` is in use, so `DeleteImageVersion` for it is refused with
/// `ResourceInUse` (HTTP 400) and it stays readable; once no app names it, the same delete
/// is accepted.
#[tokio::test]
async fn a_version_a_studio_app_runs_on_is_in_use_and_refused() {
    let emulator = Emulator::new();

    emulator.create_image_in("us-east-1", "pinned").await;
    for version in 1..=2 {
        emulator
            .create_version_in(
                "us-east-1",
                "pinned",
                &format!("token-{version}"),
                &[&format!("alias-{version}")],
            )
            .await;
    }

    // One app pinned to version 1 of the image. An app on ANOTHER image's version exists
    // too, so the refusal below is about the version the request names rather than about
    // any app existing.
    let app = json!({
        "DomainId": "d-abc123",
        "UserProfileName": "studio-user",
        "AppType": "KernelGateway",
        "AppName": "app-on-v1",
    });
    let mut create_app = app
        .as_object()
        .expect("the identifying members form an object")
        .clone();
    create_app.insert(
        "ResourceSpec".to_string(),
        json!({ "SageMakerImageVersionArn": version_arn("us-east-1", "pinned", 1) }),
    );
    emulator.ok("CreateApp", &Value::Object(create_app)).await;

    emulator.create_image_in("us-east-1", "other").await;
    emulator
        .create_version_in("us-east-1", "other", "token-other", &["only"])
        .await;
    emulator
        .ok(
            "CreateApp",
            &json!({
                "DomainId": "d-abc123",
                "UserProfileName": "studio-user",
                "AppType": "JupyterLab",
                "AppName": "app-on-other",
                "ResourceSpec": {
                    "SageMakerImageVersionArn": version_arn("us-east-1", "other", 1),
                },
            }),
        )
        .await;

    // Rows differ along the axis the rule generalises over: WHICH selector names the
    // pinned version, since both resolve to the same version.
    for selector in [By::Version(1), By::Alias("alias-1")] {
        let (status, parsed) = emulator
            .call("DeleteImageVersion", &selector.request("pinned"))
            .await;
        assert_eq!(
            status, 400,
            "case {selector:?}: expected HTTP 400 while an app runs on the version, \
             got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceInUse",
            "case {selector:?}: expected ResourceInUse, got {parsed:?}"
        );

        let (status, parsed) = emulator.describe_version_in("us-east-1", "pinned", 1).await;
        assert_eq!(
            status, 200,
            "case {selector:?}: the refused version stays readable, \
             got {status} with {parsed}"
        );
    }

    // The version NO app names is released while the pinned one is still in use: the
    // check is per version, not per image.
    emulator
        .ok(
            "DeleteImageVersion",
            &json!({ "ImageName": "pinned", "Version": 2 }),
        )
        .await;
    let (status, parsed) = emulator.describe_version_in("us-east-1", "pinned", 2).await;
    assert_eq!(
        status, 400,
        "case unpinned version: the version no app names is released, \
         got {status} with {parsed}"
    );

    // With the app released, nothing runs on version 1 any more and the delete goes
    // through.
    emulator.ok("DeleteApp", &app).await;
    emulator
        .ok(
            "DeleteImageVersion",
            &json!({ "ImageName": "pinned", "Alias": "alias-1" }),
        )
        .await;
    let (status, parsed) = emulator.describe_version_in("us-east-1", "pinned", 1).await;
    assert_eq!(
        status, 400,
        "case released app: the version is gone once no app names it, \
         got {status} with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceNotFound",
        "case released app: expected ResourceNotFound, got {parsed:?}"
    );

    // The other image's version is untouched throughout.
    let (status, parsed) = emulator.describe_version_in("us-east-1", "other", 1).await;
    assert_eq!(
        status, 200,
        "case neighbour: the other image's version is untouched, \
         got {status} with {parsed}"
    );
}

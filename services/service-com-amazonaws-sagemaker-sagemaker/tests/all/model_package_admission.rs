// SPDX-License-Identifier: CC0-1.0

//! What `CreateModelPackage` REFUSES, driven end to end over the real wire path (an
//! awsJson1_1 request through the multi-service router into the mounted service), so what is
//! asserted is what a client sees.
//!
//! Two rules, each asserted over a TABLE of inputs rather than over one recorded call:
//!
//! 1. **a container image path has to be one of the two formats the member documents.**
//!    `ModelPackageContainerDefinition$Image` documents them as `registry/repository[:tag]`
//!    and `registry/repository[@digest]`, so a value that is no image reference at all, or
//!    one naming no registry, is refused 400 `ValidationException` wherever it sits — the
//!    inference specification's containers and the additional specifications' — and nothing
//!    is registered. What any registry HOLDS is not part of it.
//! 2. **a spent `ClientToken` registers nothing a second time.** `ClientToken` is an
//!    `@idempotencyToken` the recorded service SPENDS: a retry presenting a token this
//!    account has already registered a model package with is refused 400
//!    `ConflictException` naming that package — whether it is still held or has since been
//!    deleted — so a second registry version is never added under a token already used.
//!
//! The harness is the one `model_package_registry` declares, so these cases drive the same
//! router every other suite of this crate drives.

use serde_json::{json, Value};

use service_com_amazonaws_sagemaker_sagemaker::state;

use crate::model_package_registry::{container, error_code, expected_arn, str_field, Emulator};

/// A registry host every well-formed row's image sits behind: this account's own Amazon ECR
/// registry in the Region the harness signs for.
const REGISTRY: &str = "000000000000.dkr.ecr.us-east-1.amazonaws.com";

/// The error message an answer carries, whichever casing the envelope spells it with.
fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// An `InferenceSpecification` whose one container names `image`.
fn inference_specification(image: &str) -> Value {
    json!({
        "Containers": [container(image)],
        "SupportedContentTypes": ["application/json"],
        "SupportedResponseMIMETypes": ["application/json"],
    })
}

/// How this service words an image path outside the two documented formats: a FIXED
/// sentence (`state::INVALID_IMAGE_PATH`).
fn invalid_image_path() -> String {
    state::INVALID_IMAGE_PATH.to_string()
}

/// RULE: a container image path outside the two formats the member documents is refused 400
/// `ValidationException` and registers nothing, wherever the member sits in the request;
/// every path of those formats registers, whatever any registry holds.
///
/// The refused rows differ along WHY the path is outside them: whitespace inside it, a
/// leading or trailing space, a tab, the empty string, uppercase in the repository, a scheme,
/// an empty path component, and a repository with no registry in front of it (a bare name and
/// a tagged one). The accepted rows differ along the registry (an Amazon ECR host, another
/// registry's, one with a port), the repository's depth, and the tag-versus-digest form.
#[tokio::test]
async fn a_container_image_path_outside_the_documented_formats_is_refused() {
    let emulator = Emulator::new();

    // Source for the two formats: `smithy_query(query="shape",
    // shape="ModelPackageContainerDefinition")` on the `Image` member — "SageMaker supports
    // both registry/repository[:tag] and registry/repository[@digest] image path formats".
    let refused = [
        ("prose", "this is not an ecr image uri".to_string()),
        ("a registry path with a space", format!("{REGISTRY}/my image:1")),
        ("a leading space", format!(" {REGISTRY}/inference:1")),
        ("a trailing space", format!("{REGISTRY}/inference:1 ")),
        ("a tab", format!("{REGISTRY}/inference\t1")),
        ("the empty string", String::new()),
        ("an uppercase repository", format!("{REGISTRY}/Inference:1")),
        ("a scheme", format!("https://{REGISTRY}/inference:1")),
        ("an empty path component", format!("{REGISTRY}//inference:1")),
        ("a bare repository with no registry", "inference".to_string()),
        ("a tagged repository with no registry", "inference:1".to_string()),
        ("a first component that is no host", "library/inference:1".to_string()),
    ];

    for (index, (case, image)) in refused.iter().enumerate() {
        let name = format!("badimage{index}");
        let case = format!("{case} in the inference specification");
        let (status, answered) = emulator
            .call(
                "CreateModelPackage",
                &json!({
                    "ModelPackageName": name,
                    "InferenceSpecification": inference_specification(image),
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {answered}"
        );
        // A value the modelled `@pattern` already excludes (`^[\S]+$` — whitespace, and the
        // empty string) is refused by the request walk with the framework's own clause;
        // every other one is refused by this operation with the service's own fixed
        // sentence. Either way the caller is told the path is not a valid image.
        assert!(
            message(&answered) == invalid_image_path()
                || message(&answered).contains("validation error"),
            "case {case:?}: expected the image-path refusal, got {:?}",
            message(&answered)
        );
        let (status, read) = emulator
            .call("DescribeModelPackage", &json!({"ModelPackageName": name}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused registration must store nothing, got {status} with {read}"
        );
    }

    // The same rule, on the container of an ADDITIONAL inference specification: it is a
    // property of the image member, not of one member of one specification.
    let (status, answered) = emulator
        .call(
            "CreateModelPackage",
            &json!({
                "ModelPackageName": "badadditional",
                "InferenceSpecification": inference_specification(&format!("{REGISTRY}/a:1")),
                "AdditionalInferenceSpecifications": [{
                    "Name": "extra",
                    "Containers": [container("not-an-image-path")],
                }],
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&answered).as_str(), message(&answered).as_str()),
        (400, "ValidationException", invalid_image_path().as_str()),
        "case \"an additional specification's container\": expected the image-path refusal, \
         got {status} with {answered}"
    );

    // And every path of the documented formats registers: the rule is about the FORM, not
    // about what a registry holds.
    for (index, (case, image)) in [
        ("an Amazon ECR host and a tag", format!("{REGISTRY}/inference:1")),
        ("a digest instead of a tag", format!("{REGISTRY}/inference@sha256:abc")),
        ("another registry's host", "registry.example.com/team/inference:2".to_string()),
        ("a registry with a port", "registry.example.com:5000/inference".to_string()),
        (
            "a repository the registry does not hold",
            format!("{REGISTRY}/never-pushed:99"),
        ),
    ]
    .iter()
    .enumerate()
    {
        let name = format!("goodimage{index}");
        let arn = emulator
            .create(
                case,
                &json!({
                    "ModelPackageName": name,
                    "InferenceSpecification": inference_specification(image),
                }),
            )
            .await;
        assert_eq!(
            arn,
            expected_arn(state::DEFAULT_REGION, &name),
            "case {case:?}: expected the package to be registered, got {arn:?}"
        );
    }
}

/// RULE: a `ClientToken` this account has SPENT registers nothing a second time — a retry
/// presenting it is refused 400 `ConflictException` naming the model package that token
/// registered, whether that package is still held or has since been deleted, so a registry
/// version is never added under a token already used; a token reused for a DIFFERENT package
/// is a fresh call.
///
/// The rows differ along the kind of package the token registered (an unversioned package and
/// a version inside a group) and along what the retry does (repeat the same request, present
/// a different one, and present it once the registration is gone).
#[tokio::test]
async fn a_spent_client_token_registers_nothing_a_second_time() {
    let emulator = Emulator::new();
    emulator.create_group(state::DEFAULT_REGION, "spenttoken").await;

    // (case, the members that name the package, the resource its ARN carries)
    let rows = [
        (
            "an unversioned package",
            json!({"ModelPackageName": "spent-package"}),
            "spent-package".to_string(),
        ),
        (
            "a version in a group",
            json!({"ModelPackageGroupName": "spenttoken"}),
            "spenttoken/1".to_string(),
        ),
    ];

    for (case, members, resource) in rows {
        let token = format!("token-{}", resource.replace('/', "-"));
        // The clause the refusal names the registration's identity with: the GROUP a version
        // joined, or the NAME an unversioned package was registered under.
        let identity_clause = match resource.split_once('/') {
            Some((group, _)) => format!("Group:{group}"),
            None => format!("Name:{resource}"),
        };
        let mut body = members.clone();
        body["ClientToken"] = json!(token);
        body["InferenceSpecification"] = inference_specification(&format!("{REGISTRY}/a:1"));

        let first = emulator.create(case, &body).await;
        assert_eq!(
            first,
            expected_arn(state::DEFAULT_REGION, &resource),
            "case {case:?}: expected the ARN for {resource:?}, got {first:?}"
        );

        // Every retry presenting the spent token is refused and registers nothing: the same
        // request, a request that differs, and — once the registration is deleted — the
        // same request again.
        let differing = {
            let mut differing = body.clone();
            differing["ModelPackageDescription"] = json!("a second registration");
            differing
        };
        for (retry, request) in [
            ("the same request again", body.clone()),
            ("a request that differs", differing),
        ] {
            let (status, answered) = emulator.call("CreateModelPackage", &request).await;
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (400, "ConflictException"),
                "case {case:?} / {retry:?}: a spent token must be refused, got {status} with \
                 {answered}"
            );
            assert!(
                message(&answered).contains(&identity_clause),
                "case {case:?} / {retry:?}: the refusal must name the registration the token \
                 was spent on, got {:?}",
                message(&answered)
            );
        }
        // The first registration is untouched by the refused retries, and no second version
        // joined the group.
        let described = emulator.describe(case, &first).await;
        assert_eq!(
            str_field(&described, "ModelPackageArn"),
            first,
            "case {case:?}: the first registration must be untouched, got {described}"
        );

        // Once the registration is gone, the token is still spent.
        let (status, deleted) = emulator
            .call("DeleteModelPackage", &json!({ "ModelPackageName": resource }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the package must be deletable, got {status} with {deleted}"
        );
        let (status, answered) = emulator.call("CreateModelPackage", &body).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ConflictException"),
            "case {case:?}: a spent token must be refused after the deletion too, got \
             {status} with {answered}"
        );
        let (status, read) = emulator
            .call(
                "DescribeModelPackage",
                &json!({"ModelPackageName": resource}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the refused retry must register nothing, got {status} with {read}"
        );
    }

    // A FRESH token registers normally after all of that, so what the rule refuses is the
    // reuse and nothing else.
    let fresh = emulator
        .create(
            "a fresh token after a spent one",
            &json!({
                "ModelPackageGroupName": "spenttoken",
                "ClientToken": "token-fresh",
                "InferenceSpecification": inference_specification(&format!("{REGISTRY}/a:1")),
            }),
        )
        .await;
    assert_eq!(
        fresh,
        expected_arn(state::DEFAULT_REGION, "spenttoken/1"),
        "case \"a fresh token after a spent one\": expected a registration, got {fresh:?}"
    );
    let described = emulator.describe("a fresh token after a spent one", &fresh).await;
    assert_eq!(
        str_field(&described, "ModelPackageArn"),
        fresh,
        "case \"a fresh token after a spent one\": the registration must read back, got \
         {described}"
    );

    // And a token spent on one package is a fresh call for ANOTHER: the record is scoped to
    // the identity the retry asks for.
    let elsewhere = emulator
        .create(
            "a spent token on another package",
            &json!({
                "ModelPackageName": "token-elsewhere",
                "ClientToken": "token-spent-package",
                "InferenceSpecification": inference_specification(&format!("{REGISTRY}/a:1")),
            }),
        )
        .await;
    assert_eq!(
        elsewhere,
        expected_arn(state::DEFAULT_REGION, "token-elsewhere"),
        "case \"a spent token on another package\": expected a registration, got {elsewhere:?}"
    );
}

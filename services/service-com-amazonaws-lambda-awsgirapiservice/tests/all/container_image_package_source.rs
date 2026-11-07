// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a CONTAINER-IMAGE package: what a function may be created with,
//! and which image it ends up frozen to.
//!
//! The RULES asserted here, each over a table of inputs:
//!
//! 1. **the image is resolved against the registry.** "For a container image, the code
//!    property must include the URI of a container image in the Amazon ECR registry"
//!    (`CreateFunction`), and Lambda follows that reference in the function's own Region
//!    when the package is stored: a reference that is no image URI of that Region's
//!    registry is refused `InvalidParameterValueException` / `Source image <uri> is not
//!    valid. Provide a valid source image.`, one that is but names nothing pushed is
//!    refused `Source image <uri> does not exist. Provide a valid source image.`, and
//!    neither creates a function. `UpdateFunctionCode` follows a new reference the same
//!    way.
//! 2. **the resolved URI pins the registry's own digest.** `Code.ResolvedImageUri` is "The
//!    resolved URI for the image", so it is the repository pinned to the manifest digest
//!    ECR returned for the tag the function names — the value `PutImage` answered and
//!    `DescribeImages` reports — and `Code.ImageUri` is the URI as supplied.
//! 3. **a container image mounts no layers.** `Layers` is "A list of function layers to add
//!    to the function's execution environment", which a container image has none of, so a
//!    create naming any layer is refused `Lambda layers are not supported for functions
//!    created with container images.` and nothing is created.
//! 4. **a code update may only name the package form the function's type uses.** Supplying
//!    `ImageUri` for a `Zip` function is refused `Please don't provide ImageUri when
//!    updating a function with packageType Zip.`, and the symmetric mistakes on an image
//!    function are refused by the same sentence family.
//!
//! The rows differ along what those rules generalise over: how the reference addresses the
//! image (a tag, the default tag, a digest, a namespaced repository), which part of it is
//! wrong (the Region, the repository, the tag, the whole form), how many layers a create
//! names, and which member a code update supplies.
//!
//! ECR is mounted BESIDE Lambda on one engine, and every image this suite resolves is
//! pushed through ECR's own `CreateRepository`/`PutImage` — so the digest asserted here is
//! the one the registry minted rather than one this test computed. The Lambda-side harness
//! (the `Answer` wrapper, `json_call` and the activation window) is the one
//! `function_lifecycle.rs` publishes.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{
    create_function, json_call, make_activation_due, package_bytes, zip_file, Answer, REGION, ROLE,
};

/// The `X-Amz-Target` prefix Amazon ECR routes on (that crate's `OPERATIONS.md`, "Wire
/// protocol").
const ECR_TARGET_PREFIX: &str = "AmazonEC2ContainerRegistry_V20150921";

/// A SigV4 credential scope naming ECR in the SAME Region the Lambda harness signs for, so
/// a repository created here is a repository of the functions' own Region.
const ECR_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/ecr/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The account id every emulator ARN and registry URI carries.
const REGISTRY_ID: &str = "000000000000";

/// The error code every refusal in this suite carries.
const INVALID_PARAMETER: &str = "InvalidParameterValueException";

/// The `message` of a wire error, under either casing the JSON envelopes use.
fn message(answer: &Answer) -> Option<String> {
    answer.string("message").or_else(|| answer.string("Message"))
}

/// A manifest a push can store with no layer upload behind it: a schema-2 image manifest
/// that declares no filesystem layer and no configuration blob, so the repository is
/// missing none of the blobs it references.
fn manifest(marker: &str) -> String {
    json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
        "layers": [],
        "annotations": { "org.opencontainers.image.title": marker },
    })
    .to_string()
}

/// Lambda and ECR on one engine: the registry a function's image is resolved against is
/// ECR's own store, so it is written through ECR's own surface — the shape
/// `core/router/src/lib.rs` mounts them in.
fn emulator(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(engine)),
    );
    router.register_service(
        service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router
}

/// Call one ECR operation on the awsJson1_1 wire.
async fn ecr_call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("http://api.ecr.us-east-1.amazonaws.com/")
        .header("host", "api.ecr.us-east-1.amazonaws.com")
        .header("authorization", ECR_AUTHORIZATION)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{ECR_TARGET_PREFIX}.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The registry URI of the repository `name` in the Region the harness signs for.
fn repository_uri(name: &str) -> String {
    format!("{REGISTRY_ID}.dkr.ecr.{REGION}.amazonaws.com/{name}")
}

/// Create the repository `name` and push `manifest` to it under `tag`, answering the
/// manifest digest the REGISTRY minted for it.
async fn push_image(
    router: &MultiServiceRouter,
    name: &str,
    tag: &str,
    manifest: &str,
) -> String {
    let created = ecr_call(router, "CreateRepository", &json!({"repositoryName": name})).await;
    assert!(
        created.status == 200 || created.status == 400,
        "case {name:?}: expected CreateRepository to answer, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let pushed = ecr_call(
        router,
        "PutImage",
        &json!({
            "repositoryName": name,
            "imageTag": tag,
            "imageManifest": manifest,
        }),
    )
    .await;
    assert_eq!(
        pushed.status, 200,
        "case {name:?}: expected 200 from PutImage, got {} {}",
        pushed.status,
        String::from_utf8_lossy(&pushed.body)
    );
    pushed
        .json()
        .pointer("/image/imageId/imageDigest")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {name:?}: expected PutImage to report a manifest digest, got {}",
                String::from_utf8_lossy(&pushed.body)
            )
        })
        .to_string()
}

/// `GetFunction` on a function, with no qualifier.
async fn get_function(router: &MultiServiceRouter, name: &str) -> Answer {
    let uri = format!("/2015-03-31/functions/{name}");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// A `CreateFunction` request body for an image package.
fn image_create(name: &str, image_uri: &str) -> Value {
    json!({
        "FunctionName": name,
        "Role": ROLE,
        "PackageType": "Image",
        "Code": { "ImageUri": image_uri },
    })
}

/// One row of the resolution table: the reference a create names, and the sentence the
/// create is refused with — or `None` where it must be served.
struct SourceCase {
    /// What makes this row its own case.
    case: &'static str,
    /// The `ImageUri` the create carries, built from the repository the row needs.
    image_uri: String,
    /// The fixed part of the refusal AWS words, or `None` for a reference that resolves.
    refusal: Option<&'static str>,
}

/// Rule 1: a container-image create resolves its reference against the registry of the
/// function's own Region, and is refused where the reference names no image there.
#[tokio::test]
async fn an_image_create_resolves_its_reference_against_the_registry() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator(&engine);

    // One repository that really holds an image, under one tag, plus a namespaced one.
    push_image(&router, "held", "v1", &manifest("held-v1")).await;
    push_image(&router, "team/held", "latest", &manifest("team-latest")).await;
    // And a repository that exists but was never pushed to.
    let empty = ecr_call(
        &router,
        "CreateRepository",
        &json!({"repositoryName": "empty"}),
    )
    .await;
    assert_eq!(
        empty.status, 200,
        "expected 200 from CreateRepository, got {} {}",
        empty.status,
        String::from_utf8_lossy(&empty.body)
    );

    let rows = vec![
        SourceCase {
            case: "the pushed tag of a held repository",
            image_uri: format!("{}:v1", repository_uri("held")),
            refusal: None,
        },
        SourceCase {
            case: "a namespaced repository's pushed tag",
            image_uri: format!("{}:latest", repository_uri("team/held")),
            refusal: None,
        },
        SourceCase {
            case: "no tag at all, which names the default one",
            image_uri: repository_uri("team/held"),
            refusal: None,
        },
        SourceCase {
            case: "another Region's registry",
            image_uri: format!("{REGISTRY_ID}.dkr.ecr.eu-west-1.amazonaws.com/held:v1"),
            refusal: Some("is not valid"),
        },
        SourceCase {
            case: "no registry host at all",
            image_uri: "docker.io/library/alpine:3".to_string(),
            refusal: Some("is not valid"),
        },
        SourceCase {
            case: "a tag that was never pushed",
            image_uri: format!("{}:never", repository_uri("held")),
            refusal: Some("does not exist"),
        },
        SourceCase {
            case: "an existing repository nothing was pushed to",
            image_uri: format!("{}:v1", repository_uri("empty")),
            refusal: Some("does not exist"),
        },
        SourceCase {
            case: "a repository this registry does not hold",
            image_uri: format!("{}:v1", repository_uri("absent")),
            refusal: Some("does not exist"),
        },
        SourceCase {
            case: "a digest nothing was pushed under",
            image_uri: format!(
                "{}@sha256:{}",
                repository_uri("held"),
                "0".repeat(64)
            ),
            refusal: Some("does not exist"),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let name = format!("source-{index}");
        let answer = create_function(&router, &image_create(&name, &row.image_uri)).await;
        match row.refusal {
            None => {
                assert_eq!(
                    answer.status, 201,
                    "case {:?}: expected 201 from CreateFunction, got {} {}",
                    row.case,
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                make_activation_due(&engine);
                let read = get_function(&router, &name).await;
                assert_eq!(
                    read.json().pointer("/Code/ImageUri").and_then(Value::as_str),
                    Some(row.image_uri.as_str()),
                    "case {:?}: expected the ImageUri as supplied, got {}",
                    row.case,
                    String::from_utf8_lossy(&read.body)
                );
            }
            Some(fixed) => {
                assert_eq!(
                    answer.status, 400,
                    "case {:?}: expected 400 from CreateFunction, got {} {}",
                    row.case,
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                assert_eq!(
                    answer.error_code(),
                    INVALID_PARAMETER,
                    "case {:?}: expected {INVALID_PARAMETER}, got {}",
                    row.case,
                    answer.error_code()
                );
                let expected = format!(
                    "Source image {} {fixed}. Provide a valid source image.",
                    row.image_uri
                );
                assert_eq!(
                    message(&answer).as_deref(),
                    Some(expected.as_str()),
                    "case {:?}: expected {expected:?}, got {:?}",
                    row.case,
                    message(&answer)
                );
                // Nothing was created: the read of the function it would have been is the
                // modelled 404.
                let read = get_function(&router, &name).await;
                assert_eq!(
                    read.status, 404,
                    "case {:?}: expected the refused function to be absent, got {} {}",
                    row.case,
                    read.status,
                    String::from_utf8_lossy(&read.body)
                );
            }
        }
    }
}

/// Rule 2: `ResolvedImageUri` is the repository pinned to the digest the REGISTRY minted,
/// whichever way the reference addressed the image, and it is stable across reads.
#[tokio::test]
async fn the_resolved_uri_pins_the_digest_the_registry_minted() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator(&engine);

    // Two repositories, each with its own image, so a digest cannot be confused with the
    // repository it was pushed to; the second one is pushed under two tags.
    let one = push_image(&router, "pinned-one", "v1", &manifest("one")).await;
    let two = push_image(&router, "pinned-two", "v1", &manifest("two")).await;
    let also = push_image(&router, "pinned-two", "latest", &manifest("two")).await;
    assert_eq!(
        two, also,
        "expected one manifest to have one digest however many tags point at it, got \
         {two:?} and {also:?}"
    );
    assert_ne!(
        one, two,
        "expected two manifests to have two digests, got {one:?} twice"
    );

    // (case, the reference the function names, the repository the resolved URI pins, the
    // digest it pins it to)
    let rows: Vec<(&str, String, String, String)> = vec![
        (
            "a tag",
            format!("{}:v1", repository_uri("pinned-one")),
            repository_uri("pinned-one"),
            one.clone(),
        ),
        (
            "another repository's tag",
            format!("{}:v1", repository_uri("pinned-two")),
            repository_uri("pinned-two"),
            two.clone(),
        ),
        (
            "a second tag of one manifest",
            format!("{}:latest", repository_uri("pinned-two")),
            repository_uri("pinned-two"),
            two.clone(),
        ),
        (
            "the digest itself",
            format!("{}@{two}", repository_uri("pinned-two")),
            repository_uri("pinned-two"),
            two.clone(),
        ),
    ];

    for (index, (case, image_uri, repository, digest)) in rows.iter().enumerate() {
        let name = format!("resolved-{index}");
        let answer = create_function(&router, &image_create(&name, image_uri)).await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: expected 201 from CreateFunction, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        make_activation_due(&engine);
        let expected = format!("{repository}@{digest}");
        let expected_sha256 = digest
            .split_once(':')
            .map(|(_, hexadecimal)| hexadecimal.to_string());
        for read in ["first read", "second read"] {
            let body = get_function(&router, &name).await.json();
            let resolved = body
                .pointer("/Code/ResolvedImageUri")
                .and_then(Value::as_str)
                .map(str::to_string);
            assert_eq!(
                resolved.as_deref(),
                Some(expected.as_str()),
                "case {case:?} ({read}): expected {expected:?}, got {resolved:?}"
            );
            // The image IS the deployment package, so what `CodeSha256` reports is the
            // digest the registry minted for it.
            let reported = body
                .pointer("/Configuration/CodeSha256")
                .and_then(Value::as_str)
                .map(str::to_string);
            assert_eq!(
                reported, expected_sha256,
                "case {case:?} ({read}): expected CodeSha256 {expected_sha256:?}, got \
                 {reported:?}"
            );
        }
    }
}

/// Rule 3: a create that names any layer on a container-image function is refused, and
/// nothing is created. The rows differ in how many layers are named and in which other
/// members the request carries.
#[tokio::test]
async fn layers_are_refused_for_a_container_image_function() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator(&engine);
    let digest = push_image(&router, "layered", "v1", &manifest("layered")).await;
    assert!(
        digest.starts_with("sha256:"),
        "expected PutImage to mint a sha256 digest, got {digest:?}"
    );
    let image_uri = format!("{}:v1", repository_uri("layered"));

    let one_layer = format!("arn:aws:lambda:{REGION}:{REGISTRY_ID}:layer:shared:1");
    let another = format!("arn:aws:lambda:{REGION}:{REGISTRY_ID}:layer:other:2");
    // (case, the Layers member, whether the create is refused)
    let rows: Vec<(&str, Value, bool)> = vec![
        ("no Layers member at all", Value::Null, false),
        ("an empty Layers list", json!([]), false),
        ("one layer", json!([one_layer.clone()]), true),
        ("two layers", json!([one_layer, another]), true),
    ];

    for (index, (case, layers, refused)) in rows.iter().enumerate() {
        let name = format!("layered-{index}");
        let mut request = image_create(&name, &image_uri);
        if !layers.is_null() {
            request["Layers"] = layers.clone();
        }
        let answer = create_function(&router, &request).await;
        if !refused {
            assert_eq!(
                answer.status, 201,
                "case {case:?}: expected 201 from CreateFunction, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            continue;
        }
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 from CreateFunction, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {}",
            answer.error_code()
        );
        assert_eq!(
            message(&answer).as_deref(),
            Some("Lambda layers are not supported for functions created with container images."),
            "case {case:?}: expected the fixed layers sentence, got {:?}",
            message(&answer)
        );
        make_activation_due(&engine);
        let read = get_function(&router, &name).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: expected the refused function to be absent, got {} {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
    }
}

/// Rule 4: a code update may only name the package form the function's own `PackageType`
/// uses, and each mistake is refused by naming the member, the call and the package type.
#[tokio::test]
async fn a_code_update_may_only_name_its_package_type_s_form() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator(&engine);
    push_image(&router, "updatable", "v1", &manifest("updatable")).await;
    let image_uri = format!("{}:v1", repository_uri("updatable"));

    // A Zip function and an Image function, so both directions of the mismatch are driven.
    let zipped = create_function(
        &router,
        &json!({
            "FunctionName": "zipped",
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": zip_file(&package_bytes(32)) },
        }),
    )
    .await;
    assert_eq!(
        zipped.status, 201,
        "expected 201 from CreateFunction, got {} {}",
        zipped.status,
        String::from_utf8_lossy(&zipped.body)
    );
    let imaged = create_function(&router, &image_create("imaged", &image_uri)).await;
    assert_eq!(
        imaged.status, 201,
        "expected 201 from CreateFunction, got {} {}",
        imaged.status,
        String::from_utf8_lossy(&imaged.body)
    );
    make_activation_due(&engine);

    // (case, the function updated, the update body, the sentence it is refused with)
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "ImageUri on a Zip function",
            "zipped",
            json!({ "ImageUri": image_uri }),
            "Please don't provide ImageUri when updating a function with packageType Zip.",
        ),
        (
            "ZipFile on an Image function, which is short of its ImageUri",
            "imaged",
            json!({ "ZipFile": zip_file(&package_bytes(16)) }),
            "Please provide ImageUri when updating a function with packageType Image.",
        ),
        (
            "an S3 object on an Image function",
            "imaged",
            json!({ "S3Bucket": "packages", "S3Key": "fn.zip" }),
            "Please provide ImageUri when updating a function with packageType Image.",
        ),
        (
            "no package member at all on an Image function",
            "imaged",
            json!({}),
            "Please provide ImageUri when updating a function with packageType Image.",
        ),
        (
            "an ImageUri together with inline bytes",
            "imaged",
            json!({ "ImageUri": image_uri, "ZipFile": zip_file(&package_bytes(16)) }),
            "Please don't provide ZipFile when updating a function with packageType Image.",
        ),
        (
            "an ImageUri together with an S3 object",
            "imaged",
            json!({ "ImageUri": image_uri, "S3Bucket": "packages", "S3Key": "fn.zip" }),
            "Please don't provide S3Bucket when updating a function with packageType Image.",
        ),
    ];

    for (case, function, body, expected) in rows {
        let uri = format!("/2015-03-31/functions/{function}/code");
        let answer = json_call(&router, "PUT", &uri, &body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 from UpdateFunctionCode, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {}",
            answer.error_code()
        );
        assert_eq!(
            message(&answer).as_deref(),
            Some(expected),
            "case {case:?}: expected {expected:?}, got {:?}",
            message(&answer)
        );
    }

    // And the update that DOES name the right form is served, and re-pins the function to
    // the digest the registry holds for the new reference.
    let next = push_image(&router, "updatable", "v2", &manifest("updatable-v2")).await;
    let updated = json_call(
        &router,
        "PUT",
        "/2015-03-31/functions/imaged/code",
        &json!({ "ImageUri": format!("{}:v2", repository_uri("updatable")) }),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "expected 200 from UpdateFunctionCode, got {} {}",
        updated.status,
        String::from_utf8_lossy(&updated.body)
    );
    make_activation_due(&engine);
    let expected = format!("{}@{next}", repository_uri("updatable"));
    let resolved = get_function(&router, "imaged")
        .await
        .json()
        .pointer("/Code/ResolvedImageUri")
        .and_then(Value::as_str)
        .map(str::to_string);
    assert_eq!(
        resolved.as_deref(),
        Some(expected.as_str()),
        "case {:?}: expected {expected:?}, got {resolved:?}",
        "the new reference is resolved too"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what a response REPORTS about a stored component or image
//! recipe, beside the resource itself.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! * every answer about a VERSIONED resource of this service carries
//!   `latestVersionReferences`: the resource's own ARN with its version replaced by
//!   `x.x.x`, `<major>.x.x`, `<major>.<minor>.x` and its exact `<major>.<minor>.<patch>`,
//!   each a reference this service resolves and none of them pinned to a build number;
//! * a resource reports its own `tags` as a map on every read — an empty one when it
//!   carries no tags — rather than omitting the member;
//! * an image recipe reports ONE `platform` and its `components` list on every read and
//!   in every list entry: the platform its components target, or, for a recipe that
//!   includes none, the platform of the parent image it builds from, and an empty
//!   component list for a recipe created without components.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every case signs for; the ARNs it creates carry it.
const REGION: &str = "us-west-2";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A build component document: one `build` phase, so the derived type is `BUILD`.
const BUILD_DOCUMENT: &str =
    "name: HelloWorld\nschemaVersion: 1.0\nphases:\n  - name: build\n    steps:\n      - name: Greet\n        action: ExecuteBash\n        inputs:\n          commands:\n            - echo hello\n";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `imagebuilder` in [`REGION`] — the signal the router routes on
/// and the handlers read the Region from.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{REGION}/imagebuilder/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, parsed)
}

/// The body of a 200 answer, or a panic naming the case.
fn ok(case: &str, status: u16, body: Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    body
}

/// One member of a response, or a panic naming the case.
fn member<'body>(case: &str, body: &'body Value, name: &str) -> &'body Value {
    body.get(name)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} in {body}"))
}

/// The text one member of a response carries.
fn text(case: &str, body: &Value, name: &str) -> String {
    member(case, body, name)
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected {name} to be text in {body}"))
        .to_string()
}

/// A `CreateComponent` body with whatever `extra` names overlaid.
fn component_body(name: &str, version: &str, token: &str, extra: Value) -> Value {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("semanticVersion".to_string(), json!(version));
    body.insert("platform".to_string(), json!("Linux"));
    body.insert("clientToken".to_string(), json!(token));
    body.insert("data".to_string(), json!(BUILD_DOCUMENT));
    overlay(&mut body, extra);
    Value::Object(body)
}

/// A `CreateImageRecipe` body with whatever `extra` names overlaid.
fn recipe_body(name: &str, version: &str, token: &str, extra: Value) -> Value {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("semanticVersion".to_string(), json!(version));
    body.insert("parentImage".to_string(), json!("ami-0123456789abcdef0"));
    body.insert("clientToken".to_string(), json!(token));
    overlay(&mut body, extra);
    Value::Object(body)
}

/// Overlay `extra`'s members onto `body`, so a row can both add and replace one.
fn overlay(body: &mut Map<String, Value>, extra: Value) {
    if let Value::Object(extra) = extra {
        for (name, value) in extra {
            body.insert(name, value);
        }
    }
}

async fn create_component(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "PUT", "/CreateComponent", Some(body)).await
}

async fn get_component(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/GetComponent?componentBuildVersionArn={arn}"),
        None,
    )
    .await
}

async fn create_recipe(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "PUT", "/CreateImageRecipe", Some(body)).await
}

async fn get_recipe(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/GetImageRecipe?imageRecipeArn={arn}"),
        None,
    )
    .await
}

async fn list_recipes(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/ListImageRecipes", Some(body)).await
}

/// The ARN of a resource of `kind` named `normalized`, at the version nodes `version`.
fn resource_arn(kind: &str, normalized: &str, version: &str) -> String {
    format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:{kind}/{normalized}/{version}")
}

/// The four wildcard-version ARNs a resource of `kind` named `normalized` at `version` is
/// reached by: the version replaced by `x.x.x`, `<major>.x.x`, `<major>.<minor>.x` and the
/// exact version, and never a build number.
fn expected_references(kind: &str, normalized: &str, version: &str) -> Value {
    let nodes: Vec<&str> = version.split('.').collect();
    let (major, minor, patch) = (nodes[0], nodes[1], nodes[2]);
    json!({
        "latestVersionArn": resource_arn(kind, normalized, "x.x.x"),
        "latestMajorVersionArn": resource_arn(kind, normalized, &format!("{major}.x.x")),
        "latestMinorVersionArn": resource_arn(kind, normalized, &format!("{major}.{minor}.x")),
        "latestPatchVersionArn": resource_arn(kind, normalized, &format!("{major}.{minor}.{patch}")),
    })
}

/// The `tags` a response reports, as a map, or `None` when the member is absent.
fn reported_tags(case: &str, body: &Value) -> Option<BTreeMap<String, String>> {
    let tags = body.get("tags")?;
    Some(
        tags.as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected a tags object in {body}"))
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case:?}: expected text in {body}"))
                        .to_string(),
                )
            })
            .collect(),
    )
}

/// The versions the reference rows span. They differ along what the rule generalises
/// over: the size of each node, whether the three nodes differ, and the documented bound
/// — `semanticVersion` states "you can assign any positive integer value, including zero.
/// The upper limit is 2^30-1, or 1073741823, for each node".
const VERSIONS: &[(&str, &str)] = &[
    ("the lowest assignable version", "0.0.0"),
    ("three distinct single-digit nodes", "1.2.3"),
    ("a date pattern", "2026.11.1"),
    ("the documented upper limit on every node", "1073741823.1073741823.1073741823"),
];

/// Both creates and both reads report the four wildcard version ARNs the created resource
/// is reached by, for every version a create may assign.
#[tokio::test]
async fn create_and_read_report_the_latest_version_references() {
    let router = emulator();

    for (n, (case, version)) in VERSIONS.iter().enumerate() {
        // The component: created, then read back by its own build version ARN.
        let component_name = format!("References Component {n}");
        let normalized = format!("references-component-{n}");
        let created = create_component(
            &router,
            &component_body(&component_name, version, &format!("ctok-{n:04}"), json!({})),
        )
        .await;
        let created = ok(case, created.0, created.1);
        let expected = expected_references("component", &normalized, version);
        let actual = member(case, &created, "latestVersionReferences");
        assert_eq!(
            actual, &expected,
            "case {case:?}: expected the component create to report {expected}, got {actual}"
        );

        let arn = text(case, &created, "componentBuildVersionArn");
        let read = get_component(&router, &arn).await;
        let read = ok(case, read.0, read.1);
        let actual = member(case, &read, "latestVersionReferences");
        assert_eq!(
            actual, &expected,
            "case {case:?}: expected the component read to report {expected}, got {actual}"
        );

        // The recipe: the same four references, under its own resource kind.
        let recipe_name = format!("References Recipe {n}");
        let normalized = format!("references-recipe-{n}");
        let created = create_recipe(
            &router,
            &recipe_body(&recipe_name, version, &format!("rtok-{n:04}"), json!({})),
        )
        .await;
        let created = ok(case, created.0, created.1);
        let expected = expected_references("image-recipe", &normalized, version);
        let actual = member(case, &created, "latestVersionReferences");
        assert_eq!(
            actual, &expected,
            "case {case:?}: expected the recipe create to report {expected}, got {actual}"
        );

        let arn = text(case, &created, "imageRecipeArn");
        let read = get_recipe(&router, &arn).await;
        let read = ok(case, read.0, read.1);
        let actual = member(case, &read, "latestVersionReferences");
        assert_eq!(
            actual, &expected,
            "case {case:?}: expected the recipe read to report {expected}, got {actual}"
        );
    }
}

/// Each reported reference RESOLVES: a read through any of the four reaches the resource,
/// and for a name+version pair with several build versions it reaches the newest — which
/// is why none of them carries a build number.
#[tokio::test]
async fn the_reported_references_resolve_to_the_latest_build_version() {
    let router = emulator();
    let name = "Resolving Component";
    let normalized = "resolving-component";
    let version = "1.0.0";

    // Two build versions of ONE name+version pair: the second create differs in content,
    // so it is a new build version rather than the already-exists refusal.
    let first = create_component(
        &router,
        &component_body(name, version, "build-token-1", json!({})),
    )
    .await;
    let first = ok("the first build version", first.0, first.1);
    let second = create_component(
        &router,
        &component_body(
            name,
            version,
            "build-token-2",
            json!({"description": "the second build version"}),
        ),
    )
    .await;
    let second = ok("the second build version", second.0, second.1);

    let newest = text("the second build version", &second, "componentBuildVersionArn");
    assert_ne!(
        newest,
        text("the first build version", &first, "componentBuildVersionArn"),
        "case the second build version: expected a build version of its own, got {newest:?}"
    );

    // Both creates report the SAME references: they name the latest version, not a build.
    let expected = expected_references("component", normalized, version);
    for (case, created) in [
        ("the first build version", &first),
        ("the second build version", &second),
    ] {
        let actual = member(case, created, "latestVersionReferences");
        assert_eq!(
            actual, &expected,
            "case {case:?}: expected {expected}, got {actual}"
        );
    }

    let references = expected
        .as_object()
        .expect("the four references are an object")
        .clone();
    for (case, reference) in &references {
        let reference = reference.as_str().expect("a reference is text");
        let read = get_component(&router, reference).await;
        let read = ok(case, read.0, read.1);
        let component = member(case, &read, "component").clone();
        let arn = text(case, &component, "arn");
        assert_eq!(
            arn, newest,
            "case {case:?}: expected {reference} to resolve to the newest build version {newest:?}, got {arn:?}"
        );
        // The read's own references are those of the resolved resource, so following one
        // reference and then the next never drifts.
        let actual = member(case, &read, "latestVersionReferences");
        assert_eq!(
            actual, &expected,
            "case {case:?}: expected the read through {reference} to report {expected}, got {actual}"
        );
    }
}

/// A resource reports its own tags as a map on every read: the tags it carries, and an
/// empty map when it carries none.
#[tokio::test]
async fn a_read_reports_the_resource_tags_as_a_map() {
    let router = emulator();

    // The rows differ along what the rule generalises over: how many tags the create
    // carried, including none at all.
    let rows: Vec<(&str, Value, BTreeMap<String, String>)> = vec![
        ("a resource created with no tags", json!({}), BTreeMap::new()),
        (
            "a resource created with one tag",
            json!({"tags": {"Owner": "images"}}),
            [("Owner", "images")]
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        ),
        (
            "a resource created with several tags",
            json!({"tags": {"Owner": "images", "Stage": "prod", "cost center": "eng/platform"}}),
            [
                ("Owner", "images"),
                ("Stage", "prod"),
                ("cost center", "eng/platform"),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        ),
    ];

    for (n, (case, extra, expected)) in rows.into_iter().enumerate() {
        let created = create_component(
            &router,
            &component_body(
                &format!("Tagged Component {n}"),
                "1.0.0",
                &format!("tctok-{n:04}"),
                extra.clone(),
            ),
        )
        .await;
        let created = ok(case, created.0, created.1);
        let arn = text(case, &created, "componentBuildVersionArn");
        let read = get_component(&router, &arn).await;
        let read = ok(case, read.0, read.1);
        let component = member(case, &read, "component").clone();
        let actual = reported_tags(case, &component);
        assert_eq!(
            actual,
            Some(expected.clone()),
            "case {case:?}: expected the component to report the tags {expected:?}, got {actual:?}"
        );

        let created = create_recipe(
            &router,
            &recipe_body(
                &format!("Tagged Recipe {n}"),
                "1.0.0",
                &format!("trtok-{n:04}"),
                extra,
            ),
        )
        .await;
        let created = ok(case, created.0, created.1);
        let arn = text(case, &created, "imageRecipeArn");
        let read = get_recipe(&router, &arn).await;
        let read = ok(case, read.0, read.1);
        let recipe = member(case, &read, "imageRecipe").clone();
        let actual = reported_tags(case, &recipe);
        assert_eq!(
            actual,
            Some(expected.clone()),
            "case {case:?}: expected the recipe to report the tags {expected:?}, got {actual:?}"
        );
    }
}

/// Every recipe reports ONE platform and its component list, on the read and in the
/// listed summary: the platform its components target, or the platform of the parent image
/// it builds from, and an empty list when it includes no components.
#[tokio::test]
async fn a_recipe_reports_the_platform_of_what_it_builds() {
    let router = emulator();

    // A Linux component, so one row can name components instead of leaning on the parent
    // image.
    let component = create_component(
        &router,
        &component_body("Platform Component", "1.0.0", "pctok-0001", json!({})),
    )
    .await;
    let component = ok("the component the rows include", component.0, component.1);
    let component_arn = text(
        "the component the rows include",
        &component,
        "componentBuildVersionArn",
    );

    // The rows differ along what the rule generalises over: which of the documented
    // `parentImage` forms the recipe names ("AMI ID", an "Image Builder image Amazon
    // Resource Name (ARN)" or an "Amazon Web Services Systems Manager (SSM) Parameter
    // Store Parameter"), which of the three `Platform` values that image carries
    // (`smithy_query(query="shape", shape="Platform")`: Windows, Linux, macOS), and
    // whether the recipe includes components at all.
    let rows: Vec<(&str, &str, Value, &str, usize)> = vec![
        (
            "an AMI id and no components",
            "ami-parent-recipe",
            json!({"parentImage": "ami-0123456789abcdef0"}),
            "Linux",
            0,
        ),
        (
            "an SSM parameter naming a Linux image",
            "ssm-parent-recipe",
            json!({
                "parentImage": "ssm:/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64",
            }),
            "Linux",
            0,
        ),
        (
            "a managed Windows parent image",
            "windows-parent-recipe",
            json!({
                "parentImage": format!("arn:aws:imagebuilder:{REGION}:aws:image/windows-server-2022-english-full-base-x86/x.x.x"),
            }),
            "Windows",
            0,
        ),
        (
            "a managed macOS parent image",
            "macos-parent-recipe",
            json!({
                "parentImage": format!("arn:aws:imagebuilder:{REGION}:aws:image/macos-sonoma-14-arm64/x.x.x"),
            }),
            "macOS",
            0,
        ),
        (
            "components that target one platform",
            "component-platform-recipe",
            json!({
                "parentImage": "ami-0123456789abcdef0",
                "components": [{"componentArn": component_arn.clone()}],
            }),
            "Linux",
            1,
        ),
    ];

    for (n, (case, name, extra, platform, components)) in rows.into_iter().enumerate() {
        let created = create_recipe(
            &router,
            &recipe_body(name, "1.0.0", &format!("prtok-{n:04}"), extra),
        )
        .await;
        let created = ok(case, created.0, created.1);
        let arn = text(case, &created, "imageRecipeArn");

        let read = get_recipe(&router, &arn).await;
        let read = ok(case, read.0, read.1);
        let recipe = member(case, &read, "imageRecipe").clone();
        let actual = recipe.get("platform").cloned();
        assert_eq!(
            actual,
            Some(json!(platform)),
            "case {case:?}: expected the platform {platform:?}, got {actual:?}"
        );
        let listed = member(case, &recipe, "components")
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a components list in {recipe}"))
            .len();
        assert_eq!(
            listed, components,
            "case {case:?}: expected {components} components, got {listed} in {recipe}"
        );

        // The summary a listing carries reports the same platform as the detail view.
        let page = list_recipes(&router, &json!({})).await;
        let page = ok(case, page.0, page.1);
        let summaries = member(case, &page, "imageRecipeSummaryList").clone();
        let summary = summaries
            .as_array()
            .and_then(|summaries| {
                summaries
                    .iter()
                    .find(|summary| summary.get("arn") == Some(&json!(arn)))
            })
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected {arn} in {summaries}"));
        let actual = summary.get("platform").cloned();
        assert_eq!(
            actual,
            Some(json!(platform)),
            "case {case:?}: expected the summary to report the platform {platform:?}, got {actual:?}"
        );
    }
}

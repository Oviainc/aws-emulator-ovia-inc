// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a Greengrass V2 read reports about the state a WRITE
//! derived, rather than about the bytes the write carried.
//!
//! The rules under test:
//!
//! * a component version published from an `inlineRecipe` is VALIDATED by the time
//!   `CreateComponentVersion` answers, so the create reports the state at the
//!   instant it accepted the request (`REQUESTED`) and every call that follows —
//!   `DescribeComponent`, and the `ConflictException` a republish is refused with —
//!   reports the validated version's terminal `DEPLOYABLE`;
//! * the service stores the recipe document it DERIVED from the submitted one —
//!   every submitted member, in the submitted order, plus the `ComponentType` the
//!   service assigns, which the recipe reference places immediately after
//!   `ComponentVersion` — and serializes that document for every reader, so a
//!   recipe read back as JSON is COMPACT (no space after `:` or `,`) and carries
//!   the assigned type, whatever spacing, member order or `ComponentType` the
//!   submitted bytes had;
//! * the same stored document carries the recipe members whose default is the
//!   EMPTY collection — the recipe's `Lifecycle`, and each manifest's `Lifecycle`
//!   and `Artifacts` — whether or not the submitted document declared them, so a
//!   client reading one always finds a collection; and
//! * `GetDeployment` reports the deployment's `components` map and its `tags` map,
//!   each EMPTY rather than absent when the deployment holds nothing in it.
//!
//! Every assertion drives the real `restJson1` front door: the request is built the
//! way an SDK builds it, dispatched through [`MultiServiceRouter`], and judged on
//! the HTTP status and the JSON body. Nothing reaches into the state struct, so a
//! handler that answered from the request instead of from what it persisted fails
//! here.
//!
//! The recipe rows differ along what the rule generalises over — the submitted
//! wire format, the spacing of the submitted bytes, the ORDER of the submitted
//! members, whether a `ComponentType` was submitted, which empty members the
//! document carries, and the kind of every scalar in it — not along the component
//! names, which the rule says nothing about.

use std::sync::Arc;

use aws_smithy_types::base64;
use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_greengrassv2_greengrassv2::state::COMPONENT_TYPE_GENERIC;
use service_com_amazonaws_greengrassv2_greengrassv2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for; every minted ARN carries it.
const REGION: &str = "us-west-2";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The emulator front door with only this service mounted, over ONE shared engine —
/// exactly how `core/router` assembles it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one signed request and return its status and parsed JSON body.
async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Vec<u8>) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://greengrass.{REGION}.amazonaws.com{path}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/greengrass/aws4_request, \
                 SignedHeaders=host, Signature=00"
            ),
        )
        .header("content-type", "application/json")
        .body(Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or(Value::String(String::from_utf8_lossy(&raw).into_owned()))
    };
    (status, value)
}

async fn post(router: &MultiServiceRouter, path: &str, body: &Value) -> (u16, Value) {
    let bytes = serde_json::to_vec(body).expect("a serializable request body");
    call(router, "POST", path, bytes).await
}

async fn get(router: &MultiServiceRouter, path: &str) -> (u16, Value) {
    call(router, "GET", path, Vec::new()).await
}

/// A string member of a JSON object, or the empty string when absent.
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// An ARN as a client percent-encodes a non-greedy `@httpLabel`.
fn encoded_label(arn: &str) -> String {
    arn.replace(':', "%3A").replace('/', "%2F")
}

/// The component-version ARN this emulator mints for `name`@`version`.
fn version_arn(name: &str, version: &str) -> String {
    format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:components:{name}:versions:{version}")
}

/// The deployment ARN of `deployment_id`.
fn deployment_arn(deployment_id: &str) -> String {
    format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:deployments:{deployment_id}")
}

/// An IoT thing ARN, which is what a deployment targets.
fn thing_target(name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT}:thing/{name}")
}

/// `CreateComponentVersion` with `document` as the inline recipe.
async fn publish(router: &MultiServiceRouter, document: &str) -> (u16, Value) {
    post(
        router,
        "/greengrass/v2/createComponentVersion",
        &json!({ "inlineRecipe": base64::encode(document.as_bytes()) }),
    )
    .await
}

/// `GetComponent` on `arn` in `format`, answering the decoded recipe document.
async fn stored_recipe(router: &MultiServiceRouter, arn: &str, format: &str) -> (u16, String) {
    let (status, body) = get(
        router,
        &format!(
            "/greengrass/v2/components/{}?recipeOutputFormat={format}",
            encoded_label(arn)
        ),
    )
    .await;
    let blob = base64::decode(text(&body, "recipe")).unwrap_or_default();
    (
        status,
        String::from_utf8(blob).unwrap_or_else(|_| format!("<not UTF-8> {body}")),
    )
}

/// The member NAMES of a recipe document, in the order it declares them.
fn member_names(document: &str) -> Vec<String> {
    yaml_rust2::YamlLoader::load_from_str(document)
        .unwrap_or_default()
        .first()
        .and_then(yaml_rust2::Yaml::as_hash)
        .map(|map| {
            map.iter()
                .map(|(key, _)| key.as_str().unwrap_or("<not a string>").to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A scalar member of a recipe document, or `None` when it declares none.
fn member(document: &str, key: &str) -> Option<String> {
    yaml_rust2::YamlLoader::load_from_str(document)
        .ok()?
        .first()?
        .as_hash()?
        .iter()
        .find(|(member, _)| member.as_str() == Some(key))
        .and_then(|(_, value)| value.as_str())
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// The recipe table
// ---------------------------------------------------------------------------

/// One row of the recipe table: a submitted recipe document and the JSON document
/// the service must store for it.
struct RecipeCase {
    label: &'static str,
    /// The bytes submitted as `inlineRecipe`.
    submitted: &'static str,
    /// The `RecipeOutputFormat` the submitted bytes are written in.
    format: &'static str,
    name: &'static str,
    version: &'static str,
    /// The document `GetComponent` must answer with for `recipeOutputFormat=JSON`,
    /// byte for byte.
    stored_json: &'static str,
}

/// Recipe member names and nesting below come from the IoT Greengrass component
/// recipe reference (`RecipeFormatVersion`, `ComponentName`, `ComponentVersion`,
/// `ComponentDescription`, `ComponentPublisher`, `ComponentConfiguration`,
/// `Manifests[].Platform`, `Manifests[].Artifacts`, `Lifecycle`), and
/// `ComponentType` is the member the service assigns.
const RECIPES: &[RecipeCase] = &[
    RecipeCase {
        label: "json spaced the way a JSON encoder writes it, with empty members",
        submitted: r#"{"RecipeFormatVersion": "2020-01-25", "ComponentName": "com.example.Spaced", "ComponentVersion": "1.0.0", "ComponentDescription": "A probe component", "ComponentPublisher": "probe", "ComponentConfiguration": {"DefaultConfiguration": {}}, "Manifests": [{"Platform": {"os": "all"}, "Lifecycle": {}, "Artifacts": []}], "Lifecycle": {}}"#,
        format: "JSON",
        name: "com.example.Spaced",
        version: "1.0.0",
        stored_json: r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.Spaced","ComponentVersion":"1.0.0","ComponentType":"aws.greengrass.generic","ComponentDescription":"A probe component","ComponentPublisher":"probe","ComponentConfiguration":{"DefaultConfiguration":{}},"Manifests":[{"Platform":{"os":"all"},"Lifecycle":{},"Artifacts":[]}],"Lifecycle":{}}"#,
    },
    RecipeCase {
        label: "the same document already compact: only the assigned type is added",
        submitted: r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.Compact","ComponentVersion":"1.0.0","ComponentConfiguration":{"DefaultConfiguration":{}},"Manifests":[{"Platform":{"os":"all"},"Lifecycle":{},"Artifacts":[]}],"Lifecycle":{}}"#,
        format: "JSON",
        name: "com.example.Compact",
        version: "1.0.0",
        stored_json: r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.Compact","ComponentVersion":"1.0.0","ComponentType":"aws.greengrass.generic","ComponentConfiguration":{"DefaultConfiguration":{}},"Manifests":[{"Platform":{"os":"all"},"Lifecycle":{},"Artifacts":[]}],"Lifecycle":{}}"#,
    },
    RecipeCase {
        label: "members submitted in another order keep it, and the type follows the version",
        submitted: r#"{"ComponentName":"com.example.Ordered","ComponentDescription":"Out of order","ComponentVersion":"2.0.0","RecipeFormatVersion":"2020-01-25"}"#,
        format: "JSON",
        name: "com.example.Ordered",
        version: "2.0.0",
        stored_json: r#"{"ComponentName":"com.example.Ordered","ComponentDescription":"Out of order","ComponentVersion":"2.0.0","ComponentType":"aws.greengrass.generic","RecipeFormatVersion":"2020-01-25","Lifecycle":{}}"#,
    },
    RecipeCase {
        label: "a submitted ComponentType does not decide the stored one",
        submitted: r#"{"RecipeFormatVersion":"2020-01-25","ComponentType":"aws.greengrass.lambda","ComponentName":"com.example.Typed","ComponentVersion":"3.0.0"}"#,
        format: "JSON",
        name: "com.example.Typed",
        version: "3.0.0",
        stored_json: r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.Typed","ComponentVersion":"3.0.0","ComponentType":"aws.greengrass.generic","Lifecycle":{}}"#,
    },
    RecipeCase {
        label: "scalars of every JSON kind, including a string that must be escaped",
        submitted: r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.Scalars","ComponentVersion":"4.5.6","ComponentConfiguration":{"DefaultConfiguration":{"Count":3,"Ratio":1.5,"Enabled":true,"Absent":null,"Quoted":"say \"hi\"\nand \\ bye"}},"Manifests":[]}"#,
        format: "JSON",
        name: "com.example.Scalars",
        version: "4.5.6",
        stored_json: r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.Scalars","ComponentVersion":"4.5.6","ComponentType":"aws.greengrass.generic","ComponentConfiguration":{"DefaultConfiguration":{"Count":3,"Ratio":1.5,"Enabled":true,"Absent":null,"Quoted":"say \"hi\"\nand \\ bye"}},"Manifests":[],"Lifecycle":{}}"#,
    },
    RecipeCase {
        label: "a manifest with nothing but a platform: the empty defaults are supplied",
        submitted: r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.Bare","ComponentVersion":"7.0.0","Manifests":[{"Platform":{"os":"all"}},{"Platform":{"os":"linux"},"Artifacts":[{"Uri":"s3://bucket/key"}]}]}"#,
        format: "JSON",
        name: "com.example.Bare",
        version: "7.0.0",
        stored_json: r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.Bare","ComponentVersion":"7.0.0","ComponentType":"aws.greengrass.generic","Manifests":[{"Platform":{"os":"all"},"Lifecycle":{},"Artifacts":[]},{"Platform":{"os":"linux"},"Artifacts":[{"Uri":"s3://bucket/key"}],"Lifecycle":{}}],"Lifecycle":{}}"#,
    },
    RecipeCase {
        label: "a yaml recipe, whose stored document carries the assigned type too",
        submitted: "RecipeFormatVersion: '2020-01-25'\nComponentName: com.example.Yaml\nComponentVersion: 5.0.0\nComponentPublisher: probe\nManifests:\n  - Platform:\n      os: linux\n    Lifecycle: {}\n    Artifacts: []\nLifecycle: {}\n",
        format: "YAML",
        name: "com.example.Yaml",
        version: "5.0.0",
        stored_json: r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.Yaml","ComponentVersion":"5.0.0","ComponentType":"aws.greengrass.generic","ComponentPublisher":"probe","Manifests":[{"Platform":{"os":"linux"},"Lifecycle":{},"Artifacts":[]}],"Lifecycle":{}}"#,
    },
    RecipeCase {
        label: "a yaml recipe that declares its own type, which the service replaces",
        submitted: "RecipeFormatVersion: '2020-01-25'\nComponentName: com.example.YamlTyped\nComponentType: aws.greengrass.nucleus\nComponentVersion: 6.0.0\n",
        format: "YAML",
        name: "com.example.YamlTyped",
        version: "6.0.0",
        stored_json: r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.YamlTyped","ComponentVersion":"6.0.0","ComponentType":"aws.greengrass.generic","Lifecycle":{}}"#,
    },
];

#[tokio::test]
async fn a_stored_recipe_is_the_document_the_service_derived_from_the_submitted_one() {
    let router = emulator();

    for case in RECIPES {
        let (status, body) = publish(&router, case.submitted).await;
        assert_eq!(
            status, 201,
            "case {:?}: CreateComponentVersion must answer 201, got {status} {body}",
            case.label
        );

        let arn = version_arn(case.name, case.version);

        // Read as JSON: the stored document, serialized compactly, member for
        // member and byte for byte.
        let (status, document) = stored_recipe(&router, &arn, "JSON").await;
        assert_eq!(
            status, 200,
            "case {:?}: GetComponent must answer 200, got {status} {document}",
            case.label
        );
        assert_eq!(
            document, case.stored_json,
            "case {:?}: expected the derived document {:?}, got {document:?}",
            case.label, case.stored_json
        );

        // Read in the format the recipe was submitted in: the same document, with
        // the same members in the same order, and the same assigned type.
        let (status, native) = stored_recipe(&router, &arn, case.format).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetComponent as {} must answer 200, got {status} {native}",
            case.label, case.format
        );
        assert_eq!(
            member_names(&native),
            member_names(case.stored_json),
            "case {:?}: the stored {} document must declare the same members in the \
             same order, got {native:?}",
            case.label,
            case.format
        );
        assert_eq!(
            member(&native, "ComponentType").as_deref(),
            Some(COMPONENT_TYPE_GENERIC),
            "case {:?}: the stored {} document must carry the assigned type, got {native:?}",
            case.label,
            case.format
        );
    }
}

#[tokio::test]
async fn a_published_version_is_validated_by_the_time_the_create_answers() {
    let router = emulator();

    // One row per submitted wire format, plus a version form the state machine does
    // not depend on either.
    let json_case = RECIPES
        .iter()
        .find(|case| case.format == "JSON")
        .expect("a JSON row");
    let yaml_case = RECIPES
        .iter()
        .find(|case| case.format == "YAML")
        .expect("a YAML row");
    for (label, document, name, version) in [
        (
            "json",
            json_case.submitted.to_string(),
            json_case.name,
            json_case.version,
        ),
        (
            "yaml",
            yaml_case.submitted.to_string(),
            yaml_case.name,
            yaml_case.version,
        ),
        (
            "a prerelease version",
            r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.Prerelease","ComponentVersion":"0.1.0-alpha.1"}"#.to_string(),
            "com.example.Prerelease",
            "0.1.0-alpha.1",
        ),
    ] {
        let (status, created) = publish(&router, &document).await;
        assert_eq!(
            status, 201,
            "case {label:?}: CreateComponentVersion must answer 201, got {status} {created}"
        );
        // The create answers the state at the instant it ACCEPTED the request,
        // which is before it validated the recipe.
        assert_eq!(
            text(&created["status"], "componentState"),
            "REQUESTED",
            "case {label:?}: the create must report the state at acceptance, got {created}"
        );

        // By the time it answered, the recipe was validated — so every read that
        // follows reports the terminal state, with no wait in between, and a
        // second read reports the same thing.
        let arn = version_arn(name, version);
        for read in ["the first read", "the same read again"] {
            let (status, described) = get(
                &router,
                &format!(
                    "/greengrass/v2/components/{}/metadata",
                    encoded_label(&arn)
                ),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}, {read}: DescribeComponent must answer 200, \
                 got {status} {described}"
            );
            assert_eq!(
                text(&described["status"], "componentState"),
                "DEPLOYABLE",
                "case {label:?}, {read}: a validated version must be DEPLOYABLE, \
                 got {described}"
            );
        }

        // The refusal a republish is answered with carries that same stored state.
        let (status, conflict) = publish(&router, &document).await;
        assert_eq!(
            status, 409,
            "case {label:?}: republishing must answer 409, got {status} {conflict}"
        );
        assert_eq!(
            text(&conflict, "message"),
            format!(
                "Component [{name} : {version}] for account [{ACCOUNT}] already exists \
                 with state: [DEPLOYABLE]"
            ),
            "case {label:?}: the conflict must name the stored version's state, got {conflict}"
        );
    }
}

// ---------------------------------------------------------------------------
// Deployment reads
// ---------------------------------------------------------------------------

/// One row of the deployment table: the `components` and `tags` members the create
/// carries, and what `GetDeployment` must report for each.
struct DeploymentCase {
    label: &'static str,
    /// The `components` member of the create request, or `None` when it names none.
    components: Option<fn() -> Value>,
    /// The `tags` member of the create request, or `None` when it carries none.
    tags: Option<fn() -> Value>,
}

/// The rows differ in whether the deployment specifies components at all, how many
/// it specifies and how fully, and whether it was created with tags — the two map
/// members `GetDeployment` reports.
fn deployment_cases() -> Vec<DeploymentCase> {
    vec![
        DeploymentCase {
            label: "no components member and one tag",
            components: None,
            tags: Some(|| json!({ "owner": "platform-team" })),
        },
        DeploymentCase {
            label: "an explicitly empty components map and no tags",
            components: Some(|| json!({})),
            tags: None,
        },
        DeploymentCase {
            label: "one component and several tags",
            components: Some(|| json!({ "com.example.One": { "componentVersion": "1.0.0" } })),
            tags: Some(|| json!({ "owner": "platform-team", "tier": "test" })),
        },
        DeploymentCase {
            label: "two components, one of them configured",
            components: Some(|| {
                json!({
                    "com.example.One": { "componentVersion": "1.0.0" },
                    "com.example.Two": {
                        "componentVersion": "2.0.0",
                        "configurationUpdate": { "merge": "{\"Message\":\"hi\"}" }
                    }
                })
            }),
            tags: None,
        },
    ]
}

#[tokio::test]
async fn get_deployment_reports_the_components_and_tags_maps_it_stored() {
    let router = emulator();

    for (row, case) in deployment_cases().into_iter().enumerate() {
        let mut request = Map::new();
        request.insert(
            "targetArn".to_string(),
            Value::String(thing_target(&format!("ReadCore{row}"))),
        );
        if let Some(components) = case.components {
            request.insert("components".to_string(), components());
        }
        if let Some(tags) = case.tags {
            request.insert("tags".to_string(), tags());
        }
        let request = Value::Object(request);

        let (status, created) = post(&router, "/greengrass/v2/deployments", &request).await;
        assert_eq!(
            status, 201,
            "case {:?}: CreateDeployment must answer 201, got {status} {created}",
            case.label
        );
        let id = text(&created, "deploymentId");

        // `components` and `tags` are reported as maps whether or not the
        // deployment holds anything in them: the empty map, never an absent member.
        let expected_components = case.components.map_or_else(|| json!({}), |make| make());
        let expected_tags = case.tags.map_or_else(|| json!({}), |make| make());

        let (status, stored) = get(&router, &format!("/greengrass/v2/deployments/{id}")).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetDeployment must answer 200, got {status} {stored}",
            case.label
        );
        assert_eq!(
            stored.get("components"),
            Some(&expected_components),
            "case {:?}: GetDeployment must report the stored components map, got {stored}",
            case.label
        );
        assert_eq!(
            stored.get("tags"),
            Some(&expected_tags),
            "case {:?}: GetDeployment must report the stored tags map, got {stored}",
            case.label
        );

        // The tagging route reads the same stored tags through the deployment's ARN.
        let (status, listed) = get(
            &router,
            &format!("/tags/{}", encoded_label(&deployment_arn(&id))),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: ListTagsForResource must answer 200, got {status} {listed}",
            case.label
        );
        assert_eq!(
            listed.get("tags"),
            Some(&expected_tags),
            "case {:?}: the tag listing must report the create's tags, got {listed}",
            case.label
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Greengrass V2 component-publish-then-deploy loop.
//!
//! Every assertion drives the real `restJson1` front door: the request is built the
//! way an SDK builds it, dispatched through [`MultiServiceRouter`], and judged on the
//! HTTP status and the JSON body. Nothing reaches into the state struct, so a handler
//! that answered from a canned body instead of from what it persisted fails here.

use std::sync::Arc;

use aws_smithy_types::base64;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_greengrassv2_greengrassv2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for; every minted ARN must carry it.
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
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> (u16, Value) {
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
    let body = response.into_body();
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::String(
            String::from_utf8_lossy(&body).into_owned(),
        ))
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

async fn delete(router: &MultiServiceRouter, path: &str) -> (u16, Value) {
    call(router, "DELETE", path, Vec::new()).await
}

/// `CreateComponentVersion` with `recipe` as the inline recipe, merged with any
/// additional top-level members (`tags`, `clientToken`, …).
async fn create_component(
    router: &MultiServiceRouter,
    recipe: &str,
    extra: &Value,
) -> (u16, Value) {
    let mut body = json!({ "inlineRecipe": base64::encode(recipe.as_bytes()) });
    if let (Some(target), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    post(router, "/greengrass/v2/createComponentVersion", &body).await
}

/// The wire error code of a `restJson1` error body.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// A string member of a JSON object, or the empty string when absent.
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The component-version ARN this emulator mints for `name`@`version`.
fn version_arn(name: &str, version: &str) -> String {
    format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:components:{name}:versions:{version}")
}

/// Percent-encode the colons in an ARN, the way an AWS SDK encodes an `@httpLabel`.
fn encoded_label(arn: &str) -> String {
    arn.replace(':', "%3A")
}

/// A recipe document's members, as the mapping a reader can compare.
fn recipe_members(document: &str) -> Vec<(String, String)> {
    let loaded = yaml_rust2::YamlLoader::load_from_str(document)
        .expect("a recipe document the YAML parser reads");
    loaded
        .first()
        .and_then(yaml_rust2::Yaml::as_hash)
        .expect("a recipe mapping")
        .iter()
        .map(|(key, value)| {
            let mut rendered = String::new();
            yaml_rust2::YamlEmitter::new(&mut rendered)
                .dump(value)
                .expect("an emittable recipe member");
            (
                key.as_str().expect("a string recipe key").to_string(),
                rendered,
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The recipe table
// ---------------------------------------------------------------------------

/// One row of the recipe table: a recipe document and everything the control plane
/// must derive from it. The rows differ along what the parse generalises over — wire
/// format (JSON / YAML), name, version form (release / prerelease / two-digit major),
/// which optional metadata keys are present, how many platform manifests there are,
/// and whether unrelated recipe sections are present.
struct RecipeCase {
    label: &'static str,
    recipe: &'static str,
    /// The `RecipeOutputFormat` the submitted bytes are written in.
    format: &'static str,
    /// The document `GetComponent` must answer with for `recipeOutputFormat=JSON`:
    /// the recipe the service DERIVED from `recipe`, byte for byte.
    stored_json: &'static str,
    name: &'static str,
    version: &'static str,
    description: Option<&'static str>,
    publisher: Option<&'static str>,
    /// The `name` of each platform the recipe's manifests declare, in order.
    platform_names: &'static [Option<&'static str>],
    tags: &'static [(&'static str, &'static str)],
}

/// Keys and nesting below come from the IoT Greengrass component recipe reference
/// (`RecipeFormatVersion`, `ComponentName`, `ComponentVersion`,
/// `ComponentDescription`, `ComponentPublisher`, `Manifests[].Platform`).
const RECIPES: &[RecipeCase] = &[
    RecipeCase {
        label: "json, only the required keys",
        recipe: r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.Minimal","ComponentVersion":"1.0.0"}"#,
        format: "JSON",
        stored_json: r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.Minimal","ComponentVersion":"1.0.0","ComponentType":"aws.greengrass.generic","Lifecycle":{}}"#,
        name: "com.example.Minimal",
        version: "1.0.0",
        description: None,
        publisher: None,
        platform_names: &[],
        tags: &[],
    },
    RecipeCase {
        label: "json, description, publisher and two platform manifests",
        recipe: r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.Full","ComponentVersion":"3.1.4","ComponentDescription":"A fully specified component.","ComponentPublisher":"Example Corp","Manifests":[{"Platform":{"os":"linux","architecture":"aarch64"},"Lifecycle":{"Run":"echo one"}},{"Platform":{"name":"windows-x86","os":"windows"},"Lifecycle":{"Run":"echo two"}}]}"#,
        format: "JSON",
        stored_json: r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.Full","ComponentVersion":"3.1.4","ComponentType":"aws.greengrass.generic","ComponentDescription":"A fully specified component.","ComponentPublisher":"Example Corp","Manifests":[{"Platform":{"os":"linux","architecture":"aarch64"},"Lifecycle":{"Run":"echo one"},"Artifacts":[]},{"Platform":{"name":"windows-x86","os":"windows"},"Lifecycle":{"Run":"echo two"},"Artifacts":[]}],"Lifecycle":{}}"#,
        name: "com.example.Full",
        version: "3.1.4",
        description: Some("A fully specified component."),
        publisher: Some("Example Corp"),
        platform_names: &[None, Some("windows-x86")],
        tags: &[("owner", "platform-team"), ("tier", "test")],
    },
    RecipeCase {
        label: "yaml, only the required keys",
        recipe: "RecipeFormatVersion: '2020-01-25'\nComponentName: com.example.YamlMinimal\nComponentVersion: 2.3.4\n",
        format: "YAML",
        stored_json: r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.YamlMinimal","ComponentVersion":"2.3.4","ComponentType":"aws.greengrass.generic","Lifecycle":{}}"#,
        name: "com.example.YamlMinimal",
        version: "2.3.4",
        description: None,
        publisher: None,
        platform_names: &[],
        tags: &[],
    },
    RecipeCase {
        label: "yaml, prerelease version and one platform manifest",
        recipe: "RecipeFormatVersion: '2020-01-25'\nComponentName: com.example.YamlFull\nComponentVersion: 0.1.0-alpha.1\nComponentDescription: A YAML component.\nComponentPublisher: Example Corp\nManifests:\n  - Platform:\n      os: linux\n    Lifecycle:\n      Run: echo hi\n",
        format: "YAML",
        stored_json: r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.YamlFull","ComponentVersion":"0.1.0-alpha.1","ComponentType":"aws.greengrass.generic","ComponentDescription":"A YAML component.","ComponentPublisher":"Example Corp","Manifests":[{"Platform":{"os":"linux"},"Lifecycle":{"Run":"echo hi"},"Artifacts":[]}],"Lifecycle":{}}"#,
        name: "com.example.YamlFull",
        version: "0.1.0-alpha.1",
        description: Some("A YAML component."),
        publisher: Some("Example Corp"),
        platform_names: &[None],
        tags: &[("owner", "platform-team")],
    },
    RecipeCase {
        label: "yaml, a lifecycle command carrying a shell '&&' and a '*' glob",
        recipe: "RecipeFormatVersion: '2020-01-25'\nComponentName: com.example.Shell\nComponentVersion: 4.5.6\nManifests:\n  - Platform:\n      os: linux\n    Lifecycle:\n      Run: \"make && ./run\"\n      Install: rm -f build/*.o\n",
        format: "YAML",
        stored_json: r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.Shell","ComponentVersion":"4.5.6","ComponentType":"aws.greengrass.generic","Manifests":[{"Platform":{"os":"linux"},"Lifecycle":{"Run":"make && ./run","Install":"rm -f build/*.o"},"Artifacts":[]}],"Lifecycle":{}}"#,
        name: "com.example.Shell",
        version: "4.5.6",
        description: None,
        publisher: None,
        platform_names: &[None],
        tags: &[],
    },
    RecipeCase {
        label: "json, two-digit major plus unrelated recipe sections",
        recipe: r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.Nested","ComponentVersion":"10.0.0","ComponentConfiguration":{"DefaultConfiguration":{"Message":"hello","Count":3}},"ComponentDependencies":{"aws.greengrass.Nucleus":{"VersionRequirement":">=2.0.0","DependencyType":"HARD"}}}"#,
        format: "JSON",
        stored_json: r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.Nested","ComponentVersion":"10.0.0","ComponentType":"aws.greengrass.generic","ComponentConfiguration":{"DefaultConfiguration":{"Message":"hello","Count":3}},"ComponentDependencies":{"aws.greengrass.Nucleus":{"VersionRequirement":">=2.0.0","DependencyType":"HARD"}},"Lifecycle":{}}"#,
        name: "com.example.Nested",
        version: "10.0.0",
        description: None,
        publisher: None,
        platform_names: &[],
        tags: &[],
    },
];

/// The `tags` member for a case's tag list, or an empty object when it has none.
fn tags_member(case: &RecipeCase) -> Value {
    let mut map = serde_json::Map::new();
    for (key, value) in case.tags {
        map.insert((*key).to_string(), Value::String((*value).to_string()));
    }
    if map.is_empty() {
        json!({})
    } else {
        json!({ "tags": Value::Object(map) })
    }
}

#[tokio::test]
async fn a_created_component_version_reads_back_exactly_as_it_was_published() {
    let router = emulator();

    // CREATE every row, and judge the create response itself.
    for case in RECIPES {
        let (status, body) = create_component(&router, case.recipe, &tags_member(case)).await;
        assert_eq!(
            status, 201,
            "case {:?}: CreateComponentVersion must answer 201, got {status} {body}",
            case.label
        );
        assert_eq!(
            text(&body, "componentName"),
            case.name,
            "case {:?}: componentName must come from the recipe, got {body}",
            case.label
        );
        assert_eq!(
            text(&body, "componentVersion"),
            case.version,
            "case {:?}: componentVersion must come from the recipe, got {body}",
            case.label
        );
        assert_eq!(
            text(&body, "arn"),
            version_arn(case.name, case.version),
            "case {:?}: the minted ARN must name the parsed component and version, got {body}",
            case.label
        );
        assert!(
            body.get("creationTimestamp").is_some(),
            "case {:?}: creationTimestamp is required, got {body}",
            case.label
        );
        // The create reports the version's state at the instant it ACCEPTED the
        // request, which is before it validated the recipe: the non-terminal
        // REQUESTED.
        assert_eq!(
            text(&body["status"], "componentState"),
            "REQUESTED",
            "case {:?}: the create must report the state at acceptance, got {body}",
            case.label
        );
    }

    // GET the recipe back, in the format it was submitted in and in the other one.
    for case in RECIPES {
        let arn = version_arn(case.name, case.version);
        for format in ["JSON", "YAML"] {
            let (status, body) = get(
                &router,
                &format!(
                    "/greengrass/v2/components/{}?recipeOutputFormat={format}",
                    encoded_label(&arn),
                ),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: GetComponent as {format} must answer 200, got {status} {body}",
                case.label
            );
            assert_eq!(
                text(&body, "recipeOutputFormat"),
                format,
                "case {:?}: recipeOutputFormat must echo the requested format, got {body}",
                case.label
            );
            let recipe = base64::decode(text(&body, "recipe")).expect("a base64 recipe blob");
            let document = String::from_utf8(recipe).expect("a UTF-8 recipe");

            // What comes back is the document the service DERIVED from the
            // submitted one: every submitted member, in the submitted order, plus
            // the members the service supplies — the assigned `ComponentType`, and
            // the recipe members whose default is the empty collection.
            if format == "JSON" {
                assert_eq!(
                    document, case.stored_json,
                    "case {:?} (submitted as {}): expected the derived document {:?}, \
                     got {document:?}",
                    case.label, case.format, case.stored_json
                );
            } else {
                // The YAML rendering is the same document, so it declares the same
                // members with the same values, in the same order.
                assert_eq!(
                    recipe_members(&document),
                    recipe_members(case.stored_json),
                    "case {:?} (submitted as {}): the {format} rendering must carry the \
                     same members as {:?}, got {document:?}",
                    case.label,
                    case.format,
                    case.stored_json
                );
            }
        }
    }

    // The create reported the state at the instant it accepted the request; the
    // recipe it validated is validated by the time it answered, so every read that
    // follows reports the terminal state.

    for case in RECIPES {
        let arn = version_arn(case.name, case.version);
        let (status, body) =
            get(&router, &format!("/greengrass/v2/components/{arn}/metadata")).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeComponent must answer 200, got {status} {body}",
            case.label
        );
        assert_eq!(
            (
                text(&body, "arn"),
                text(&body, "componentName"),
                text(&body, "componentVersion")
            ),
            (arn.clone(), case.name.to_string(), case.version.to_string()),
            "case {:?}: DescribeComponent must report what was persisted, got {body}",
            case.label
        );
        assert_eq!(
            body.get("description").and_then(Value::as_str),
            case.description,
            "case {:?}: the description comes from the recipe, got {body}",
            case.label
        );
        assert_eq!(
            body.get("publisher").and_then(Value::as_str),
            case.publisher,
            "case {:?}: the publisher comes from the recipe, got {body}",
            case.label
        );
        assert_eq!(
            text(&body["status"], "componentState"),
            "DEPLOYABLE",
            "case {:?}: a published version is validated, so it must be DEPLOYABLE, got {body}",
            case.label
        );

        let platforms: Vec<Option<&str>> = body
            .get("platforms")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|p| p.get("name").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            platforms, case.platform_names,
            "case {:?}: one platform per recipe manifest, in recipe order, got {body}",
            case.label
        );

        let tags = body.get("tags").cloned().unwrap_or(Value::Null);
        for (key, value) in case.tags {
            assert_eq!(
                tags.get(*key).and_then(Value::as_str),
                Some(*value),
                "case {:?}: tag {key} must be persisted, got {tags}",
                case.label
            );
        }
        // A component with no tags reports the EMPTY map, not an absent member, so a
        // client reading `tags` always finds one.
        assert_eq!(
            tags.as_object().map(serde_json::Map::len),
            Some(case.tags.len()),
            "case {:?}: the tag map must hold exactly the submitted tags, got {tags}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_large_but_shallow_recipe_is_accepted() {
    let router = emulator();

    // The indicator bound is a pre-parse upper bound on nesting, so it counts
    // hyphens inside scalars too. A real recipe can carry thousands: 400 manifests
    // whose Lifecycle commands use `--flag` pairs and hyphenated file names come to
    // roughly 3 000 indicators while nesting only four levels deep, and must be
    // accepted. This pins the false-rejection boundary.
    let mut recipe = String::from(
        "RecipeFormatVersion: '2020-01-25'\nComponentName: com.example.Big\nComponentVersion: 1.0.0\nManifests:\n",
    );
    for index in 0..400 {
        recipe.push_str(&format!(
            "  - Platform:\n      os: linux\n      architecture: arm-v{index}\n    Lifecycle:\n      Run: \"tool --input-file in-{index}.txt --output-dir out-{index}\"\n"
        ));
    }

    let (status, body) = create_component(&router, &recipe, &json!({})).await;
    assert_eq!(
        status, 201,
        "case {:?}: a large but shallow recipe must be accepted, got {status} {body}",
        "400 manifests"
    );

    let arn = version_arn("com.example.Big", "1.0.0");
    let (status, described) =
        get(&router, &format!("/greengrass/v2/components/{arn}/metadata")).await;
    assert_eq!(
        status, 200,
        "case {:?}: DescribeComponent must answer 200, got {status} {described}",
        "400 manifests"
    );
    assert_eq!(
        described["platforms"].as_array().map(Vec::len),
        Some(400),
        "case {:?}: one platform per manifest must be persisted, got {}",
        "400 manifests",
        described["platforms"]
            .as_array()
            .map_or_else(|| "none".to_string(), |items| items.len().to_string())
    );
}

#[tokio::test]
async fn an_unknown_component_version_is_not_found() {
    let router = emulator();
    let missing = version_arn("com.example.Absent", "1.0.0");

    for (label, path) in [
        ("DescribeComponent", format!("/greengrass/v2/components/{missing}/metadata")),
        ("GetComponent", format!("/greengrass/v2/components/{missing}")),
    ] {
        let (status, body) = get(&router, &path).await;
        assert_eq!(
            status, 404,
            "case {label:?}: an unknown ARN must answer 404, got {status} {body}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException, got {}",
            error_code(&body)
        );
    }
}

#[tokio::test]
async fn republishing_a_component_version_conflicts_but_a_new_version_does_not() {
    let router = emulator();
    let recipe = RECIPES[0].recipe;

    let (status, _) = create_component(&router, recipe, &json!({})).await;
    assert_eq!(status, 201, "the first publish must succeed, got {status}");

    let (status, body) = create_component(&router, recipe, &json!({})).await;
    assert_eq!(
        status, 409,
        "case {:?}: republishing the same name+version must conflict, got {status} {body}",
        "same name and version"
    );
    assert!(
        error_code(&body).contains("ConflictException"),
        "expected ConflictException, got {}",
        error_code(&body)
    );

    // A different version of the SAME component is a new resource, not a conflict.
    let next = r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.Minimal","ComponentVersion":"1.0.1"}"#;
    let (status, body) = create_component(&router, next, &json!({})).await;
    assert_eq!(
        status, 201,
        "case {:?}: a new version of the same component must be accepted, got {status} {body}",
        "same name, new version"
    );
}

#[tokio::test]
async fn a_replayed_client_token_returns_the_first_result_instead_of_a_duplicate() {
    let router = emulator();
    let recipe = RECIPES[0].recipe;
    let token = json!({ "clientToken": "token-abc-123" });

    let (status, first) = create_component(&router, recipe, &token).await;
    assert_eq!(status, 201, "the first publish must succeed, got {status} {first}");

    let (status, replay) = create_component(&router, recipe, &token).await;
    assert_eq!(
        status, 201,
        "case {:?}: a replayed clientToken must not conflict, got {status} {replay}",
        "same token, same recipe"
    );
    assert_eq!(
        text(&replay, "arn"),
        text(&first, "arn"),
        "case {:?}: the replay must return the first result, got {replay}",
        "same token, same recipe"
    );
    assert_eq!(
        text(&replay, "creationTimestamp"),
        text(&first, "creationTimestamp"),
        "case {:?}: the replay must not restamp the resource, got {replay}",
        "same token, same recipe"
    );

    // The same recipe under a FRESH token is a genuine duplicate create.
    let (status, body) =
        create_component(&router, recipe, &json!({ "clientToken": "token-xyz-999" })).await;
    assert_eq!(
        status, 409,
        "case {:?}: a new token does not excuse a duplicate version, got {status} {body}",
        "new token, same recipe"
    );
}

#[tokio::test]
async fn a_recipe_that_is_not_a_recipe_is_refused() {
    let router = emulator();

    // Each row breaks the recipe in a different way; every one must be a 400
    // ValidationException, never a 2xx and never a 501.
    let cases: &[(&str, &str)] = &[
        ("empty document", ""),
        ("a sequence, not a mapping", "[1, 2, 3]"),
        ("a bare scalar", "\"just a string\""),
        (
            "missing ComponentName",
            r#"{"RecipeFormatVersion":"2020-01-25","ComponentVersion":"1.0.0"}"#,
        ),
        (
            "missing ComponentVersion",
            r#"{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.NoVersion"}"#,
        ),
        ("unparseable yaml", "ComponentName: [unclosed\n"),
    ];

    for (label, recipe) in cases {
        let (status, body) = create_component(&router, recipe, &json!({})).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a broken recipe must answer 400, got {status} {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {label:?}: expected ValidationException, got {}",
            error_code(&body)
        );
    }

    // A recipe that breaches one of the emulator's bounds on an untrusted
    // document is refused the same way — a 400, never a crash or a 2xx. Each row
    // is built, not written out, so it stays inside the HTTP body cap.
    let alias_bomb = {
        // The classic YAML alias ("billion laughs") shape: a few hundred bytes
        // that would expand to ~9^8 nodes.
        let mut text = String::from("ComponentName: com.example.Bomb\nComponentVersion: 1.0.0\na0: &a0 [\"x\",\"x\",\"x\",\"x\",\"x\",\"x\",\"x\",\"x\",\"x\"]\n");
        for level in 1..9 {
            let previous = level - 1;
            text.push_str(&format!("a{level}: &a{level} ["));
            for item in 0..9 {
                if item > 0 {
                    text.push(',');
                }
                text.push_str(&format!("*a{previous}"));
            }
            text.push_str("]\n");
        }
        text
    };
    let deep_flow = format!(
        "ComponentName: com.example.Deep\nComponentVersion: 1.0.0\nManifests: {}{}\n",
        "[".repeat(512),
        "]".repeat(512)
    );
    // Just past MAX_RECIPE_FLOW_OPENERS: flow nesting costs neither a column nor a
    // line, so the pre-parse guard has to refuse this before the parser descends.
    // Hidden closers cannot buy the depth back, because closers are never
    // subtracted.
    let flow_past_the_bound = format!(
        "ComponentName: com.example.Flow\nComponentVersion: 1.0.0\nManifests: {}\n",
        "[\"]\",".repeat(1100)
    );
    let deep_block = {
        let mut text = String::from("ComponentName: com.example.Indented\nComponentVersion: 1.0.0\n");
        for level in 0..400 {
            text.push_str(&" ".repeat(level));
            text.push_str(&format!("k{level}:\n"));
        }
        text
    };
    let oversized = format!(
        r#"{{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.Huge","ComponentVersion":"1.0.0","Pad":"{}"}}"#,
        "p".repeat(400 * 1024)
    );
    // Compact block sequence entries nest one level per two bytes with NO
    // indentation and no flow indicator. 8192 of them is past
    // MAX_RECIPE_COMPACT_PER_LINE (and the total indicator bound as well), so the
    // PRE-parse guard is what refuses this input, before the loader descends a
    // single level.
    let compact_sequence = format!(
        "ComponentName: com.example.Compact\nComponentVersion: 1.0.0\nManifests: {}x\n",
        "- ".repeat(8192)
    );
    // An anchor on its own expands nothing and is legitimate YAML; only an ALIAS
    // can multiply a document, and the loader resolves one by copying, so these
    // rows are refused by the pre-load parser event scan rather than by a byte scan.
    let alias_behind_explicit_key =
        "ComponentName: com.example.Explicit\nComponentVersion: 1.0.0\nx: &a [1, 2]\n? *a\n: 1\n";
    let alias_behind_tag =
        "ComponentName: com.example.Tagged\nComponentVersion: 1.0.0\nx: &a str\ny: !!str *a\n";
    // A '#' that directly follows a non-space byte is NOT a comment in YAML, so a
    // guard that skipped to end-of-line there would miss every indicator after it.
    let depth_behind_hash = format!(
        "ComponentName: com.example.Hash\nComponentVersion: 1.0.0\nManifests: [a#,{}\n",
        "[".repeat(2048)
    );

    for (label, recipe) in [
        ("yaml alias bomb", alias_bomb.as_str()),
        ("deeply nested flow collections", deep_flow.as_str()),
        ("flow openers past the pre-parse bound", flow_past_the_bound.as_str()),
        ("deeply indented block mapping", deep_block.as_str()),
        ("compact block sequence on one line", compact_sequence.as_str()),
        ("alias behind the explicit-key indicator", alias_behind_explicit_key),
        ("alias behind a tag", alias_behind_tag),
        ("nesting hidden behind a non-comment '#'", depth_behind_hash.as_str()),
        ("larger than the recipe size bound", oversized.as_str()),
    ] {
        let (status, body) = create_component(&router, recipe, &json!({})).await;
        assert_eq!(
            status, 400,
            "case {label:?}: an out-of-bounds recipe must answer 400, got {status} {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {label:?}: expected ValidationException, got {}",
            error_code(&body)
        );
    }

    // Neither source given at all.
    let (status, body) = post(&router, "/greengrass/v2/createComponentVersion", &json!({})).await;
    assert_eq!(
        status, 400,
        "case {:?}: one of inlineRecipe / lambdaFunction is required, got {status} {body}",
        "no recipe source"
    );

    // A Lambda source cannot be honoured without that function's deployment
    // package, so it is answered as not implemented rather than invented.
    let (status, body) = post(
        &router,
        "/greengrass/v2/createComponentVersion",
        &json!({
            "lambdaFunction": {
                "lambdaArn": format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:hello:1")
            }
        }),
    )
    .await;
    assert_eq!(
        status, 501,
        "case {:?}: a Lambda-sourced component must answer 501, got {status} {body}",
        "lambdaFunction source"
    );
}

#[tokio::test]
async fn the_latest_version_of_a_component_is_the_greatest_semantic_version() {
    let router = emulator();

    // Byte order would pick "2.2.0" here; semantic-version precedence picks
    // "10.0.1", because a two-digit major outranks a one-digit one and a
    // prerelease ranks below the release it precedes.
    let versions = [
        "1.0.0",
        "2.0.0",
        "2.2.0",
        "2.10.0",
        "10.0.0",
        "10.0.1-rc.1",
        "10.0.1",
    ];
    for version in versions {
        let recipe = format!(
            r#"{{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.Many","ComponentVersion":"{version}"}}"#
        );
        let (status, body) = create_component(&router, &recipe, &json!({})).await;
        assert_eq!(
            status, 201,
            "case {version:?}: publishing the version must succeed, got {status} {body}"
        );
    }

    let (status, body) = get(&router, "/greengrass/v2/components").await;
    assert_eq!(status, 200, "ListComponents must answer 200, got {status} {body}");
    let components = body["components"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        components.len(),
        1,
        "every version belongs to the one component, got {body}"
    );
    let latest = &components[0]["latestVersion"];
    assert_eq!(
        text(latest, "componentVersion"),
        "10.0.1",
        "the latest version must be the greatest semantic version, got {body}"
    );
    assert_eq!(
        text(latest, "arn"),
        version_arn("com.example.Many", "10.0.1"),
        "latestVersion.arn must be the version-level ARN, got {body}"
    );
    assert_eq!(
        text(&components[0], "arn"),
        format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:components:com.example.Many"),
        "a Component's own ARN is name-level, with no :versions: segment, got {body}"
    );
}

#[tokio::test]
async fn list_components_pages_through_every_component_in_a_stable_order() {
    let router = emulator();

    let names = [
        "com.example.Alpha",
        "com.example.Bravo",
        "com.example.Charlie",
        "com.example.Delta",
        "com.example.Echo",
    ];
    for name in names {
        let recipe = format!(
            r#"{{"RecipeFormatVersion":"2020-01-25","ComponentName":"{name}","ComponentVersion":"1.0.0"}}"#
        );
        let (status, body) = create_component(&router, &recipe, &json!({})).await;
        assert_eq!(status, 201, "publishing {name} must succeed, got {status} {body}");
    }

    // Each row is a different page size, including both published bounds of
    // `maxResults` (1 and 100) and sizes that do and do not divide the collection.
    for page_size in [1, 2, 3, 5, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut path = format!("/greengrass/v2/components?maxResults={page_size}");
            if let Some(token) = &token {
                path.push_str(&format!("&nextToken={token}"));
            }
            let (status, body) = get(&router, &path).await;
            assert_eq!(
                status, 200,
                "case maxResults={page_size}: ListComponents must answer 200, got {status} {body}"
            );
            let page = body["components"].as_array().cloned().unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case maxResults={page_size}: a page must not exceed maxResults, got {} items",
                page.len()
            );
            for item in &page {
                seen.push(text(item, "componentName"));
            }
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case maxResults={page_size}: pagination did not terminate after {pages} pages"
            );
            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen,
            names.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "case maxResults={page_size}: paging must yield every component exactly once, in name order"
        );
    }

    // A PUBLIC listing enumerates the components AWS publishes, of which this
    // emulator holds none; the private ones must not leak into it.
    let (status, body) = get(&router, "/greengrass/v2/components?scope=PUBLIC").await;
    assert_eq!(status, 200, "a PUBLIC listing must answer 200, got {status} {body}");
    assert_eq!(
        body["components"].as_array().map(Vec::len).unwrap_or(0),
        0,
        "no component here is PUBLIC, got {body}"
    );

    // An explicit PRIVATE scope is the same listing as the default.
    let (status, private) = get(&router, "/greengrass/v2/components?scope=PRIVATE").await;
    assert_eq!(status, 200, "a PRIVATE listing must answer 200, got {status} {private}");
    let (_, default) = get(&router, "/greengrass/v2/components").await;
    assert_eq!(
        private["components"], default["components"],
        "PRIVATE is the default scope, got {private} vs {default}"
    );

    // A token this service never issued is a client error, not a panic or a page.
    let (status, body) = get(&router, "/greengrass/v2/components?nextToken=not-a-cursor").await;
    assert_eq!(
        status, 400,
        "case {:?}: a forged cursor must be refused, got {status} {body}",
        "forged nextToken"
    );
}

#[tokio::test]
async fn deleting_a_component_version_hides_it_from_every_reader() {
    let router = emulator();

    for version in ["1.0.0", "1.1.0"] {
        let recipe = format!(
            r#"{{"RecipeFormatVersion":"2020-01-25","ComponentName":"com.example.Doomed","ComponentVersion":"{version}"}}"#
        );
        let (status, body) = create_component(&router, &recipe, &json!({})).await;
        assert_eq!(status, 201, "publishing {version} must succeed, got {status} {body}");
    }

    let doomed = version_arn("com.example.Doomed", "1.1.0");
    // Addressed with the percent-encoded label an SDK would send.
    let (status, body) = delete(
        &router,
        &format!("/greengrass/v2/components/{}", encoded_label(&doomed)),
    )
    .await;
    assert_eq!(
        status, 204,
        "case {:?}: DeleteComponent must answer 204, got {status} {body}",
        "delete the newer version"
    );

    for (label, path) in [
        ("DescribeComponent", format!("/greengrass/v2/components/{doomed}/metadata")),
        ("GetComponent", format!("/greengrass/v2/components/{doomed}")),
    ] {
        let (status, body) = get(&router, &path).await;
        assert_eq!(
            status, 404,
            "case {label:?}: the deleted version must be gone, got {status} {body}"
        );
    }

    // Deleting one version leaves the others, and the component's latest version
    // is recomputed from what remains.
    let (status, body) = get(&router, "/greengrass/v2/components").await;
    assert_eq!(status, 200, "ListComponents must answer 200, got {status} {body}");
    assert_eq!(
        text(&body["components"][0]["latestVersion"], "componentVersion"),
        "1.0.0",
        "after the delete the latest surviving version must be reported, got {body}"
    );

    // Deleting the last version removes the component itself.
    let survivor = version_arn("com.example.Doomed", "1.0.0");
    let (status, _) = delete(&router, &format!("/greengrass/v2/components/{survivor}")).await;
    assert_eq!(status, 204, "deleting the last version must answer 204, got {status}");
    let (_, body) = get(&router, "/greengrass/v2/components").await;
    assert_eq!(
        body["components"].as_array().map(Vec::len).unwrap_or(0),
        0,
        "a component with no versions is no longer listed, got {body}"
    );

    // And a second delete of the same ARN is a 404, not a second 204.
    let (status, body) = delete(&router, &format!("/greengrass/v2/components/{survivor}")).await;
    assert_eq!(
        status, 404,
        "case {:?}: deleting an absent version must answer 404, got {status} {body}",
        "delete twice"
    );
}

// ---------------------------------------------------------------------------
// Deployments
// ---------------------------------------------------------------------------

fn thing_target(name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT}:thing/{name}")
}

#[tokio::test]
async fn a_deployment_reads_back_as_created_and_supersedes_its_target_predecessor() {
    let router = emulator();
    let recipe = RECIPES[0].recipe;
    let (status, _) = create_component(&router, recipe, &json!({})).await;
    assert_eq!(status, 201, "publishing the component must succeed, got {status}");

    let target = thing_target("MyCore");
    let request = json!({
        "targetArn": target,
        "deploymentName": "hello-world-rollout",
        "components": {
            "com.example.Minimal": {
                "componentVersion": "1.0.0",
                "configurationUpdate": { "merge": "{\"Message\":\"hi\"}" },
                "runWith": { "posixUser": "ggc_user:ggc_group" }
            }
        },
        "deploymentPolicies": {
            "failureHandlingPolicy": "ROLLBACK",
            "componentUpdatePolicy": { "timeoutInSeconds": 90, "action": "NOTIFY_COMPONENTS" }
        },
        "iotJobConfiguration": {
            "timeoutConfig": { "inProgressTimeoutInMinutes": 15 }
        },
        "tags": { "wave": "one" }
    });

    let (status, first) = post(&router, "/greengrass/v2/deployments", &request).await;
    assert_eq!(
        status, 201,
        "CreateDeployment must answer 201, got {status} {first}"
    );
    let first_id = text(&first, "deploymentId");
    let first_job = text(&first, "iotJobId");
    assert!(
        !first_id.is_empty() && !first_job.is_empty(),
        "a deployment must be given an id and an IoT job, got {first}"
    );
    assert_eq!(
        text(&first, "iotJobArn"),
        format!("arn:aws:iot:{REGION}:{ACCOUNT}:job/{first_job}"),
        "iotJobArn must name the iotJobId it was issued with, got {first}"
    );

    let (status, stored) = get(&router, &format!("/greengrass/v2/deployments/{first_id}")).await;
    assert_eq!(status, 200, "GetDeployment must answer 200, got {status} {stored}");
    assert_eq!(
        text(&stored, "targetArn"),
        target,
        "GetDeployment must report the persisted targetArn, got {stored}"
    );
    assert_eq!(
        text(&stored, "deploymentName"),
        "hello-world-rollout",
        "GetDeployment must report the persisted deploymentName, got {stored}"
    );
    assert_eq!(
        text(&stored, "deploymentStatus"),
        "ACTIVE",
        "a new deployment is ACTIVE for its target, got {stored}"
    );
    assert_eq!(
        text(&stored, "revisionId"),
        "1",
        "the first deployment to a target is revision 1, got {stored}"
    );
    assert_eq!(
        stored.get("isLatestForTarget").and_then(Value::as_bool),
        Some(true),
        "the only deployment for a target is the latest one, got {stored}"
    );
    assert_eq!(
        stored["components"], request["components"],
        "the components map must round-trip verbatim, got {stored}"
    );
    assert_eq!(
        stored["deploymentPolicies"], request["deploymentPolicies"],
        "the deployment policies must round-trip verbatim, got {stored}"
    );
    assert_eq!(
        stored["iotJobConfiguration"], request["iotJobConfiguration"],
        "the IoT job configuration must round-trip verbatim, got {stored}"
    );
    assert_eq!(
        stored["tags"], request["tags"],
        "the deployment tags must round-trip verbatim, got {stored}"
    );
    assert!(
        stored.get("creationTimestamp").is_some(),
        "a deployment carries a creationTimestamp, got {stored}"
    );

    // A second deployment to the same target supersedes the first.
    let (status, second) = post(
        &router,
        "/greengrass/v2/deployments",
        &json!({ "targetArn": target, "deploymentName": "hello-world-rollout-2" }),
    )
    .await;
    assert_eq!(status, 201, "the second deployment must succeed, got {status} {second}");
    let second_id = text(&second, "deploymentId");
    assert_ne!(
        second_id, first_id,
        "each deployment gets its own id, got {second}"
    );

    let (_, superseded) = get(&router, &format!("/greengrass/v2/deployments/{first_id}")).await;
    assert_eq!(
        superseded.get("isLatestForTarget").and_then(Value::as_bool),
        Some(false),
        "case {:?}: the superseded deployment is no longer the latest, got {superseded}",
        "second deployment, same target"
    );
    let (_, latest) = get(&router, &format!("/greengrass/v2/deployments/{second_id}")).await;
    assert_eq!(
        latest.get("isLatestForTarget").and_then(Value::as_bool),
        Some(true),
        "case {:?}: the new deployment is the latest, got {latest}",
        "second deployment, same target"
    );
    assert_eq!(
        text(&latest, "revisionId"),
        "2",
        "the second deployment to a target is revision 2, got {latest}"
    );

    // A deployment to a DIFFERENT target has its own revision line and does not
    // disturb the first target's latest.
    let other = format!("arn:aws:iot:{REGION}:{ACCOUNT}:thinggroup/Fleet");
    let (status, elsewhere) = post(
        &router,
        "/greengrass/v2/deployments",
        &json!({ "targetArn": other }),
    )
    .await;
    assert_eq!(status, 201, "a thing-group deployment must succeed, got {status} {elsewhere}");
    let (_, elsewhere) = get(
        &router,
        &format!("/greengrass/v2/deployments/{}", text(&elsewhere, "deploymentId")),
    )
    .await;
    assert_eq!(
        text(&elsewhere, "revisionId"),
        "1",
        "case {:?}: revisions are per target, got {elsewhere}",
        "first deployment to a second target"
    );
    let (_, latest) = get(&router, &format!("/greengrass/v2/deployments/{second_id}")).await;
    assert_eq!(
        latest.get("isLatestForTarget").and_then(Value::as_bool),
        Some(true),
        "case {:?}: another target's deployment must not demote this one, got {latest}",
        "first deployment to a second target"
    );

    // An unknown deployment id is a 404.
    let (status, body) = get(
        &router,
        "/greengrass/v2/deployments/00000000-0000-4000-8000-000000000000",
    )
    .await;
    assert_eq!(status, 404, "an unknown deployment must answer 404, got {status} {body}");
    assert!(
        error_code(&body).contains("ResourceNotFoundException"),
        "expected ResourceNotFoundException, got {}",
        error_code(&body)
    );
}

#[tokio::test]
async fn a_deployment_target_must_be_an_iot_thing_or_thing_group_arn() {
    let router = emulator();

    // Accepted: the two resource types the TargetARN pattern admits, in either
    // partition form, with or without a path-bearing resource name.
    let accepted = [
        format!("arn:aws:iot:{REGION}:{ACCOUNT}:thing/MyCore"),
        format!("arn:aws:iot:{REGION}:{ACCOUNT}:thinggroup/Fleet"),
        format!("arn:aws-us-gov:iot:{REGION}:{ACCOUNT}:thing/GovCore"),
        format!("arn:aws:iot:{REGION}:{ACCOUNT}:thinggroup/nested/group"),
    ];
    for target in &accepted {
        let (status, body) = post(
            &router,
            "/greengrass/v2/deployments",
            &json!({ "targetArn": target }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {target:?}: a well-formed target must be accepted, got {status} {body}"
        );
    }

    // Refused: not an ARN at all, the wrong service, the wrong resource type, a
    // non-numeric account, and an empty resource name.
    let refused = [
        "not-an-arn".to_string(),
        format!("arn:aws:iam:{REGION}:{ACCOUNT}:role/MyRole"),
        format!("arn:aws:iot:{REGION}:{ACCOUNT}:certificate/abc"),
        format!("arn:aws:iot:{REGION}:not-an-account:thing/MyCore"),
        format!("arn:aws:iot:{REGION}:{ACCOUNT}:thing/"),
        String::new(),
    ];
    for target in &refused {
        let (status, body) = post(
            &router,
            "/greengrass/v2/deployments",
            &json!({ "targetArn": target }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {target:?}: a malformed target must be refused, got {status} {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {target:?}: expected ValidationException, got {}",
            error_code(&body)
        );
    }

    // And targetArn is required.
    let (status, body) = post(&router, "/greengrass/v2/deployments", &json!({})).await;
    assert_eq!(
        status, 400,
        "case {:?}: targetArn is required, got {status} {body}",
        "no targetArn"
    );
}

#[tokio::test]
async fn a_replayed_deployment_client_token_returns_the_first_deployment() {
    let router = emulator();
    let target = thing_target("TokenCore");
    let request = json!({ "targetArn": target, "clientToken": "deploy-token-1" });

    let (status, first) = post(&router, "/greengrass/v2/deployments", &request).await;
    assert_eq!(status, 201, "the first deployment must succeed, got {status} {first}");

    let (status, replay) = post(&router, "/greengrass/v2/deployments", &request).await;
    assert_eq!(status, 201, "a replayed token must not fail, got {status} {replay}");
    assert_eq!(
        text(&replay, "deploymentId"),
        text(&first, "deploymentId"),
        "case {:?}: the replay must return the first deployment, got {replay}",
        "same deployment token"
    );
    assert_eq!(
        text(&replay, "iotJobId"),
        text(&first, "iotJobId"),
        "case {:?}: the replay must not mint a second IoT job, got {replay}",
        "same deployment token"
    );

    // The replay created nothing, so the target is still on revision 1.
    let (_, stored) = get(
        &router,
        &format!("/greengrass/v2/deployments/{}", text(&first, "deploymentId")),
    )
    .await;
    assert_eq!(
        text(&stored, "revisionId"),
        "1",
        "case {:?}: a replay must not advance the target's revision, got {stored}",
        "same deployment token"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for reading a Greengrass V2 resource's tags.
//!
//! Every assertion drives the real `restJson1` front door: the request is built the
//! way an SDK builds it, dispatched through [`MultiServiceRouter`], and judged on the
//! HTTP status and the JSON body. Nothing reaches into the state struct, so a handler
//! that answered from a canned body instead of from what the create persisted fails
//! here.
//!
//! The rule under test: the tag listing reports the tag set stored ON the resource the
//! `resourceArn` names — the same set the resource's own reader reports — and a
//! `resourceArn` naming no stored resource is a 404.

use std::sync::Arc;

use aws_smithy_types::base64;
use bytes::Bytes;
use serde_json::{json, Map, Value};
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

/// The wire error code of a `restJson1` error body.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// A resource ARN as a client percent-encodes a non-greedy `@httpLabel`.
fn encoded_label(arn: &str) -> String {
    arn.replace(':', "%3A").replace('/', "%2F")
}

/// `ListTagsForResource` on `arn` (`GET /tags/{resourceArn}`).
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    get(router, &format!("/tags/{}", encoded_label(arn))).await
}

/// The component-version ARN this emulator mints for `name`@`version`.
fn version_arn(name: &str, version: &str) -> String {
    format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:components:{name}:versions:{version}")
}

/// The deployment ARN of `deployment_id`, the `deployments:<id>` resource field the
/// model's `GenericV2ARN` pattern admits.
fn deployment_arn(deployment_id: &str) -> String {
    format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:deployments:{deployment_id}")
}

/// An IoT thing ARN, which is what a deployment targets.
fn thing_target(name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT}:thing/{name}")
}

/// A tag map as a JSON object.
fn tag_object(tags: &[(String, String)]) -> Value {
    let mut map = Map::new();
    for (key, value) in tags {
        map.insert(key.clone(), Value::String(value.clone()));
    }
    Value::Object(map)
}

/// The smallest recipe the parser accepts, for `name`@`version`. Keys from the IoT
/// Greengrass component recipe reference (`RecipeFormatVersion`, `ComponentName`,
/// `ComponentVersion`).
fn recipe(name: &str, version: &str) -> String {
    format!(
        r#"{{"RecipeFormatVersion":"2020-01-25","ComponentName":"{name}","ComponentVersion":"{version}"}}"#
    )
}

/// `CreateComponentVersion` for `name`@`version`, carrying `tags` when there are any.
async fn create_component(
    router: &MultiServiceRouter,
    name: &str,
    version: &str,
    tags: &[(String, String)],
) -> (u16, Value) {
    let mut body = json!({ "inlineRecipe": base64::encode(recipe(name, version).as_bytes()) });
    if !tags.is_empty() {
        body["tags"] = tag_object(tags);
    }
    post(router, "/greengrass/v2/createComponentVersion", &body).await
}

/// `CreateDeployment` for `target`, carrying `tags` when there are any; answers the
/// new deployment's id.
async fn create_deployment(
    router: &MultiServiceRouter,
    target: &str,
    tags: &[(String, String)],
) -> String {
    let mut body = json!({ "targetArn": target });
    if !tags.is_empty() {
        body["tags"] = tag_object(tags);
    }
    let (status, created) = post(router, "/greengrass/v2/deployments", &body).await;
    assert_eq!(
        status, 201,
        "CreateDeployment must answer 201, got {status} {created}"
    );
    created["deploymentId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------
// The tag table
// ---------------------------------------------------------------------------

/// Which kind of resource a row tags. The model's `GenericV2ARN` pattern admits
/// `components`, `deployments` and `coreDevices`; the first two are the resources
/// this emulator holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    ComponentVersion,
    Deployment,
}

/// One row of the tag table: a resource to create, and the tag set its create carries.
#[derive(Debug)]
struct TagCase {
    label: &'static str,
    kind: Kind,
    /// The component name / thing name the row's resource is created under.
    resource_name: &'static str,
    tags: &'static [(&'static str, &'static str)],
}

/// The rows differ along what the rule generalises over: the resource KIND whose tags
/// are read, how MANY tags it carries (none, one, several), and the FORM of a tag's
/// key and value (ASCII, punctuation, non-ASCII, the empty value `TagValue`'s
/// `length 0..256` admits).
const TAG_CASES: &[TagCase] = &[
    TagCase {
        label: "a component version with no tags",
        kind: Kind::ComponentVersion,
        resource_name: "com.example.Untagged",
        tags: &[],
    },
    TagCase {
        label: "a component version with one tag",
        kind: Kind::ComponentVersion,
        resource_name: "com.example.OneTag",
        tags: &[("owner", "platform-team")],
    },
    TagCase {
        label: "a component version with three tags, one of them empty-valued",
        kind: Kind::ComponentVersion,
        resource_name: "com.example.ThreeTags",
        tags: &[("owner", "platform-team"), ("tier", "test"), ("retired", "")],
    },
    TagCase {
        label: "a deployment with no tags",
        kind: Kind::Deployment,
        resource_name: "UntaggedCore",
        tags: &[],
    },
    TagCase {
        label: "a deployment with two tags",
        kind: Kind::Deployment,
        resource_name: "TaggedCore",
        tags: &[("wave", "one"), ("env", "staging")],
    },
    TagCase {
        label: "a deployment whose tag keys and values carry punctuation and non-ASCII",
        kind: Kind::Deployment,
        resource_name: "PunctuationCore",
        tags: &[
            ("aws:cost/center", "line=42"),
            ("ünïcode", "naïve value"),
            ("spaced key", "spaced value"),
        ],
    },
];

/// A row's tags as owned pairs.
fn pairs(case: &TagCase) -> Vec<(String, String)> {
    case.tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Create the row's resource and answer the ARN its tags are read by, plus the path
/// of the resource's OWN reader (`DescribeComponent` / `GetDeployment`), which must
/// report the same tag set.
async fn create_case(router: &MultiServiceRouter, case: &TagCase) -> (String, String) {
    let tags = pairs(case);
    match case.kind {
        Kind::ComponentVersion => {
            let (status, body) = create_component(router, case.resource_name, "1.0.0", &tags).await;
            assert_eq!(
                status, 201,
                "case {:?}: CreateComponentVersion must answer 201, got {status} {body}",
                case.label
            );
            let arn = version_arn(case.resource_name, "1.0.0");
            let reader = format!("/greengrass/v2/components/{}/metadata", encoded_label(&arn));
            (arn, reader)
        }
        Kind::Deployment => {
            let id = create_deployment(router, &thing_target(case.resource_name), &tags).await;
            assert!(
                !id.is_empty(),
                "case {:?}: CreateDeployment must answer a deployment id",
                case.label
            );
            let reader = format!("/greengrass/v2/deployments/{id}");
            (deployment_arn(&id), reader)
        }
    }
}

#[tokio::test]
async fn a_resources_tags_are_listed_exactly_as_its_create_persisted_them() {
    let router = emulator();

    for case in TAG_CASES {
        let (arn, reader) = create_case(&router, case).await;
        let expected = tag_object(&pairs(case));

        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: ListTagsForResource must answer 200, got {status} {body}",
            case.label
        );
        assert_eq!(
            body["tags"], expected,
            "case {:?}: the listing must report exactly the submitted tags, got {body}",
            case.label
        );
        // A resource with no tags reports the EMPTY map, not an absent member, so a
        // client reading `tags` always finds one.
        assert_eq!(
            body["tags"].as_object().map(Map::len),
            Some(case.tags.len()),
            "case {:?}: the tag map must hold exactly {} entries, got {body}",
            case.label,
            case.tags.len()
        );

        // The listing and the resource's own reader report ONE tag set, because the
        // tags are stored on the resource rather than beside it.
        let (status, stored) = get(&router, &reader).await;
        assert_eq!(
            status, 200,
            "case {:?}: the resource's own reader must answer 200, got {status} {stored}",
            case.label
        );
        assert_eq!(
            stored["tags"], body["tags"],
            "case {:?}: the resource reports the tags the listing does, got {stored}",
            case.label
        );

        // Listing is a report: a second listing sees what the first did.
        let (status, again) = list_tags(&router, &arn).await;
        assert_eq!(
            (status, &again["tags"]),
            (200, &body["tags"]),
            "case {:?}: a repeated listing must not change what a caller sees, got {status} {again}",
            case.label
        );
    }
}

/// `TagMap`'s `length 1..200`, `TagKey`'s `length 1..128` and `TagValue`'s
/// `length 0..256` (`smithy_query(query="shape",shape="TagMap,TagKey,TagValue")`).
const MAX_TAGS: usize = 200;
const MAX_TAG_KEY: usize = 128;
const MAX_TAG_VALUE: usize = 256;

#[tokio::test]
async fn the_listing_reports_a_tag_set_at_every_published_bound() {
    let router = emulator();

    // The upper bound: exactly MAX_TAGS tags, the longest key and the longest value
    // among them, and the empty value the lower value bound admits.
    let mut tags: Vec<(String, String)> = (0..MAX_TAGS - 2)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    tags.push(("k".repeat(MAX_TAG_KEY), "v".repeat(MAX_TAG_VALUE)));
    tags.push(("empty-valued".to_string(), String::new()));

    let id = create_deployment(&router, &thing_target("BoundCore"), &tags).await;
    let (status, body) = list_tags(&router, &deployment_arn(&id)).await;
    assert_eq!(
        status, 200,
        "case {MAX_TAGS} tags at the key and value bounds: must answer 200, got {status} {body}"
    );
    assert_eq!(
        body["tags"],
        tag_object(&tags),
        "case {MAX_TAGS} tags at the key and value bounds: every tag must read back verbatim, got {body}"
    );

    // One past each bound is refused by the modelled constraint, so no resource ever
    // holds a tag set the listing could report outside it.
    let too_many: Vec<(String, String)> = (0..=MAX_TAGS)
        .map(|index| (format!("key-{index}"), String::new()))
        .collect();
    let past_bounds = [
        ("one tag past the map bound", too_many),
        ("one tag short of the map bound", Vec::new()),
    ];
    for (case, tags) in past_bounds {
        let mut body = json!({ "targetArn": thing_target("BoundCore") });
        body["tags"] = tag_object(&tags);
        let (status, refused) = post(&router, "/greengrass/v2/deployments", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the create must be refused with 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );
    }
}

#[tokio::test]
async fn an_arn_that_names_no_stored_resource_is_not_found() {
    let router = emulator();

    // One resource of each kind exists, so every row below differs from a stored
    // resource only in what it names.
    let present_tags = vec![("owner".to_string(), "platform-team".to_string())];
    let (status, body) =
        create_component(&router, "com.example.Present", "1.0.0", &present_tags).await;
    assert_eq!(
        status, 201,
        "publishing the component must succeed, got {status} {body}"
    );
    let present_deployment =
        create_deployment(&router, &thing_target("PresentCore"), &[]).await;

    let rows = [
        (
            "a component version of a component never published",
            version_arn("com.example.Absent", "1.0.0"),
            "COMPONENT_VERSION",
        ),
        (
            "a version never published of a component that exists",
            version_arn("com.example.Present", "9.9.9"),
            "COMPONENT_VERSION",
        ),
        (
            "a published component version in another region",
            format!(
                "arn:aws:greengrass:eu-west-1:{ACCOUNT}:components:com.example.Present:versions:1.0.0"
            ),
            "COMPONENT_VERSION",
        ),
        (
            "a published component version under another account",
            format!(
                "arn:aws:greengrass:{REGION}:111111111111:components:com.example.Present:versions:1.0.0"
            ),
            "COMPONENT_VERSION",
        ),
        (
            "a component ARN with no version, which names no taggable resource",
            format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:components:com.example.Present"),
            "RESOURCE",
        ),
        (
            "a deployment id nothing created",
            deployment_arn("44444444-4444-4444-4444-444444444444"),
            "DEPLOYMENT",
        ),
        (
            "a core device, which no operation registers",
            format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:coreDevices:PresentCore"),
            "CORE_DEVICE",
        ),
    ];

    for (case, arn, resource_type) in rows {
        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: an ARN naming no stored resource must answer 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: the refusal names the ARN the caller sent, got {body}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some(resource_type),
            "case {case:?}: the refusal names the resource kind the ARN addressed, got {body}"
        );
    }

    // The stored resources are still readable, so the rows above were refused for
    // what they named and not because the listing lost the resources.
    for (case, arn) in [
        (
            "the published component version",
            version_arn("com.example.Present", "1.0.0"),
        ),
        ("the created deployment", deployment_arn(&present_deployment)),
    ] {
        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the stored resource must still list its tags, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn a_deleted_component_versions_tags_are_no_longer_listed() {
    let router = emulator();
    let tags = vec![("owner".to_string(), "platform-team".to_string())];
    let (status, body) = create_component(&router, "com.example.Doomed", "2.0.0", &tags).await;
    assert_eq!(
        status, 201,
        "publishing the component must succeed, got {status} {body}"
    );
    let arn = version_arn("com.example.Doomed", "2.0.0");

    let (status, body) = list_tags(&router, &arn).await;
    assert_eq!(
        (status, &body["tags"]),
        (200, &tag_object(&tags)),
        "case before the delete: the tags must be listed, got {status} {body}"
    );

    let (status, body) = delete(
        &router,
        &format!("/greengrass/v2/components/{}", encoded_label(&arn)),
    )
    .await;
    assert_eq!(
        status, 204,
        "DeleteComponent must answer 204, got {status} {body}"
    );

    let (status, body) = list_tags(&router, &arn).await;
    assert_eq!(
        status, 404,
        "case after the delete: the tags of a deleted version must answer 404, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case after the delete: expected ResourceNotFoundException, got {body}"
    );
}

#[tokio::test]
async fn a_resource_arn_outside_the_modelled_pattern_is_refused() {
    let router = emulator();

    // `GenericV2ARN`'s pattern
    // (^arn:[^:]*:greengrass:[^:]*:(aws|[0-9]+):(components|deployments|coreDevices):)
    // is what the request must satisfy before any handler runs; each row breaches one
    // part of it.
    let rows = [
        ("no ARN at all", "not-an-arn".to_string()),
        (
            "another service's ARN",
            format!("arn:aws:iot:{REGION}:{ACCOUNT}:thing/PresentCore"),
        ),
        (
            "a Greengrass ARN whose resource field is another type",
            format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:things:PresentCore"),
        ),
        (
            "an account field that is neither `aws` nor digits",
            format!("arn:aws:greengrass:{REGION}:my-account:deployments:d-1"),
        ),
        (
            "a resource type with no identifier field after it",
            format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:deployments"),
        ),
    ];

    for (case, arn) in rows {
        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an ARN outside the modelled pattern must answer 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }
}

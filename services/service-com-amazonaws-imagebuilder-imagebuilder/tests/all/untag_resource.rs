// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UntagResource`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes. The resource ARN rides in the URI path, as the
//! modelled `@httpLabel` puts it, and the keys ride in the query string, as the modelled
//! `@httpQuery` list puts them: one `tagKeys` parameter per key.
//!
//! The rule under test: untagging takes away exactly the keys the request names from the
//! `tags` member of the ONE stored record its ARN names, whichever kind of resource that
//! is, leaving every other key of that record and every other record untouched; a key the
//! record does not carry is already absent, so naming it removes nothing and is served
//! rather than refused; and an ARN the calling Region holds no resource under names
//! nothing to untag. Every later read of the resource — the tag listing and the resource's
//! own read — reports exactly the tags the record then carries.
//!
//! The rows span what the rule generalises over: which resource kind the ARN names, how
//! the tags first got onto the record (a create or a tagging call), how many keys one call
//! names, what a key looks like, whether the key is carried or absent, and whether the
//! call empties the record's tags or leaves some behind.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for when it only needs one; the ARNs it creates carry it.
const REGION: &str = "us-west-2";

/// A second Region, for the case that checks a regional endpoint reaches only its own
/// Region's resources.
const OTHER_REGION: &str = "eu-west-1";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A build component document: one `build` phase, which is all `CreateComponent` needs to
/// store a component this suite can tag and untag.
const BUILD_DOCUMENT: &str = "name: HelloWorld\nschemaVersion: 1.0\nphases:\n  - name: build\n    steps:\n      - name: Greet\n        action: ExecuteBash\n        inputs:\n          commands:\n            - echo hello\n";

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

/// A request signed for `imagebuilder` in `region` — the signal the router routes on and
/// the handlers read the Region from.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{region}/imagebuilder/\
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
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// Percent-encode one path segment, as an AWS SDK encodes a NON-greedy `@httpLabel`:
/// everything outside the RFC 3986 unreserved set, so the `:` and `/` of an ARN travel as
/// `%3A` and `%2F` and the whole ARN stays ONE path segment.
fn encode_label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// `ListTagsForResource` for `arn`, in `region`.
async fn list_tags_in(router: &MultiServiceRouter, region: &str, arn: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/tags/{}", encode_label(arn)),
        None,
    )
    .await
}

/// `ListTagsForResource` for `arn`, in [`REGION`].
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    list_tags_in(router, REGION, arn).await
}

/// `TagResource` writing `tags` onto `arn`, in [`REGION`].
async fn tag_resource(
    router: &MultiServiceRouter,
    arn: &str,
    tags: &[(&str, &str)],
) -> (u16, Value) {
    call_in(
        router,
        REGION,
        "POST",
        &format!("/tags/{}", encode_label(arn)),
        Some(&json!({ "tags": tag_object(tags) })),
    )
    .await
}

/// `UntagResource` removing `keys` from `arn`, in `region`: the modelled `tagKeys` list is
/// an `@httpQuery` member, so each key travels as its own `tagKeys` parameter.
async fn untag_resource_in(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    keys: &[&str],
) -> (u16, Value) {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", encode_label(key)))
        .collect::<Vec<_>>()
        .join("&");
    call_in(
        router,
        region,
        "DELETE",
        &format!("/tags/{}?{query}", encode_label(arn)),
        None,
    )
    .await
}

/// `UntagResource` removing `keys` from `arn`, in [`REGION`].
async fn untag_resource(router: &MultiServiceRouter, arn: &str, keys: &[&str]) -> (u16, Value) {
    untag_resource_in(router, REGION, arn, keys).await
}

/// The pairs as the JSON object a `tags` member carries.
fn tag_object(tags: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (key, value) in tags {
        map.insert((*key).to_string(), json!(value));
    }
    Value::Object(map)
}

/// The pairs as the map an assertion compares against, so key order never matters.
fn tag_map(tags: &[(&str, &str)]) -> BTreeMap<String, String> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The `tags` a `ListTagsForResource` answer reports, as a map.
fn reported_tags(case: &str, body: &Value) -> BTreeMap<String, String> {
    let tags = body
        .get("tags")
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags member in {body}"));
    let object = tags
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected tags to be a map, got {tags}"));
    object
        .iter()
        .map(|(key, value)| {
            let text = value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected text for tag {key}, got {value}"))
                .to_string();
            (key.clone(), text)
        })
        .collect()
}

/// The error code a refusal carries, from `__type` (restJson1 renders the shape there,
/// optionally namespace-qualified).
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error shape, got {body}"))
        .rsplit(['#', '/'])
        .next()
        .expect("a non-empty error type")
        .to_string()
}

/// Assert `status` and the error code of a refusal.
fn assert_refused(case: &str, status: u16, body: &Value, expect_status: u16, expect_code: &str) {
    assert_eq!(
        status, expect_status,
        "case {case:?}: expected HTTP {expect_status}, got {status} with {body}"
    );
    let code = error_code(case, body);
    assert_eq!(
        code, expect_code,
        "case {case:?}: expected the error code {expect_code:?}, got {code:?}"
    );
}

/// Assert a call succeeded, and answer its body.
fn assert_ok(case: &str, status: u16, body: Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    body
}

/// The text one member of a response carries.
fn text(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected text in {name} of {body}"))
        .to_string()
}

/// The tags one resource carries, read through its own tag listing.
async fn tags_of(router: &MultiServiceRouter, case: &str, arn: &str) -> BTreeMap<String, String> {
    let (status, body) = list_tags(router, arn).await;
    let body = assert_ok(case, status, body);
    reported_tags(case, &body)
}

/// Which resource kind a row untags, and how a resource of that kind is created and read
/// back through its own Get.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    InfrastructureConfiguration,
    Component,
    ImageRecipe,
}

impl Kind {
    /// Create a resource of this kind in `region`, optionally carrying `tags`, and answer
    /// its ARN.
    async fn create(
        self,
        router: &MultiServiceRouter,
        region: &str,
        name: &str,
        tags: Option<&[(&str, &str)]>,
    ) -> String {
        let (uri, member) = match self {
            Self::InfrastructureConfiguration => (
                "/CreateInfrastructureConfiguration",
                "infrastructureConfigurationArn",
            ),
            Self::Component => ("/CreateComponent", "componentBuildVersionArn"),
            Self::ImageRecipe => ("/CreateImageRecipe", "imageRecipeArn"),
        };
        let mut body = Map::new();
        body.insert("name".to_string(), json!(name));
        body.insert("clientToken".to_string(), json!(format!("token-{name}")));
        match self {
            Self::InfrastructureConfiguration => {
                body.insert(
                    "instanceProfileName".to_string(),
                    json!("EC2InstanceProfileForImageBuilder"),
                );
            }
            Self::Component => {
                body.insert("semanticVersion".to_string(), json!("1.0.0"));
                body.insert("platform".to_string(), json!("Linux"));
                body.insert("data".to_string(), json!(BUILD_DOCUMENT));
            }
            Self::ImageRecipe => {
                body.insert("semanticVersion".to_string(), json!("1.0.0"));
                body.insert("parentImage".to_string(), json!("ami-0123456789abcdef0"));
            }
        }
        if let Some(tags) = tags {
            body.insert("tags".to_string(), tag_object(tags));
        }
        let case = format!("create the {self:?} {name}");
        let (status, answer) = call_in(router, region, "PUT", uri, Some(&Value::Object(body))).await;
        let answer = assert_ok(&case, status, answer);
        text(&case, &answer, member)
    }

    /// The tags this kind's OWN read reports for `arn` — the tags member of the resource
    /// the Get answers with.
    async fn tags_from_get(
        self,
        router: &MultiServiceRouter,
        case: &str,
        arn: &str,
    ) -> BTreeMap<String, String> {
        let (uri, member) = match self {
            Self::InfrastructureConfiguration => (
                format!("/GetInfrastructureConfiguration?infrastructureConfigurationArn={arn}"),
                "infrastructureConfiguration",
            ),
            Self::Component => (
                format!("/GetComponent?componentBuildVersionArn={arn}"),
                "component",
            ),
            Self::ImageRecipe => (
                format!("/GetImageRecipe?imageRecipeArn={arn}"),
                "imageRecipe",
            ),
        };
        let (status, body) = call_in(router, REGION, "GET", &uri, None).await;
        let body = assert_ok(case, status, body);
        let resource = body
            .get(member)
            .unwrap_or_else(|| panic!("case {case:?}: expected a {member} in {body}"));
        match resource.get("tags") {
            Some(_) => reported_tags(case, resource),
            None => BTreeMap::new(),
        }
    }
}

/// One row of the untagging table: a resource kind, the tags its create carried, the tags a
/// tagging call then wrote, the keys one untagging call names, and the tags the resource
/// carries afterwards.
struct Row {
    case: &'static str,
    kind: Kind,
    name: &'static str,
    created_with: Option<&'static [(&'static str, &'static str)]>,
    tagged_with: &'static [(&'static str, &'static str)],
    remove: &'static [&'static str],
    expected: &'static [(&'static str, &'static str)],
}

/// The untagging table. The rows differ along what the rule generalises over: the resource
/// kind, whether the tags started at the create or at a tagging call, how many keys one
/// call names, the shape of a key (the modelled `TagKey` pattern admits spaces, `.`, `:`,
/// `/`, `=`, `+`, `-` and `@`), whether a named key is one the resource carries, and
/// whether the call leaves tags behind or none at all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one of several keys removed from an infrastructure configuration",
            kind: Kind::InfrastructureConfiguration,
            name: "Untagged Infra",
            created_with: None,
            tagged_with: &[("Owner", "platform"), ("Stage", "dev")],
            remove: &["Stage"],
            expected: &[("Owner", "platform")],
        },
        Row {
            case: "several keys removed in one call, from tags a create carried",
            kind: Kind::Component,
            name: "Untagged Component",
            created_with: Some(&[("Source", "create"), ("Stage", "dev"), ("Team", "builders")]),
            tagged_with: &[("Owner", "platform")],
            remove: &["Stage", "Team", "Source"],
            expected: &[("Owner", "platform")],
        },
        Row {
            case: "every key removed leaves the resource carrying no tags",
            kind: Kind::ImageRecipe,
            name: "Emptied Recipe",
            created_with: Some(&[("Owner", "platform")]),
            tagged_with: &[("Stage", "prod")],
            remove: &["Owner", "Stage"],
            expected: &[],
        },
        Row {
            case: "a key the resource does not carry removes nothing",
            kind: Kind::InfrastructureConfiguration,
            name: "Absent Key Infra",
            created_with: Some(&[("Owner", "platform")]),
            tagged_with: &[("Stage", "prod")],
            remove: &["Absent"],
            expected: &[("Owner", "platform"), ("Stage", "prod")],
        },
        Row {
            case: "a carried key and an absent one named together",
            kind: Kind::Component,
            name: "Mixed Keys Component",
            created_with: None,
            tagged_with: &[("Owner", "platform"), ("Stage", "prod")],
            remove: &["Stage", "never-set"],
            expected: &[("Owner", "platform")],
        },
        Row {
            case: "keys across the modelled character set",
            kind: Kind::ImageRecipe,
            name: "Punctuated Recipe",
            created_with: None,
            tagged_with: &[
                ("cost center", "eng/platform"),
                ("app.version", "1.2.3+build"),
                ("contact@example", "a=b"),
                ("empty", ""),
            ],
            remove: &["cost center", "contact@example", "empty"],
            expected: &[("app.version", "1.2.3+build")],
        },
        Row {
            case: "no key of the resource is named, so every tag stays",
            kind: Kind::ImageRecipe,
            name: "Untouched Recipe",
            created_with: Some(&[("Owner", "platform"), ("Stage", "prod")]),
            tagged_with: &[],
            remove: &["Other", "Another"],
            expected: &[("Owner", "platform"), ("Stage", "prod")],
        },
    ]
}

#[tokio::test]
async fn untagging_removes_exactly_the_keys_the_request_names() {
    for row in rows() {
        let router = emulator();
        let arn = row
            .kind
            .create(&router, REGION, row.name, row.created_with)
            .await;
        if !row.tagged_with.is_empty() {
            let (status, body) = tag_resource(&router, &arn, row.tagged_with).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected the tagging call to answer HTTP 200, got {status} with \
                 {body}",
                row.case
            );
        }

        let (status, body) = untag_resource(&router, &arn, row.remove).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the untagging call to answer HTTP 200, got {status} with {body}",
            row.case
        );

        let expected = tag_map(row.expected);
        let actual = tags_of(&router, row.case, &arn).await;
        assert_eq!(
            actual, expected,
            "case {:?}: expected the tags {expected:?}, got {actual:?}",
            row.case
        );

        // The tags live on the resource's own record, so the resource's own read reports
        // the same map.
        let from_get = row.kind.tags_from_get(&router, row.case, &arn).await;
        assert_eq!(
            from_get, expected,
            "case {:?}: expected the resource's own read to report {expected:?}, got {from_get:?}",
            row.case
        );

        // Naming the same keys again removes nothing more: what is already absent stays
        // absent, and the keys the first call left are the keys the second call leaves.
        let (status, body) = untag_resource(&router, &arn, row.remove).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the repeated untagging call to answer HTTP 200, got {status} \
             with {body}",
            row.case
        );
        let repeated = tags_of(&router, row.case, &arn).await;
        assert_eq!(
            repeated, expected,
            "case {:?}: expected the repeated call to leave the tags at {expected:?}, got \
             {repeated:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_key_removed_can_be_written_again() {
    let router = emulator();
    let arn = Kind::ImageRecipe
        .create(&router, REGION, "Retagged Recipe", None)
        .await;

    let (status, body) = tag_resource(&router, &arn, &[("Stage", "dev")]).await;
    assert_ok("tag the recipe", status, body);

    let (status, body) = untag_resource(&router, &arn, &["Stage"]).await;
    assert_ok("remove the only key", status, body);
    let emptied = tags_of(&router, "remove the only key", &arn).await;
    assert!(
        emptied.is_empty(),
        "case \"remove the only key\": expected no tags, got {emptied:?}"
    );

    // A resource left carrying no tags is a resource that can be tagged again, and the
    // value it takes is the new one — not the value the removed key used to carry.
    let (status, body) = tag_resource(&router, &arn, &[("Stage", "prod")]).await;
    assert_ok("tag the recipe again", status, body);
    let expected = tag_map(&[("Stage", "prod")]);
    let actual = tags_of(&router, "tag the recipe again", &arn).await;
    assert_eq!(
        actual, expected,
        "case \"tag the recipe again\": expected the tags {expected:?}, got {actual:?}"
    );
}

#[tokio::test]
async fn untagging_one_resource_leaves_every_other_resource_alone() {
    let router = emulator();
    let shared = &[("Owner", "platform"), ("Stage", "dev")];
    let kinds = [
        (Kind::InfrastructureConfiguration, "Shared Infra"),
        (Kind::Component, "Shared Component"),
        (Kind::ImageRecipe, "Shared Recipe"),
    ];
    let mut arns = Vec::new();
    for (kind, name) in kinds {
        arns.push((kind, name, kind.create(&router, REGION, name, Some(shared)).await));
    }

    // Untag exactly one of them; the other records name the same keys and must keep them.
    let (status, body) = untag_resource(&router, &arns[1].2, &["Stage"]).await;
    assert_ok("untag only the component", status, body);

    for (index, (kind, name, arn)) in arns.iter().enumerate() {
        let case = format!("{name} after untagging only the component");
        let expected = if index == 1 {
            tag_map(&[("Owner", "platform")])
        } else {
            tag_map(shared)
        };
        let actual = tags_of(&router, &case, arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the tags {expected:?}, got {actual:?}"
        );
        let from_get = kind.tags_from_get(&router, &case, arn).await;
        assert_eq!(
            from_get, expected,
            "case {case:?}: expected the resource's own read to report {expected:?}, got \
             {from_get:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_this_region_holds_no_resource_under_is_reported_missing() {
    let router = emulator();
    // One resource of each stored kind exists, so a refusal below is about the ARN in the
    // request and not about an empty store.
    let configuration = Kind::InfrastructureConfiguration
        .create(&router, REGION, "Present Infra", Some(&[("Owner", "platform")]))
        .await;
    Kind::Component
        .create(&router, REGION, "Present Component", None)
        .await;
    Kind::ImageRecipe
        .create(&router, REGION, "Present Recipe", None)
        .await;

    // Each row is a well-formed `ImageBuilderArn` the calling Region holds nothing under:
    // a never-created resource of a kind this store holds, a resource kind it holds none
    // of at all (the modelled `ImageBuilderArn` pattern admits ten), and a resource that
    // exists in ANOTHER Region.
    let cases: Vec<(String, String)> = vec![
        (
            "a never-created infrastructure configuration".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:infrastructure-configuration/absent"),
        ),
        (
            "a never-created component build version".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/absent/1.0.0/1"),
        ),
        (
            "another build version of a component that exists".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/present-component/1.0.0/2"),
        ),
        (
            "a never-created image recipe".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image-recipe/absent/1.0.0"),
        ),
        (
            "an image pipeline, a kind this store holds none of".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image-pipeline/absent"),
        ),
        (
            "a distribution configuration, a kind this store holds none of".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:distribution-configuration/absent"),
        ),
        (
            "a build workflow, a kind this store holds none of".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:workflow/build/absent/1.0.0/1"),
        ),
        (
            "a configuration of another Region".to_string(),
            configuration.replace(REGION, OTHER_REGION),
        ),
    ];

    for (case, arn) in &cases {
        let (status, body) = untag_resource(&router, arn, &["Owner"]).await;
        assert_refused(case, status, &body, 404, "ResourceNotFoundException");
    }

    // The configuration this Region holds is not reachable from the other Region's
    // endpoint either — the refusal is about the Region, not about the ARN's text — and a
    // refused call removes nothing, so its tags are the ones it was created with.
    let (status, body) = untag_resource_in(&router, OTHER_REGION, &configuration, &["Owner"]).await;
    assert_refused(
        "a configuration untagged from another Region",
        status,
        &body,
        404,
        "ResourceNotFoundException",
    );
    let expected = tag_map(&[("Owner", "platform")]);
    let actual = tags_of(&router, "the configuration after every refusal", &configuration).await;
    assert_eq!(
        actual, expected,
        "case \"the configuration after every refusal\": expected the tags {expected:?}, got \
         {actual:?}"
    );
}

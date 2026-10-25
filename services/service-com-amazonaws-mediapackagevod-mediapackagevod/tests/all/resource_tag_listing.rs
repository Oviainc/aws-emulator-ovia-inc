// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource` reports what the tag store holds for the resource its
//! `ResourceArn` names.
//!
//! The rule under test, stated once: `ListTagsForResource` answers the tag map
//! persisted for the resource its `ResourceArn` names — the SAME map the describes and
//! the listings of that resource report, for every resource type of this service and
//! whatever tags it was created with — reading it without changing anything; and a
//! `ResourceArn` that names no resource this service holds in the Region the request is
//! served in is answered `NotFoundException` (HTTP 404) rather than with an empty map.
//!
//! Each test is a TABLE: the rows span the three resource types this service's ARNs
//! name (`smithy_query(query="digest")` — PackagingGroup, PackagingConfiguration,
//! Asset), the shapes a tag map takes (absent, empty, one entry, several, non-ASCII
//! keys and values), and the ways an ARN can name nothing (an id never created, another
//! Region's, another account's, a resource type this service has none of, another
//! service's ARN, a value that is not an ARN, an ARN carrying no id, and a resource
//! that was deleted). So what is pinned is the rule, not one recorded call.
//!
//! Every case drives real HTTP through the router the production binary builds.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mediapackagevod_mediapackagevod::state as vod;

/// The Region every case is signed for unless it is varying the Region.
pub(crate) const REGION: &str = "us-west-2";

/// A second, distinct Region, for the rows about an ARN of another Region.
pub(crate) const OTHER_REGION: &str = "eu-central-1";

/// The PackagingGroup a packaging configuration and an asset are created under: both
/// need a parent group that exists.
pub(crate) const PARENT_GROUP: &str = "vod-parent-group";

/// A well-formed S3 object ARN, for `CreateAsset$SourceArn`.
pub(crate) const SOURCE_ARN: &str = "arn:aws:s3:::test-bucket/asset.smil";

/// The modelled refusal of a resource this service does not hold
/// (`smithy_query(query="shape",shape="NotFoundException")` — `httpError 404`).
const NOT_FOUND: (u16, &str) = (404, "NotFoundException");

/// What one request answered: the HTTP status, restJson1's `x-amzn-errortype`, and the
/// parsed body.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) errortype: String,
    pub(crate) body: Value,
}

/// One resource type of this service, as the tag rows address it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Kind {
    /// What a failing case names this resource type by.
    pub(crate) name: &'static str,
    /// The resource-type segment this resource's ARN carries.
    pub(crate) arn_kind: &'static str,
    /// The collection route its create posts to.
    pub(crate) create_route: &'static str,
    /// The status this service answers that create with.
    pub(crate) created_status: u16,
}

/// The three resource types an ARN of this service names. `CreatePackagingConfiguration`
/// is the one create this service answers 202 rather than 200 with.
pub(crate) const KINDS: [Kind; 3] = [
    Kind {
        name: "PackagingGroup",
        arn_kind: vod::ARN_KIND_PACKAGING_GROUPS,
        create_route: "/packaging_groups",
        created_status: 200,
    },
    Kind {
        name: "PackagingConfiguration",
        arn_kind: vod::ARN_KIND_PACKAGING_CONFIGURATIONS,
        create_route: "/packaging_configurations",
        created_status: 202,
    },
    Kind {
        name: "Asset",
        arn_kind: vod::ARN_KIND_ASSETS,
        create_route: "/assets",
        created_status: 200,
    },
];

impl Kind {
    /// The route that describes the resource `id` of this type.
    pub(crate) fn describe_route(self, id: &str) -> String {
        match self.arn_kind {
            vod::ARN_KIND_PACKAGING_GROUPS => format!("/packaging_groups/{id}"),
            vod::ARN_KIND_PACKAGING_CONFIGURATIONS => format!("/packaging_configurations/{id}"),
            _ => format!("/assets/{id}"),
        }
    }

    /// A create body for `id` of this type, carrying `tags` when the row supplies a
    /// tag map (a row that supplies none omits the member entirely).
    pub(crate) fn create_body(self, id: &str, tags: Option<&Value>) -> Value {
        let role_arn = format!(
            "arn:aws:iam::{}:role/MediaPackageVod",
            engine::emulator_account_id()
        );
        let mut body = match self.arn_kind {
            vod::ARN_KIND_PACKAGING_GROUPS => json!({ "id": id }),
            vod::ARN_KIND_PACKAGING_CONFIGURATIONS => json!({
                "id": id,
                "packagingGroupId": PARENT_GROUP,
                "hlsPackage": { "hlsManifests": [{}] },
            }),
            _ => json!({
                "id": id,
                "packagingGroupId": PARENT_GROUP,
                "sourceArn": SOURCE_ARN,
                "sourceRoleArn": role_arn,
            }),
        };
        if let Some(tags) = tags {
            body["tags"] = tags.clone();
        }
        body
    }
}

/// A router carrying exactly this service over one shared engine — the same assembly
/// `core/router` performs, minus the other services.
pub(crate) fn router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_mediapackagevod_mediapackagevod::EmulatedService::new(Arc::new(
            engine::InMemoryEngine::new(),
        )),
    );
    router
}

/// Send one request signed for `region` and report what it answered. Signature
/// verification is off for this router, so only the credential SCOPE matters — that is
/// what the handlers read the Region from.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let response = dispatch(router, region, method, path, body).await;
    let status = response.status().as_u16();
    let errortype = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| "<unset>".to_string());
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        errortype,
        body,
    }
}

/// The same round trip as [`call`], with the WHOLE response handed back — status, headers
/// and the body as bytes — for a case that asserts something about the response a parsed
/// [`Answer`] does not carry, such as how an empty body is framed.
pub(crate) async fn dispatch(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> http::Response<bytes::Bytes> {
    let host = format!("mediapackage-vod.{region}.amazonaws.com");
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host.clone())
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{region}\
                 /mediapackage-vod/aws4_request, SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");

    router.dispatch(request).await
}

/// The `GET /tags/{ResourceArn}` route for `arn`.
///
/// `ResourceArn` is a non-greedy `@httpLabel`, so it occupies ONE path segment and the
/// `/` inside an ARN is sent percent-encoded, exactly as an AWS SDK sends it.
pub(crate) fn tags_route(arn: &str) -> String {
    format!("/tags/{}", arn.replace('/', "%2F"))
}

/// The `tags` member of a response body, as a map. A body that carries none reads as
/// no tags, so a missing member and an empty map compare equal here.
pub(crate) fn tag_map(body: &Value) -> BTreeMap<String, String> {
    body.get("tags")
        .and_then(Value::as_object)
        .map(|tags| {
            tags.iter()
                .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.into())))
                .collect()
        })
        .unwrap_or_default()
}

/// A tag map as the request sent it, for comparing against what was reported.
pub(crate) fn expected_map(tags: Option<&Value>) -> BTreeMap<String, String> {
    tags.and_then(Value::as_object)
        .map(|tags| {
            tags.iter()
                .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.into())))
                .collect()
        })
        .unwrap_or_default()
}

/// The tag maps the rows span: the member absent altogether, an empty map, one entry,
/// several entries, keys and values carrying spaces and punctuation, and non-ASCII
/// text. `Tags` is a `__mapOf__string` with no declared constraints
/// (`smithy_query(query="shape",shape="ListTagsForResourceResponse")`), so every one of
/// these is a tag map the service accepts.
fn tag_rows() -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("no tags member at all", None),
        ("an empty tag map", Some(json!({}))),
        ("one tag", Some(json!({ "environment": "production" }))),
        (
            "several tags",
            Some(json!({ "environment": "staging", "team": "video", "tier": "gold" })),
        ),
        (
            "keys and values carrying spaces and punctuation",
            Some(json!({ "Cost Center": "video / vod", "note": "renewed, 2026-01-01" })),
        ),
        (
            "non-ASCII keys and values",
            Some(json!({ "região": "são-paulo", "プロジェクト": "配信" })),
        ),
    ]
}

/// Create the parent PackagingGroup every configuration and asset row needs.
pub(crate) async fn create_parent_group(router: &MultiServiceRouter, region: &str) {
    let answer = call(
        router,
        region,
        "POST",
        "/packaging_groups",
        Some(json!({ "id": PARENT_GROUP })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "parent group in {region:?}: expected 200, got {} body {}",
        answer.status, answer.body
    );
}

/// Create the resource `id` of `kind` carrying `tags` and report the ARN the create
/// answered with.
pub(crate) async fn create_with_tags(
    router: &MultiServiceRouter,
    region: &str,
    kind: Kind,
    id: &str,
    tags: Option<&Value>,
    case: &str,
) -> String {
    let answer = call(
        router,
        region,
        "POST",
        kind.create_route,
        Some(kind.create_body(id, tags)),
    )
    .await;
    assert_eq!(
        answer.status,
        kind.created_status,
        "case {case:?}: Create{} expected {}, got {} body {}",
        kind.name,
        kind.created_status,
        answer.status,
        answer.body
    );
    answer.body["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: Create{} reported no Arn", kind.name))
        .to_string()
}

/// Whatever tags a resource was created with are exactly what `ListTagsForResource`
/// reports for it, for every resource type and every shape of tag map — and the map is
/// the same one the describe of that resource reports, so both reads answer the one
/// stored copy.
#[tokio::test]
async fn reported_tags_are_the_tags_the_resource_was_created_with() {
    let router = router();
    create_parent_group(&router, REGION).await;

    for kind in KINDS {
        for (index, (row, tags)) in tag_rows().into_iter().enumerate() {
            let case = format!("{}/{row}", kind.name);
            let id = format!("{}-{index}", kind.arn_kind);
            let arn = create_with_tags(&router, REGION, kind, &id, tags.as_ref(), &case).await;
            assert_eq!(
                arn,
                vod::resource_arn(kind.arn_kind, REGION, &id),
                "case {case:?}: the create's Arn must be this Region's ARN of that resource"
            );

            let listed = call(&router, REGION, "GET", &tags_route(&arn), None).await;
            assert_eq!(
                listed.status, 200,
                "case {case:?}: ListTagsForResource expected 200, got {} body {}",
                listed.status, listed.body
            );
            let expected = expected_map(tags.as_ref());
            // The member is always PRESENT, even for a resource carrying no tags: an
            // absent `tags` and `{}` are different bodies, and the modelled output
            // member is what a client reads the map from.
            assert!(
                listed.body.get("tags").and_then(Value::as_object).is_some(),
                "case {case:?}: the response must carry a tags member, got {}",
                listed.body
            );
            assert_eq!(
                tag_map(&listed.body),
                expected,
                "case {case:?}: expected the created tags {expected:?}, got {}",
                listed.body
            );

            // The same stored copy the resource's own read reports.
            let described = call(&router, REGION, "GET", &kind.describe_route(&id), None).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: Describe{} expected 200, got {} body {}",
                kind.name, described.status, described.body
            );
            assert_eq!(
                tag_map(&described.body),
                expected,
                "case {case:?}: Describe{} must report the same tags {expected:?}, got {}",
                kind.name,
                described.body
            );
        }
    }
}

/// Listing a resource's tags changes nothing: repeated reads of the same resource
/// report the same map, and the resource and its tags are still there afterwards.
#[tokio::test]
async fn listing_tags_leaves_the_resource_and_its_tags_untouched() {
    let router = router();
    create_parent_group(&router, REGION).await;
    let tags = json!({ "environment": "production", "team": "video" });

    for kind in KINDS {
        let case = format!("{}/repeated reads", kind.name);
        let id = format!("{}-stable", kind.arn_kind);
        let arn = create_with_tags(&router, REGION, kind, &id, Some(&tags), &case).await;
        let expected = expected_map(Some(&tags));

        for read in 1..=3 {
            let listed = call(&router, REGION, "GET", &tags_route(&arn), None).await;
            assert_eq!(
                listed.status, 200,
                "case {case:?} read {read}: expected 200, got {} body {}",
                listed.status, listed.body
            );
            assert_eq!(
                tag_map(&listed.body),
                expected,
                "case {case:?} read {read}: expected {expected:?}, got {}",
                listed.body
            );
        }

        let described = call(&router, REGION, "GET", &kind.describe_route(&id), None).await;
        assert_eq!(
            (described.status, tag_map(&described.body)),
            (200, expected.clone()),
            "case {case:?}: the resource must be unchanged by the reads, got {} body {}",
            described.status,
            described.body
        );
    }
}

/// A `ResourceArn` that names no resource this service holds in the Region the request
/// is served in is answered `NotFoundException` (404) — never an empty tag map, which
/// would claim the resource exists and carries no tags.
#[tokio::test]
async fn an_arn_naming_no_resource_of_this_region_is_not_found() {
    let router = router();
    create_parent_group(&router, REGION).await;
    let account = engine::emulator_account_id();
    let held = create_with_tags(
        &router,
        REGION,
        KINDS[0],
        "vod-held-group",
        Some(&json!({ "environment": "production" })),
        "the resource the misses are compared against",
    )
    .await;

    // A resource of the SAME id exists in another Region, so a cross-Region read that
    // reported tags would be reporting that other Region's resource.
    create_parent_group(&router, OTHER_REGION).await;

    let mut rows: Vec<(&str, String)> = vec![
        (
            "an id this service never created",
            vod::resource_arn(vod::ARN_KIND_ASSETS, REGION, "never-created"),
        ),
        (
            "another Region's resource of the same id",
            vod::resource_arn(vod::ARN_KIND_PACKAGING_GROUPS, OTHER_REGION, PARENT_GROUP),
        ),
        (
            "another account's ARN of a held resource",
            held.replace(account, "999999999999"),
        ),
        (
            "another partition's ARN of a held resource",
            held.replacen("arn:aws:", "arn:aws-cn:", 1),
        ),
        (
            "a resource type this service has none of",
            format!("arn:aws:mediapackage-vod:{REGION}:{account}:channels/anything"),
        ),
        (
            "an ARN of another service",
            SOURCE_ARN.to_string(),
        ),
        ("a value that is not an ARN at all", "not-an-arn".to_string()),
        (
            "an ARN carrying no id",
            format!("arn:aws:mediapackage-vod:{REGION}:{account}:assets/"),
        ),
    ];

    // A deleted resource is a miss too: its tags are forgotten with it.
    let doomed = create_with_tags(
        &router,
        REGION,
        KINDS[0],
        "vod-doomed-group",
        Some(&json!({ "environment": "scratch" })),
        "a resource that is then deleted",
    )
    .await;
    let deleted = call(
        &router,
        REGION,
        "DELETE",
        "/packaging_groups/vod-doomed-group",
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 204,
        "DeletePackagingGroup: expected 204, got {} body {}",
        deleted.status, deleted.body
    );
    rows.push(("a resource that was deleted", doomed));

    for (case, arn) in rows {
        let answer = call(&router, REGION, "GET", &tags_route(&arn), None).await;
        assert_eq!(
            (answer.status, answer.errortype.as_str()),
            NOT_FOUND,
            "case {case:?} ({arn:?}): expected {NOT_FOUND:?}, got {} / {:?} body {}",
            answer.status,
            answer.errortype,
            answer.body
        );
        assert!(
            answer
                .body
                .get("message")
                .and_then(Value::as_str)
                .is_some_and(|message| message.contains("not found")),
            "case {case:?}: the refusal must say the resource was not found, got {}",
            answer.body
        );
    }

    // The resource that IS held still reports its tags after every miss.
    let listed = call(&router, REGION, "GET", &tags_route(&held), None).await;
    assert_eq!(
        (listed.status, tag_map(&listed.body)),
        (
            200,
            BTreeMap::from([("environment".to_string(), "production".to_string())])
        ),
        "the held resource must still report its tags, got {} body {}",
        listed.status,
        listed.body
    );
}

/// The same `Id` tagged differently in two Regions reports each Region's own tags: a
/// tag read resolves only the resources of the Region it is served in.
#[tokio::test]
async fn each_region_reports_its_own_resources_tags() {
    let router = router();
    let per_region = [
        (REGION, json!({ "environment": "production", "home": REGION })),
        (
            OTHER_REGION,
            json!({ "environment": "staging", "home": OTHER_REGION }),
        ),
    ];

    for (region, tags) in &per_region {
        let case = format!("{region}/own tags");
        create_with_tags(
            &router,
            region,
            KINDS[0],
            PARENT_GROUP,
            Some(tags),
            &case,
        )
        .await;
    }

    for (region, tags) in &per_region {
        let arn = vod::resource_arn(vod::ARN_KIND_PACKAGING_GROUPS, region, PARENT_GROUP);
        let listed = call(&router, region, "GET", &tags_route(&arn), None).await;
        assert_eq!(
            listed.status, 200,
            "case {region:?}: expected 200, got {} body {}",
            listed.status, listed.body
        );
        let expected = expected_map(Some(tags));
        assert_eq!(
            tag_map(&listed.body),
            expected,
            "case {region:?}: expected that Region's own tags {expected:?}, got {}",
            listed.body
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListPackagingConfigurations`.
//!
//! Every test drives real HTTP requests through the router the production binary
//! builds, so what it asserts is the wire behaviour a client sees: restJson1 routes,
//! status codes and JSON bodies. Each test owns its own engine, so no test observes
//! another's state.
//!
//! The tests assert RULES over a SET of inputs rather than one recorded call:
//!
//! * paging with any page size reports every stored configuration exactly once, in
//!   one stable order — no duplicate, no omission;
//! * `packagingGroupId` restricts the listing to that group's configurations;
//! * each listed entry is the record the create persisted, package block and tags
//!   included;
//! * `MaxResults` is accepted on each modelled bound and refused one past it, and a
//!   `NextToken` this listing did not issue is refused;
//! * the listing is reporting-only: reading it twice reports the same thing.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for. It appears in every ARN this listing reports, so
/// it is asserted rather than assumed.
const REGION: &str = "us-west-2";

/// A router carrying exactly this service over one shared engine — the same
/// assembly `core/router` performs, minus the other services.
fn router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_mediapackagevod_mediapackagevod::EmulatedService::new(Arc::new(
            engine::InMemoryEngine::new(),
        )),
    );
    router
}

/// Send one request and return its status and parsed JSON body (`Null` when empty).
///
/// The `Authorization` header carries only a SigV4 credential SCOPE: that is what the
/// router routes on and what the handlers read the region from. Signature
/// verification is off for this router, so the signature itself is never checked.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let host = format!("mediapackage-vod.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host.clone())
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}\
                 /mediapackage-vod/aws4_request, SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Create a packaging group, asserting it succeeded.
async fn create_group(router: &MultiServiceRouter, id: &str) {
    let (status, group) = call(router, "POST", "/packaging_groups", Some(json!({ "id": id }))).await;
    assert_eq!(status, 200, "CreatePackagingGroup {id}: body {group}");
}

/// Create a packaging configuration from a package block, asserting it succeeded and
/// returning the create's own response — the record every later read must match.
async fn create_config(
    router: &MultiServiceRouter,
    id: &str,
    group_id: &str,
    package: Value,
    tags: Option<Value>,
) -> Value {
    let mut body = json!({ "id": id, "packagingGroupId": group_id });
    let Value::Object(members) = package else {
        panic!("the package block must be a JSON object");
    };
    for (key, value) in members {
        body[key] = value;
    }
    if let Some(tags) = tags {
        body["tags"] = tags;
    }
    let (status, config) = call(router, "POST", "/packaging_configurations", Some(body)).await;
    // 202 Accepted: the status the recorded wire responses show for a successful
    // create, which `src/ops/create_packaging_configuration.rs` explains (the model
    // declares 200).
    assert_eq!(status, 202, "CreatePackagingConfiguration {id}: body {config}");
    config
}

/// The simplest valid package block, for a case that is not about the format.
fn hls_package() -> Value {
    json!({ "hlsPackage": { "hlsManifests": [{}] } })
}

/// One page of `ListPackagingConfigurations`: the ids it reported and the cursor it
/// handed back, asserting the call succeeded and respected `maxResults`.
async fn page(
    router: &MultiServiceRouter,
    query: &str,
    expect_at_most: usize,
) -> (Vec<String>, Option<String>) {
    let (status, body) = call(router, "GET", &format!("/packaging_configurations?{query}"), None)
        .await;
    assert_eq!(status, 200, "case {query:?}: body {body}");
    let entries = body["packagingConfigurations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {query:?}: expected a packagingConfigurations list, got {body}"))
        .clone();
    assert!(
        entries.len() <= expect_at_most,
        "case {query:?}: a page must hold at most {expect_at_most} entries, got {}",
        entries.len()
    );
    let ids = entries
        .iter()
        .map(|entry| entry["id"].as_str().unwrap_or_default().to_string())
        .collect();
    let token = body["nextToken"].as_str().map(str::to_string);
    (ids, token)
}

/// Every id the listing reports for `query`, following `nextToken` to the end.
///
/// `page_size` is the `maxResults` each request asks for, so a page longer than the
/// caller allowed fails the test rather than being silently accepted.
async fn list_all_ids(
    router: &MultiServiceRouter,
    query: &str,
    page_size: Option<usize>,
) -> Vec<String> {
    let mut ids = Vec::new();
    let mut token: Option<String> = None;
    // One more iteration than there can be pages of one configuration each, so a
    // cursor that fails to advance fails the test instead of looping forever.
    for _ in 0..256 {
        let mut request = query.to_string();
        if let Some(size) = page_size {
            request.push_str(&format!("&maxResults={size}"));
        }
        if let Some(token) = &token {
            request.push_str(&format!("&nextToken={token}"));
        }
        let (page_ids, next) = page(router, &request, page_size.unwrap_or(1000)).await;
        ids.extend(page_ids);
        match next {
            Some(next) => token = Some(next),
            None => return ids,
        }
    }
    panic!("the listing never stopped returning a nextToken for query {query:?}");
}

/// Paging reports every stored configuration exactly once, in one stable ascending
/// `Id` order, whatever the page size — and a cursor resumes strictly after the id
/// the previous page ended on.
#[tokio::test]
async fn the_listing_pages_every_configuration_exactly_once() {
    // (configurations stored, maxResults per page)
    let cases: [(usize, Option<usize>); 8] = [
        (0, Some(1)),
        (1, Some(1)),
        (3, Some(1)),
        (3, Some(2)),
        (5, Some(4)),
        (4, Some(1000)),
        (7, Some(3)),
        (6, None),
    ];
    for (total, page_size) in cases {
        let case = format!("total={total}, maxResults={page_size:?}");
        let router = router();
        create_group(&router, "g").await;
        // Ids that do NOT sort the way they were created, so an order that merely
        // echoes insertion order fails this test.
        let mut expected: Vec<String> = Vec::new();
        for index in 0..total {
            let id = format!("cfg-{}", (total - index) * 7 % 97);
            create_config(&router, &id, "g", hls_package(), None).await;
            expected.push(id);
        }
        expected.sort();

        let reported = list_all_ids(&router, "", page_size).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: the listing must report every configuration exactly once in \
             ascending id order, got {reported:?}"
        );
        let unique: BTreeSet<&String> = reported.iter().collect();
        assert_eq!(
            unique.len(),
            reported.len(),
            "case {case:?}: no configuration may be reported twice, got {reported:?}"
        );
    }
}

/// `packagingGroupId` restricts the listing to the configurations of THAT group, and
/// a group holding none — stored or not — reports an empty list with no cursor.
#[tokio::test]
async fn the_listing_is_restricted_to_one_packaging_group() {
    // (group id, how many configurations it holds)
    let groups: [(&str, usize); 4] = [("alpha", 3), ("beta", 1), ("gamma", 0), ("delta", 5)];
    let router = router();
    let mut all: Vec<String> = Vec::new();
    for (group, count) in groups {
        create_group(&router, group).await;
        for index in 0..count {
            let id = format!("{group}-cfg-{index}");
            create_config(&router, &id, group, hls_package(), None).await;
            all.push(id);
        }
    }
    all.sort();

    for (group, count) in groups {
        let mut expected: Vec<String> = all
            .iter()
            .filter(|id| id.starts_with(&format!("{group}-cfg-")))
            .cloned()
            .collect();
        expected.sort();
        assert_eq!(
            expected.len(),
            count,
            "case {group:?}: the test expects {count} configurations in that group"
        );
        let reported = list_all_ids(&router, &format!("packagingGroupId={group}"), Some(2)).await;
        assert_eq!(
            reported, expected,
            "case {group:?}: the listing must report only that group's configurations, \
             got {reported:?}"
        );
    }

    // An unfiltered listing reports every group's configurations.
    let reported = list_all_ids(&router, "", Some(3)).await;
    assert_eq!(
        reported, all,
        "an unfiltered listing must report every stored configuration, got {reported:?}"
    );

    // A group nothing is stored under is an empty listing, not a refusal: the filter
    // selects records, it does not address a resource.
    let (ids, token) = page(&router, "packagingGroupId=no-such-group", 1000).await;
    assert!(
        ids.is_empty() && token.is_none(),
        "an unmatched filter must report an empty page with no cursor, got {ids:?} / {token:?}"
    );
}

/// Each listed entry is the record the create persisted: same ARN, `CreatedAt`, group
/// id, tags and package block, whichever packaging format it was created with.
#[tokio::test]
async fn each_listed_configuration_is_the_stored_record() {
    // (case, package block) — one row per packaging format the model declares
    // (`PackagingConfiguration`'s CmafPackage / DashPackage / HlsPackage / MssPackage).
    let cases: Vec<(&str, Value)> = vec![
        ("hls", json!({ "hlsPackage": { "hlsManifests": [{ "manifestName": "primary" }] } })),
        ("dash", json!({ "dashPackage": { "dashManifests": [{ "manifestName": "wide" }] } })),
        ("cmaf", json!({ "cmafPackage": { "hlsManifests": [{}] } })),
        ("mss", json!({ "mssPackage": { "mssManifests": [{ "manifestName": "smooth" }] } })),
    ];

    let router = router();
    create_group(&router, "g").await;
    let mut created: Vec<(&str, String, Value)> = Vec::new();
    for (case, package) in cases {
        let id = format!("cfg-{case}");
        let config = create_config(
            &router,
            &id,
            "g",
            package,
            Some(json!({ "format": case, "env": "test" })),
        )
        .await;
        created.push((case, id, config));
    }

    let (status, body) = call(&router, "GET", "/packaging_configurations", None).await;
    assert_eq!(status, 200, "the listing: body {body}");
    let entries = body["packagingConfigurations"]
        .as_array()
        .expect("a packagingConfigurations list")
        .clone();
    assert_eq!(
        entries.len(),
        created.len(),
        "the listing must report every created configuration, got {body}"
    );

    for (case, id, config) in &created {
        let listed = entries
            .iter()
            .find(|entry| entry["id"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: {id} is missing from the listing: {body}"));
        for member in [
            "arn",
            "createdAt",
            "id",
            "packagingGroupId",
            "tags",
            "hlsPackage",
            "dashPackage",
            "cmafPackage",
            "mssPackage",
        ] {
            assert_eq!(
                listed[member], config[member],
                "case {case:?}: the listed {member} must be what the create persisted, \
                 got {}",
                listed[member]
            );
        }
        assert_eq!(
            listed["arn"].as_str(),
            Some(
                format!(
                    "arn:aws:mediapackage-vod:{REGION}:{}:packaging-configurations/{id}",
                    engine::emulator_account_id()
                )
                .as_str()
            ),
            "case {case:?}: the listed ARN must name this request's region and account, got {}",
            listed["arn"]
        );
    }

    // Reporting only: reading the listing again reports exactly the same thing, so
    // the read changed nothing a later caller sees.
    let (status, again) = call(&router, "GET", "/packaging_configurations", None).await;
    assert_eq!(status, 200, "the second listing: body {again}");
    assert_eq!(
        again, body,
        "ListPackagingConfigurations is reporting-only: a second read must report the same, \
         got {again}"
    );
}

/// `MaxResults` is accepted on each bound the model publishes (`range(min: 1, max:
/// 1000)`) and refused one past either; a `NextToken` this listing did not issue —
/// forged, or issued by the asset listing — is refused as unprocessable.
#[tokio::test]
async fn max_results_and_next_token_follow_their_modelled_constraints() {
    let router = router();
    create_group(&router, "g").await;
    for id in ["cfg-a", "cfg-b", "cfg-c"] {
        create_config(&router, id, "g", hls_package(), None).await;
    }

    // Each published bound of `MaxResults`, and one value in between.
    for accepted in [1, 2, 1000] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/packaging_configurations?maxResults={accepted}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "maxResults={accepted} must be accepted, body {body}");
        let returned = body["packagingConfigurations"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert!(
            returned <= accepted.min(3),
            "maxResults={accepted}: a page must hold at most that many entries, got {returned}"
        );
    }

    // One past each bound.
    for refused in [0, 1001] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/packaging_configurations?maxResults={refused}"),
            None,
        )
        .await;
        assert!(
            (400..500).contains(&status),
            "maxResults={refused} is outside the modelled range and must be refused, \
             got {status}: body {body}"
        );
    }

    // A cursor the ASSET listing issued, so the rows below can check a token is
    // bound to the listing that minted it. Two assets, so that listing hands one out.
    for id in ["asset-a", "asset-b"] {
        let (status, asset) = call(
            &router,
            "POST",
            "/assets",
            Some(json!({
                "id": id,
                "packagingGroupId": "g",
                "sourceArn": format!("arn:aws:s3:::test-bucket/{id}.smil"),
                "sourceRoleArn": format!(
                    "arn:aws:iam::{}:role/MediaPackageVod",
                    engine::emulator_account_id()
                ),
            })),
        )
        .await;
        assert_eq!(status, 200, "CreateAsset {id}: body {asset}");
    }
    let (status, asset_page) = call(&router, "GET", "/assets?maxResults=1", None).await;
    assert_eq!(status, 200, "ListAssets: body {asset_page}");
    let asset_token = asset_page["nextToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !asset_token.is_empty(),
        "with two assets the asset listing must issue a cursor: {asset_page}"
    );
    let (status, first) = call(
        &router,
        "GET",
        "/packaging_configurations?maxResults=1",
        None,
    )
    .await;
    assert_eq!(status, 200, "the first page: body {first}");
    let issued = first["nextToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !issued.is_empty(),
        "with more configurations than one page the listing must issue a cursor: {first}"
    );

    // (case, nextToken) — every cursor this listing did not issue is refused,
    // including one another listing of this same service issued.
    let forged: Vec<(&str, String)> = vec![
        ("empty", String::new()),
        ("not hex", "not-a-cursor".to_string()),
        ("checksum only", issued[..8].to_string()),
        ("payload only", issued[8..].to_string()),
        ("flipped checksum", format!("ffffffff{}", &issued[8..])),
        ("non-ascii", "🙈🙈🙈🙈🙈".to_string()),
        ("a cursor the asset listing issued", asset_token),
    ];
    for (case, token) in forged {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/packaging_configurations?nextToken={token}"),
            None,
        )
        .await;
        assert_eq!(
            status, 422,
            "case {case:?}: a NextToken this listing never issued must be refused as \
             unprocessable, got {status}: body {body}"
        );
        assert_eq!(
            body["errors"],
            json!({ "nextToken": ["is not a cursor this service issued"] }),
            "case {case:?}: expected the violation attributed to `nextToken`, got {body}"
        );
    }

    // ... and the cursor THIS listing issued resumes it, strictly after the id the
    // first page ended on.
    let (status, second) = call(
        &router,
        "GET",
        &format!("/packaging_configurations?maxResults=1&nextToken={issued}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "the issued cursor must be accepted: body {second}");
    assert_eq!(
        second["packagingConfigurations"][0]["id"].as_str(),
        Some("cfg-b"),
        "the second page must resume strictly after the first page's last id, got {second}"
    );
}

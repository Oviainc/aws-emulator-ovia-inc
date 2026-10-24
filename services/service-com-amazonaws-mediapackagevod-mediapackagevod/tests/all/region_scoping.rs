// SPDX-License-Identifier: CC0-1.0

//! MediaPackage VOD is a REGIONAL control plane, and these tests pin that rule on
//! the wire.
//!
//! The rule: every resource of this service belongs to the AWS Region the request
//! that created it was served in (the SigV4 credential scope's Region). A request
//! served in one Region resolves, lists, mutates and deletes only the resources
//! created in it; one created in another Region is reported missing
//! (`NotFoundException`, HTTP 404) and is left untouched, and the same `Id` is an
//! independent resource in each Region, whose `Arn` and egress `DomainName` carry
//! that Region.
//!
//! Each test is a TABLE over Regions and over the three resource kinds
//! (PackagingGroup, PackagingConfiguration, Asset), so what is asserted is the rule
//! and not one recorded call. The Region a request carries no credential scope for
//! is a row too: it is served in [`vod::DEFAULT_REGION`].

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mediapackagevod_mediapackagevod::state as vod;

/// The Regions this suite signs for. They only have to be distinct, well-formed
/// Region labels — the rule is about the Region a request is signed for, not about
/// any particular one — and they span four partitions-worth of geographies so no
/// case can pass by matching one recorded value. `us-east-1` is deliberately
/// included: it is also [`vod::DEFAULT_REGION`], the Region an unsigned request is
/// served in.
const REGIONS: [&str; 4] = ["us-west-2", "eu-central-1", "ap-southeast-2", "us-east-1"];

/// The one `Id` every Region's resources are created under, so a cross-Region leak
/// shows up as a resolved resource rather than as a missing one.
const SHARED_ID: &str = "vod-shared-id";

/// What a request answered.
#[derive(Debug)]
struct Answer {
    /// The HTTP status.
    status: u16,
    /// restJson1's `x-amzn-errortype`, or `<unset>` on a success.
    errortype: String,
    /// The parsed JSON body (`Null` when the response carried none).
    body: Value,
}

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

/// Send one request signed for `region` (or carrying no credential scope at all when
/// `region` is `None`) and report what it answered.
///
/// The `Authorization` header carries only a SigV4 credential SCOPE: that is what
/// the handlers read the Region from. Signature verification is off for this router,
/// so the signature itself is never checked.
async fn call(
    router: &MultiServiceRouter,
    region: Option<&str>,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let host = format!(
        "mediapackage-vod.{}.amazonaws.com",
        region.unwrap_or(vod::DEFAULT_REGION)
    );
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host.clone())
        .header("content-type", "application/json");
    if let Some(region) = region {
        request = request.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{region}\
                 /mediapackage-vod/aws4_request, SignedHeaders=host, Signature=unverified"
            ),
        );
    }
    let request = request
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");

    let response = router.dispatch(request).await;
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

/// A `CreateAsset` body for `id` in `group`.
fn asset_body(id: &str, group: &str) -> Value {
    json!({
        "id": id,
        "packagingGroupId": group,
        "sourceArn": format!("arn:aws:s3:::test-bucket/{id}.smil"),
        "sourceRoleArn":
            format!("arn:aws:iam::{}:role/MediaPackageVod", engine::emulator_account_id()),
    })
}

/// Create the group, the packaging configuration and the asset `id` in `region`,
/// asserting each create succeeded with the status this service answers it with.
async fn create_all(router: &MultiServiceRouter, region: &str, id: &str) {
    let group = call(
        router,
        Some(region),
        "POST",
        "/packaging_groups",
        Some(json!({ "id": id })),
    )
    .await;
    assert_eq!(
        group.status, 200,
        "case {region:?}/{id:?}: CreatePackagingGroup: expected 200, got {} body {}",
        group.status, group.body
    );
    let config = call(
        router,
        Some(region),
        "POST",
        "/packaging_configurations",
        Some(json!({
            "id": id,
            "packagingGroupId": id,
            "hlsPackage": { "hlsManifests": [{}] },
        })),
    )
    .await;
    assert_eq!(
        config.status, 202,
        "case {region:?}/{id:?}: CreatePackagingConfiguration: expected 202, got {} body {}",
        config.status, config.body
    );
    let asset = call(
        router,
        Some(region),
        "POST",
        "/assets",
        Some(asset_body(id, id)),
    )
    .await;
    assert_eq!(
        asset.status, 200,
        "case {region:?}/{id:?}: CreateAsset: expected 200, got {} body {}",
        asset.status, asset.body
    );
}

/// The three reads that address one resource by `Id`, as
/// `(what a failure names it, the route)`.
fn describe_routes(id: &str) -> Vec<(&'static str, String)> {
    vec![
        ("DescribePackagingGroup", format!("/packaging_groups/{id}")),
        (
            "DescribePackagingConfiguration",
            format!("/packaging_configurations/{id}"),
        ),
        ("DescribeAsset", format!("/assets/{id}")),
    ]
}

/// The three collections, as `(what a failure names it, the route, the output
/// member its page lives under)`.
fn listing_routes() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        (
            "ListPackagingGroups",
            "/packaging_groups",
            "packagingGroups",
        ),
        (
            "ListPackagingConfigurations",
            "/packaging_configurations",
            "packagingConfigurations",
        ),
        ("ListAssets", "/assets", "assets"),
    ]
}

/// The `Id`s one listing reports, in the order the page carries them.
fn listed_ids(page: &Value, member: &str) -> Vec<String> {
    page[member]
        .as_array()
        .map(|records| {
            records
                .iter()
                .filter_map(|record| record["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The same `Id` created in several Regions is one independent resource per Region:
/// every create succeeds (it is not a duplicate of another Region's), and every read
/// of it answers the record of the Region that asked — its own `Arn` and egress
/// `DomainName`, its own derived `ApproximateAssetCount` and `EgressEndpoints`.
#[tokio::test]
async fn the_same_id_is_an_independent_resource_in_every_region() {
    let router = router();
    for region in REGIONS {
        create_all(&router, region, SHARED_ID).await;
    }

    for region in REGIONS {
        let group = call(
            &router,
            Some(region),
            "GET",
            &format!("/packaging_groups/{SHARED_ID}"),
            None,
        )
        .await;
        assert_eq!(
            group.status, 200,
            "case {region:?}: DescribePackagingGroup: expected 200, got {} body {}",
            group.status, group.body
        );
        // An ARN's Region always matches the endpoint that served it.
        let expected_arn = vod::resource_arn("packaging-groups", region, SHARED_ID);
        assert_eq!(
            group.body["arn"].as_str(),
            Some(expected_arn.as_str()),
            "case {region:?}: the group's Arn must be this Region's, got {}",
            group.body["arn"]
        );
        let expected_domain = vod::packaging_group_domain_name(SHARED_ID, region);
        assert_eq!(
            group.body["domainName"].as_str(),
            Some(expected_domain.as_str()),
            "case {region:?}: the group's DomainName must be this Region's, got {}",
            group.body["domainName"]
        );
        // One asset was created in this Region's group, and only that one counts.
        assert_eq!(
            group.body["approximateAssetCount"].as_i64(),
            Some(1),
            "case {region:?}: ApproximateAssetCount must count only this Region's \
             assets, got {}",
            group.body["approximateAssetCount"]
        );

        let config = call(
            &router,
            Some(region),
            "GET",
            &format!("/packaging_configurations/{SHARED_ID}"),
            None,
        )
        .await;
        assert_eq!(
            config.status, 200,
            "case {region:?}: DescribePackagingConfiguration: expected 200, got {} body {}",
            config.status, config.body
        );
        assert_eq!(
            config.body["arn"].as_str(),
            Some(vod::resource_arn("packaging-configurations", region, SHARED_ID).as_str()),
            "case {region:?}: the configuration's Arn must be this Region's, got {}",
            config.body["arn"]
        );

        let asset = call(
            &router,
            Some(region),
            "GET",
            &format!("/assets/{SHARED_ID}"),
            None,
        )
        .await;
        assert_eq!(
            asset.status, 200,
            "case {region:?}: DescribeAsset: expected 200, got {} body {}",
            asset.status, asset.body
        );
        assert_eq!(
            asset.body["arn"].as_str(),
            Some(vod::resource_arn("assets", region, SHARED_ID).as_str()),
            "case {region:?}: the asset's Arn must be this Region's, got {}",
            asset.body["arn"]
        );
        // `EgressEndpoints` is derived from the configurations of the asset's OWN
        // group, so each Region reports exactly its one configuration, at a URL
        // rooted in its own egress origin.
        let endpoints = asset.body["egressEndpoints"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            endpoints.len(),
            1,
            "case {region:?}: expected exactly this Region's one egress endpoint, got {}",
            asset.body["egressEndpoints"]
        );
        assert!(
            endpoints[0]["url"]
                .as_str()
                .is_some_and(|url| url.starts_with(&expected_domain)),
            "case {region:?}: the playback URL must be rooted at {expected_domain:?}, got {}",
            endpoints[0]["url"]
        );

        // Each collection reports this Region's one record, never four.
        for (operation, route, member) in listing_routes() {
            let page = call(&router, Some(region), "GET", route, None).await;
            assert_eq!(
                page.status, 200,
                "case {region:?}: {operation}: expected 200, got {} body {}",
                page.status, page.body
            );
            assert_eq!(
                listed_ids(&page.body, member),
                vec![SHARED_ID.to_string()],
                "case {region:?}: {operation} must report only this Region's records, got {}",
                page.body
            );
        }
    }
}

/// A resource created in one Region does not exist in any other: every operation
/// that addresses it there answers `NotFoundException` (404), the collections there
/// do not report it, and the resource itself is left exactly as it was — a
/// cross-Region update, log configuration or delete reaches nothing.
#[tokio::test]
async fn a_resource_of_another_region_is_missing_from_every_operation() {
    let router = router();
    let home = REGIONS[0];
    create_all(&router, home, SHARED_ID).await;

    // Every Region but the home one, plus the request that carries no credential
    // scope at all (served in `DEFAULT_REGION`).
    let mut foreign: Vec<Option<&str>> = REGIONS
        .iter()
        .filter(|region| **region != home)
        .map(|region| Some(*region))
        .collect();
    foreign.push(None);

    for region in foreign {
        let case = region.unwrap_or("<unsigned>");

        // Reads: the addressed resource is missing here.
        for (operation, route) in describe_routes(SHARED_ID) {
            let answer = call(&router, region, "GET", &route, None).await;
            assert_eq!(
                (answer.status, answer.errortype.as_str()),
                (404, "NotFoundException"),
                "case {case:?}: {operation} on another Region's resource: expected \
                 404 / NotFoundException, got {} / {:?} body {}",
                answer.status,
                answer.errortype,
                answer.body
            );
        }

        // Mutations: they address nothing here, so they change nothing anywhere.
        let mutations: Vec<(&str, &str, String, Option<Value>)> = vec![
            (
                "UpdatePackagingGroup",
                "PUT",
                format!("/packaging_groups/{SHARED_ID}"),
                Some(json!({ "authorization": {
                    "cdnIdentifierSecret":
                        format!("arn:aws:secretsmanager:{case}:111122223333:secret:cdn"),
                    "secretsRoleArn":
                        format!("arn:aws:iam::{}:role/MediaPackageVod", engine::emulator_account_id()),
                } })),
            ),
            (
                "ConfigureLogs",
                "PUT",
                format!("/packaging_groups/{SHARED_ID}/configure_logs"),
                Some(json!({ "egressAccessLogs": { "logGroupName": "/aws/MediaPackage/other" } })),
            ),
            (
                "DeleteAsset",
                "DELETE",
                format!("/assets/{SHARED_ID}"),
                None,
            ),
            (
                "DeletePackagingConfiguration",
                "DELETE",
                format!("/packaging_configurations/{SHARED_ID}"),
                None,
            ),
            (
                "DeletePackagingGroup",
                "DELETE",
                format!("/packaging_groups/{SHARED_ID}"),
                None,
            ),
        ];
        for (operation, method, route, body) in mutations {
            let answer = call(&router, region, method, &route, body).await;
            assert_eq!(
                (answer.status, answer.errortype.as_str()),
                (404, "NotFoundException"),
                "case {case:?}: {operation} on another Region's resource: expected \
                 404 / NotFoundException, got {} / {:?} body {}",
                answer.status,
                answer.errortype,
                answer.body
            );
        }

        // Collections: nothing of the home Region is reported here.
        for (operation, route, member) in listing_routes() {
            let page = call(&router, region, "GET", route, None).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: {operation}: expected 200, got {} body {}",
                page.status, page.body
            );
            assert!(
                listed_ids(&page.body, member).is_empty(),
                "case {case:?}: {operation} must report none of another Region's \
                 records, got {}",
                page.body
            );
        }

        // A body naming another Region's group resolves to nothing, so the create is
        // refused as unprocessable (422) rather than attaching to it.
        for (operation, route, body) in [
            (
                "CreatePackagingConfiguration",
                "/packaging_configurations",
                json!({
                    "id": "cfg-from-another-region",
                    "packagingGroupId": SHARED_ID,
                    "hlsPackage": { "hlsManifests": [{}] },
                }),
            ),
            (
                "CreateAsset",
                "/assets",
                asset_body("asset-from-another-region", SHARED_ID),
            ),
        ] {
            let answer = call(&router, region, "POST", route, Some(body)).await;
            assert_eq!(
                (answer.status, answer.errortype.as_str()),
                (422, "UnprocessableEntityException"),
                "case {case:?}: {operation} naming another Region's packagingGroupId: \
                 expected 422 / UnprocessableEntityException, got {} / {:?} body {}",
                answer.status,
                answer.errortype,
                answer.body
            );
        }
    }

    // The home Region's resources survived every foreign call untouched: still
    // present, still with no CDN authorization and no egress access logs (no foreign
    // UpdatePackagingGroup or ConfigureLogs reached them), and still listed.
    for (operation, route) in describe_routes(SHARED_ID) {
        let answer = call(&router, Some(home), "GET", &route, None).await;
        assert_eq!(
            answer.status, 200,
            "case {home:?}: {operation} after the foreign calls: expected 200, got {} body {}",
            answer.status, answer.body
        );
    }
    let group = call(
        &router,
        Some(home),
        "GET",
        &format!("/packaging_groups/{SHARED_ID}"),
        None,
    )
    .await;
    assert!(
        group.body.get("authorization").is_none_or(Value::is_null),
        "case {home:?}: a foreign UpdatePackagingGroup must not have set Authorization, got {}",
        group.body
    );
    assert!(
        group.body.get("egressAccessLogs").is_none_or(Value::is_null),
        "case {home:?}: a foreign ConfigureLogs must not have set EgressAccessLogs, got {}",
        group.body
    );
    assert_eq!(
        group.body["approximateAssetCount"].as_i64(),
        Some(1),
        "case {home:?}: a foreign DeleteAsset must not have removed this Region's asset, got {}",
        group.body
    );
    for (operation, route, member) in listing_routes() {
        let page = call(&router, Some(home), "GET", route, None).await;
        assert_eq!(
            listed_ids(&page.body, member),
            vec![SHARED_ID.to_string()],
            "case {home:?}: {operation} after the foreign calls must still report this \
             Region's record, got {}",
            page.body
        );
    }
}

/// A delete reaches only the Region it was served in: the same `Id` in every other
/// Region is still there afterwards, with its own ARN, and can be deleted in turn.
#[tokio::test]
async fn a_delete_in_one_region_leaves_the_same_id_in_every_other() {
    let router = router();
    for region in REGIONS {
        create_all(&router, region, SHARED_ID).await;
    }

    // Delete in dependency order (the asset, then the configuration the group
    // references, then the group), one Region at a time.
    for (index, region) in REGIONS.iter().enumerate() {
        for (operation, route) in [
            ("DeleteAsset", format!("/assets/{SHARED_ID}")),
            (
                "DeletePackagingConfiguration",
                format!("/packaging_configurations/{SHARED_ID}"),
            ),
            (
                "DeletePackagingGroup",
                format!("/packaging_groups/{SHARED_ID}"),
            ),
        ] {
            let answer = call(&router, Some(region), "DELETE", &route, None).await;
            assert_eq!(
                answer.status, 204,
                "case {region:?}: {operation}: expected 204, got {} body {}",
                answer.status, answer.body
            );
        }

        // Every Region deleted so far reports the three resources missing; every
        // Region not yet deleted still reports its own.
        for (position, other) in REGIONS.iter().enumerate() {
            let deleted = position <= index;
            for (operation, route) in describe_routes(SHARED_ID) {
                let answer = call(&router, Some(other), "GET", &route, None).await;
                let expected = if deleted { 404 } else { 200 };
                assert_eq!(
                    answer.status, expected,
                    "case {other:?} (deleted={deleted}) after deleting in {region:?}: \
                     {operation}: expected {expected}, got {} body {}",
                    answer.status, answer.body
                );
            }
        }
    }
}
